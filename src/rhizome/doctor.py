"""rhizome doctor — KB pipeline-integrity checks.

Two modes, two failure classes:

  --sources  A-class drift: a registered SOURCE repo can't run its commit gate
             (gate missing / command not on PATH / no INDEX). Walks the
             registry, fail-closed, diagnosable. See run_doctor below.

  --self     B-class drift (this file's other half): the TOOL ITSELF is
             internally inconsistent — e.g. a kb→rhizome-style rename that left
             `adopt.LEFTHOOK_YML` emitting a dead `kb check` while the probe
             checked the same dead name, so they were "consistently wrong" and
             nobody noticed until a fresh adopt wrote a broken hook. `--self`
             never touches a repo; it inspects adopt's own gate template + probe
             for the failure that no source-repo scan can see (the bad command
             only ships into the NEXT repo adopt writes): catch a rename at the
             template the moment it lands, not weeks later in someone's failing
             commit. See run_self_check.

--sources — fleet-level KB pipeline-integrity check.

Fail-closed backstop: nothing else verifies that *every* registered KB source
repo can actually run its commit gate. `rhizome adopt` self-verifies at adopt
time, and the commit hook fails loudly when someone happens to commit — but a
global tool-chain drift (a rename that wrote `kb check` hooks into fresh repos)
leaves a repo "registered but gateless" with nobody watching. This walks the
registry and probes the three pipeline-integrity items per repo, fail-closed
and diagnosable (which repo / which item / why).

Scope is deliberately narrow: the pipeline *plumbing*, not whether content is
indexed. The three items —

  1. gate present      — lefthook.yml has `rhizome check`, or .pre-commit-config.yaml
                          wires an equivalent gate (mirrors adopt._lefthook_state;
                          tolerant of the old `kb check` name, like adopt).
  2. gate resolvable    — which("rhizome"): the command the gate invokes must exist
                          in the hook's fresh shell, else the gate dies exit 127.
  3. INDEX present       — discover_domains() finds at least one docs/INDEX.md domain
                          (reuses the registry's own domain discovery).

are all stat + one which, so the whole fleet scan is sub-second with zero
external dependencies. Content-index coverage (compile produced >0 / silent
skip) is NOT here — that is the compile step's own loud-report.
"""

from __future__ import annotations

import re
import shutil
from pathlib import Path

from . import adopt, contract, sources

PASS = "pass"
FAIL = "fail"

# Legacy command names that must never reappear in the adopt template's gate
# lines. A kb→rhizome rename can leave `kb check` baked into the LEFTHOOK_YML
# constant; `--self` greps for these so the next rename is caught at the
# template, not in some fresh adopt's failing commit hook.
_LEGACY_GATE_NAMES = ("kb",)

# A lefthook `run:` line, e.g. `      run: rhizome check {staged_files}`.
_RUN_LINE_RE = re.compile(r"^\s*run:\s*(?P<cmd>\S+)\b")


def _gate_present(repo_root: Path) -> tuple[bool, str]:
    """Item 1: does the repo carry a KB commit gate at all?

    A gate counts if lefthook.yml names `rhizome check` (or the tolerated old
    `kb check`, matching adopt._lefthook_state), or .pre-commit-config.yaml
    wires the same. Comment lines never count (a commented-out gate is no gate —
    same rule adopt enforces). Returns (ok, detail) where detail names the file
    that satisfied the gate, or every place we looked when it did not.
    """
    checked: list[str] = []
    for fname in ("lefthook.yml", ".pre-commit-config.yaml"):
        f = repo_root / fname
        if not f.is_file():
            checked.append(f"{fname} (absent)")
            continue
        text = f.read_text(encoding="utf-8", errors="replace")
        if any(
            ("rhizome check" in ln or "kb check" in ln)
            for ln in text.splitlines()
            if not ln.lstrip().startswith("#")
        ):
            return True, f"{fname} runs `rhizome check`"
        checked.append(f"{fname} (no `rhizome check` command)")
    return False, "no KB commit gate found: " + "; ".join(checked)


def _gate_resolvable(which) -> tuple[bool, str]:
    """Item 2: does the command the gate invokes resolve on PATH?

    `which("rhizome")` — the SAME real command adopt probes (never the dead
    `kb` alias). If it is missing, every repo's gate would die in the hook's
    fresh shell (`rhizome: command not found`, exit 127), so this is a
    fleet-wide fail. Returns (ok, detail) including the resolved path or that
    which() returned nothing.
    """
    resolved = which("rhizome")
    if resolved:
        return True, f"which(rhizome)={resolved}"
    return (
        False,
        "which(rhizome)=None — gate command not on PATH (gate would exit 127 in the hook's fresh shell)",
    )


def _index_present(repo_root: Path) -> tuple[bool, str, list[str]]:
    """Item 3: does the repo expose at least one KB domain (a docs/INDEX.md)?

    Reuses sources.discover_domains — the exact domain-discovery the indexer,
    surface-hook and recall consume — so doctor agrees with them by
    construction. Returns (ok, detail, domains). A repo with no domain is not
    being indexed at all (its content can never enter the central collection).
    """
    domains = sorted(d["domain"] for d in sources.discover_domains(repo_root))
    if domains:
        return True, f"{len(domains)} domain(s): {', '.join(domains)}", domains
    return False, f"no {contract.INDEX_FILENAME} domain found under {repo_root}", []


def check_source(
    name: str, repo_path: Path, *, gate_resolvable: tuple[bool, str]
) -> dict:
    """Probe one registered source repo. Pure (no printing).

    `gate_resolvable` is computed once for the fleet (it is a workstation-global
    PATH fact, identical for every repo) and threaded in so each row reports it.
    A missing repo can run no checks → it is a fail with that single reason.
    """
    repo_path = repo_path.resolve()
    if not repo_path.is_dir():
        return {
            "name": name,
            "path": str(repo_path),
            "ok": False,
            "checks": [
                {
                    "check": "repo-exists",
                    "status": FAIL,
                    "detail": f"registered path does not exist: {repo_path}",
                }
            ],
        }

    gate_ok, gate_detail = _gate_present(repo_path)
    res_ok, res_detail = gate_resolvable
    idx_ok, idx_detail, _ = _index_present(repo_path)

    checks = [
        {
            "check": "gate-present",
            "status": PASS if gate_ok else FAIL,
            "detail": gate_detail,
        },
        {
            "check": "gate-resolvable",
            "status": PASS if res_ok else FAIL,
            "detail": res_detail,
        },
        {
            "check": "index-present",
            "status": PASS if idx_ok else FAIL,
            "detail": idx_detail,
        },
    ]
    return {
        "name": name,
        "path": str(repo_path),
        "ok": gate_ok and res_ok and idx_ok,
        "checks": checks,
    }


def run_doctor(*, registry: Path | None = None, which=shutil.which) -> dict:
    """Walk the registry and probe every source's pipeline integrity.

    Returns {registry, gate_resolvable: {ok, detail}, sources: [check_source...],
    ok}. `ok` is False if ANY source fails ANY check → the CLI exits non-zero
    (fail-closed). Pure (no printing). Raises sources.SourcesError if the
    registry itself is missing/unreadable (loud, never a silent empty run).
    """
    reg = registry or sources.find_registry()
    entries = sources.load_sources(reg)
    # The gate command resolves (or not) once for the whole workstation — it is
    # a PATH fact, not per-repo. Probe it a single time, report it on every row.
    gate_resolvable = _gate_resolvable(which)
    results = [
        check_source(name, path, gate_resolvable=gate_resolvable)
        for name, path in entries
    ]
    return {
        "registry": str(reg),
        "gate_resolvable": {"ok": gate_resolvable[0], "detail": gate_resolvable[1]},
        "sources": results,
        "ok": all(r["ok"] for r in results),
    }


# ---- --self: tool-chain self-consistency (B-class backstop) -----------------


def _template_gate_commands(template: str) -> list[str]:
    """The command names the adopt lefthook template actually invokes.

    Parses every `run: <cmd> ...` line of LEFTHOOK_YML and returns the first
    token (the executable). Comment lines never count — a commented gate ships
    nothing. This is the ground truth of "what command a freshly adopted repo's
    hook will call in its commit shell".
    """
    cmds: list[str] = []
    for ln in template.splitlines():
        if ln.lstrip().startswith("#"):
            continue
        m = _RUN_LINE_RE.match(ln)
        if m:
            cmds.append(m.group("cmd"))
    return cmds


def _check_template_parses(cmds: list[str]) -> tuple[bool, str]:
    if cmds:
        return (
            True,
            f"parsed gate command(s) from LEFTHOOK_YML: {', '.join(sorted(set(cmds)))}",
        )
    return (
        False,
        "LEFTHOOK_YML has no `run:` gate command — the adopt template ships no gate",
    )


def _check_template_resolvable(cmds: list[str], which) -> tuple[bool, str]:
    """Item: every command the template emits must resolve on PATH.

    This is the exact rename-break judge: a template that emits a command which
    `which` cannot find writes a hook that dies `command not found` (exit 127)
    in the freshly-adopted repo's commit shell. (The kb→rhizome rename left the
    template emitting the unresolvable `kb`.)
    """
    unresolved = sorted({c for c in cmds if which(c) is None})
    if not cmds:
        return False, "no gate command to resolve (template ships no gate)"
    if unresolved:
        return False, (
            f"template gate command(s) not on PATH: {', '.join(unresolved)} — "
            "a freshly adopted repo's hook would exit 127 (command not found) in its commit shell"
        )
    resolved = "; ".join(f"{c} -> {which(c)}" for c in sorted(set(cmds)))
    return True, f"all template gate command(s) resolve: {resolved}"


def _check_template_probe_agree(cmds: list[str]) -> tuple[bool, str]:
    """Item: the command adopt's fail-closed PATH probe checks == the template's.

    A self-consistent rename bug is possible: if BOTH the template and the probe
    say `kb`, the probe happily passes a broken template. The durable fix is the
    shared `adopt._GATE_COMMAND` constant; this asserts the template's parsed
    command(s) are exactly that one probed name — so a future rename that updates
    only one place is caught here, not in the wild.
    """
    probe_cmd = adopt._GATE_COMMAND
    template_cmds = sorted(set(cmds))
    if not template_cmds:
        return (
            False,
            f"probe checks {probe_cmd!r} but the template emits no gate command",
        )
    if template_cmds != [probe_cmd]:
        return False, (
            f"template emits {template_cmds} but adopt's fail-closed probe checks "
            f"which({probe_cmd!r}) — template and probe disagree (the kb→rhizome rename hazard)"
        )
    return True, f"template and probe agree on the gate command: {probe_cmd!r}"


def _check_no_legacy_names(cmds: list[str]) -> tuple[bool, str]:
    """Item (cheap): no retired command name lingers in the template's gates."""
    found = sorted({c for c in cmds if c in _LEGACY_GATE_NAMES})
    if found:
        return (
            False,
            f"legacy gate command name(s) still in LEFTHOOK_YML: {', '.join(found)}",
        )
    return (
        True,
        f"no legacy gate command names ({', '.join(_LEGACY_GATE_NAMES)}) in the template",
    )


def run_self_check(*, which=shutil.which) -> dict:
    """Inspect the tool's own gate template + probe for internal inconsistency.

    Pure (no printing), no repo / registry / filesystem touch — it reads only
    the in-process `adopt.LEFTHOOK_YML` constant and `adopt._GATE_COMMAND`.
    Returns {checks: [{check, status, detail}], ok}; `ok` is False if any check
    fails → the CLI exits non-zero (fail-closed). This is the B-class backstop:
    catch a renamed/broken gate template at the moment it lands.
    """
    cmds = _template_gate_commands(adopt.LEFTHOOK_YML)
    items = [
        ("template-parses", _check_template_parses(cmds)),
        ("template-resolvable", _check_template_resolvable(cmds, which)),
        ("template-probe-agree", _check_template_probe_agree(cmds)),
        ("no-legacy-names", _check_no_legacy_names(cmds)),
    ]
    checks = [
        {"check": name, "status": PASS if ok else FAIL, "detail": detail}
        for name, (ok, detail) in items
    ]
    return {"checks": checks, "ok": all(c["status"] == PASS for c in checks)}
