"""rhizome adopt — one-shot, idempotent KB source-repo adoption.

Converges a repo onto the three mechanical adoption steps: a [[source]] row in
kb-sources.toml, a domain INDEX.md skeleton (only when the repo has no domain
yet), and the lefthook commit gate. Every step probes current state and only
writes what is missing, so re-running on an adopted repo is a no-op.

Plan-then-apply: every precondition (repo shape, name/path conflicts, missing
-d/-k, lefthook binary, case-variant index.md) is checked before the first
write, so a failed run never leaves a half-adopted repo behind.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import tomllib
from pathlib import Path

from . import check, contract, sources

OK = "ok"
CHANGED = "changed"
SKIPPED = "skipped"

_DEFAULT_WORKSPACE_ROOT = "~/workspace"

# The single source of truth for the gate command name. The lefthook template
# below invokes `<_GATE_COMMAND> check ...`, and the fail-closed PATH probe in
# _lefthook_state checks `which(_GATE_COMMAND)`. Keeping both on this one
# constant is what makes them rename-safe TOGETHER: a future rename that misses
# one place writes a broken hook, and `rhizome doctor --self` asserts the
# template's command == this constant == on PATH.
_GATE_COMMAND = "rhizome"

# Per-file contract check plus the repo-level duplicate-domain and frozen
# delete/rename guards.
LEFTHOOK_YML = """\
# lefthook.yml — local git hooks for this KB source repo (written by rhizome adopt).
# Install: lefthook install (one-time, per clone/worktree).
# rhizome check is domain-aware: it only validates changed Markdown inside
# a KB domain (a dir with an INDEX.md ancestor); source code and scratch docs
# outside any domain are skipped. Inside a domain, a note missing
# description/keywords, with an invalid kind, or carrying killed/derived legacy
# fields, or containing an invalid Mermaid diagram blocks the commit; run
# `rhizome check --fix` to strip legacy fields losslessly.

pre-commit:
  commands:
    rhizome-check:
      run: rhizome check {staged_files}
      glob: "*.md"
    rhizome-duplicate-domains:
      run: rhizome check --duplicate-domains --staged-frozen
"""


class AdoptError(Exception):
    """Convergence failure (conflict, blocked write, failed install) → exit 1."""


class AdoptUsageError(AdoptError):
    """Bad arguments / environment (missing repo, -d/-k, lefthook) → exit 2."""


# ---- resolve ----------------------------------------------------------------


def _resolve_repo(arg: str, workspace_root: Path) -> Path:
    """Path form (contains '/' or is '.'/'..') walks up to the repo root; a bare
    name resolves to exactly workspace_root/<name> (no fallback chain)."""
    if "/" in arg or arg in (".", ".."):
        p = Path(arg).expanduser().resolve()
        if not p.exists():
            raise AdoptUsageError(f"no such path: {p}")
        root = contract.find_repo_root(p if p.is_dir() else p.parent)
        if root is None:
            raise AdoptUsageError(f"{p} is not inside a git repo")
        return root
    cand = (workspace_root / arg).resolve()
    if not cand.is_dir():
        raise AdoptUsageError(
            f"no repo named {arg!r} under {workspace_root} — pass a path instead"
        )
    if not (cand / ".git").exists():
        raise AdoptUsageError(f"{cand} has no .git — not a repo checkout")
    return cand


def _reject_worktree(repo_root: Path) -> None:
    # The registry must point at a stable path and hooks land in the shared git
    # dir, so adoption only makes sense from the main checkout.
    if (repo_root / ".git").is_file():
        raise AdoptUsageError(
            f"{repo_root} is a linked worktree or submodule (.git is a file); "
            "run rhizome adopt from the main checkout"
        )


def _validate_name(name: str) -> None:
    # Same character rule load_sources() enforces at read time (sources.py).
    if "/" in name or "\\" in name or ".." in name or name.startswith("~"):
        raise AdoptError(
            f"repo dir name {name!r} is not a valid source name (no /, \\, .., leading ~)"
        )


# ---- registry ---------------------------------------------------------------


def _registry_status(data: dict, ws_literal: Path, name: str, repo_root: Path) -> str:
    """OK if already registered as (name → repo_root); 'missing' if absent.

    Conflicts raise: same name at a different path, or the same path already
    registered under another name (would double-scan the repo).
    """
    target = repo_root.resolve()
    for entry in data.get("source", []):
        ename = entry.get("name")
        if not ename:
            continue
        epath = (
            Path(entry["path"]).expanduser()
            if entry.get("path")
            else ws_literal / ename
        ).resolve()
        if ename == name:
            if epath == target:
                return OK
            raise AdoptError(
                f"source {name!r} already registered with a different path ({epath}); resolve manually"
            )
        if epath == target:
            raise AdoptError(f"this repo is already registered as source {ename!r}")
    return "missing"


def _tilde(p: Path) -> str:
    p = p.resolve()
    try:
        return "~/" + p.relative_to(Path.home().resolve()).as_posix()
    except ValueError:
        return str(p)


def _append_source(reg: Path, name: str, repo_root: Path, ws_literal: Path) -> None:
    """Append a [[source]] block as text (the registry is hand-maintained with
    comments; never round-trip it through a serializer). Atomic via os.replace."""
    text = reg.read_text(encoding="utf-8")
    lines = ["[[source]]", f"name = {contract._dquote(name)}"]
    if repo_root.resolve() != (ws_literal / name).resolve():
        lines.append(f"path = {contract._dquote(_tilde(repo_root))}")
    block = "\n".join(lines) + "\n"
    if not text.endswith("\n"):
        text += "\n"
    tmp = reg.with_name("." + reg.name + ".adopt-tmp")
    tmp.write_text(text + "\n" + block, encoding="utf-8")
    os.replace(tmp, reg)
    # The registry feeds three consumers (indexer / surface-hook / recall);
    # verify the write parses and carries the new name before declaring done.
    try:
        names = {n for n, _ in sources.load_sources(reg)}
    except sources.SourcesError as exc:
        raise AdoptError(f"registry self-check failed after append: {exc}")
    if name not in names:
        raise AdoptError(f"registry self-check failed: {name!r} missing after append")


# ---- INDEX skeleton ---------------------------------------------------------


def _guard_index_case_variant(repo_root: Path) -> None:
    # APFS is case-insensitive: writing docs/INDEX.md would silently OVERWRITE
    # an existing docs/index.md. Checked unconditionally (not just on the
    # skeleton path): on Python 3.13+ rglob's literal-pattern existence check
    # also makes discover_domains see index.md as a domain, so a case-variant
    # repo must be renamed before adoption either way.
    docs = repo_root / "docs"
    if not docs.is_dir():
        return
    for fn in os.listdir(docs):
        if (
            fn.lower() == contract.INDEX_FILENAME.lower()
            and fn != contract.INDEX_FILENAME
        ):
            raise AdoptError(
                f"docs/{fn} exists (case variant of {contract.INDEX_FILENAME}); "
                "rename it first — writing on a case-insensitive FS would overwrite it"
            )


def _write_index(
    repo_root: Path, name: str, description: str, keywords: list[str]
) -> Path:
    docs = repo_root / "docs"
    docs.mkdir(exist_ok=True)
    dest = docs / contract.INDEX_FILENAME
    if dest.exists():
        raise AdoptError(f"{dest} already exists (rhizome does not overwrite)")
    fm = contract.render_frontmatter(
        description=description, keywords=keywords, kind="index"
    )
    body = f"# {name} docs\n\n{description}\n\n当前入口:暂无;用 `rhizome new` 在本域落第一篇文档。"
    dest.write_text(contract.render_note(fm, body), encoding="utf-8")
    findings = check.check_path(dest)
    if check.has_errors(findings):
        msgs = "; ".join(f.message for f in findings if f.severity == check.ERROR)
        raise AdoptError(f"INDEX skeleton failed rhizome check: {dest}: {msgs}")
    return dest


# ---- lefthook ---------------------------------------------------------------


def _lefthook_state(repo_root: Path, runner, which) -> dict:
    state: dict = {
        "binary": which("lefthook"),
        # The gate command the template actually invokes: probe the REAL command
        # via the shared constant, not a dead alias. `doctor --self` asserts this
        # constant == the template's command.
        "gate_on_path": which(_GATE_COMMAND) is not None,
        "yml_exists": (repo_root / "lefthook.yml").is_file(),
        "yml_has_kb_check": False,
        "hooks_path": None,
        "other_manager": [
            p for p in (".pre-commit-config.yaml", ".husky") if (repo_root / p).exists()
        ],
        "installed": False,
    }
    if state["yml_exists"]:
        text = (repo_root / "lefthook.yml").read_text(
            encoding="utf-8", errors="replace"
        )
        state["yml_has_kb_check"] = any(
            ("rhizome check" in ln or "kb check" in ln)
            for ln in text.splitlines()
            if not ln.lstrip().startswith("#")
        )
    # A repo may wire the rhizome gate through its existing pre-commit framework
    # instead of lefthook. That counts as converged —
    # installing lefthook on top would RENAME the framework's hook to .old and
    # silently disable its other checks.
    state["precommit_gate"] = False
    pc = repo_root / ".pre-commit-config.yaml"
    if pc.is_file():
        pc_text = pc.read_text(encoding="utf-8", errors="replace")
        state["precommit_gate"] = any(
            ("rhizome check" in ln or "kb check" in ln)
            for ln in pc_text.splitlines()
            if not ln.lstrip().startswith("#")
        )
    res = runner(
        ["git", "config", "core.hooksPath"],
        cwd=repo_root,
        capture_output=True,
        text=True,
    )
    if (
        getattr(res, "returncode", 1) == 0
        and (getattr(res, "stdout", "") or "").strip()
    ):
        state["hooks_path"] = res.stdout.strip()
    pre = repo_root / ".git" / "hooks" / "pre-commit"
    if pre.is_file():
        state["installed"] = "lefthook" in pre.read_text(
            encoding="utf-8", errors="replace"
        )
    return state


# ---- the convergence --------------------------------------------------------


def count_stray_md(repo_root: Path) -> int:
    """Markdown files outside any domain — rhizome does not index them (FYI only)."""
    n = 0
    for p in repo_root.rglob("*.md"):
        if not sources._SKIP_DIRS.isdisjoint(p.parts):
            continue
        if p.name == contract.INDEX_FILENAME:
            continue
        if sources.note_domain(p, repo_root) is None:
            n += 1
    return n


def run_adopt(
    repo_arg: str,
    *,
    description: str | None = None,
    keywords: list[str] | None = None,
    registry: Path | None = None,
    runner=subprocess.run,
    which=shutil.which,
    cwd: Path | None = None,
) -> dict:
    """Converge a repo onto the adopted state; pure (no printing).

    Returns {repo, name, registry, steps: [{step, status, detail}], warnings,
    stray_md}. Raises AdoptUsageError (exit 2) / AdoptError (exit 1).
    """
    cwd = cwd or Path.cwd()
    reg = registry or sources.find_registry(cwd)
    try:
        data = tomllib.loads(reg.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise AdoptUsageError(f"{reg}: {exc}")
    # The literal workspace_root decides whether a `path =` line is needed.
    # Deliberately NOT the KB_WORKSPACE_ROOT env override: the other registry
    # consumers (<service-manager> indexer, surface-hook) run without this process's env,
    # so a bare-name row must resolve correctly against the file's own base.
    ws_literal = Path(data.get("workspace_root", _DEFAULT_WORKSPACE_ROOT)).expanduser()

    repo_root = _resolve_repo(repo_arg, ws_literal)
    _reject_worktree(repo_root)
    name = repo_root.name
    _validate_name(name)
    _guard_index_case_variant(repo_root)

    # ---- probe everything before the first write ----
    warnings: list[str] = []
    reg_status = _registry_status(data, ws_literal, name, repo_root)

    domains = sources.discover_domains(repo_root)
    need_index = not domains
    if need_index:
        if not description or not keywords:
            raise AdoptUsageError(
                f"{name} has no KB domain yet; pass -d/--description and "
                f"-k/--keywords for the docs/{contract.INDEX_FILENAME} skeleton"
            )
        description = contract.validate_description(description)
        keywords = contract.validate_keywords(keywords)
    elif description or keywords:
        warnings.append("-d/-k ignored: repo already has domain(s)")

    lh = _lefthook_state(repo_root, runner, which)
    gate_via_precommit = lh["precommit_gate"] and not lh["yml_exists"]
    if lh["binary"] is None and not gate_via_precommit:
        raise AdoptUsageError("lefthook not found on PATH (brew install lefthook)")
    if not lh["gate_on_path"]:
        # Fail closed, never warn-and-continue: a repo registered with a gate command
        # that dies in the hook's fresh shell is precisely the silent drift adopt exists
        # to prevent (the registry/index steps already self-verify before declaring done).
        raise AdoptUsageError(
            "`rhizome` not on PATH — the lefthook gate would be written but fail in the "
            "hook's fresh shell, leaving a registered repo whose KB check never runs. "
            "Install it (~/.local/bin) and re-run."
        )
    if not gate_via_precommit:
        for mgr in lh["other_manager"]:
            warnings.append(
                f"{mgr} present — another hook manager may compete with lefthook"
            )

    # ---- apply ----
    steps: list[dict] = []

    if reg_status == OK:
        steps.append(
            {
                "step": "registry",
                "status": OK,
                "detail": f"already listed in {reg.name}",
            }
        )
    else:
        _append_source(reg, name, repo_root, ws_literal)
        steps.append(
            {
                "step": "registry",
                "status": CHANGED,
                "detail": f"appended [[source]] {name!r} to {reg}",
            }
        )

    if need_index:
        dest = _write_index(repo_root, name, description, keywords)
        steps.append(
            {
                "step": "index",
                "status": CHANGED,
                "detail": f"wrote {dest.relative_to(repo_root)} (domain: docs), rhizome check ok",
            }
        )
    else:
        steps.append(
            {
                "step": "index",
                "status": OK,
                "detail": f"{len(domains)} domain(s) already discovered",
            }
        )

    wrote_yml = False
    if gate_via_precommit:
        if not (repo_root / ".git" / "hooks" / "pre-commit").is_file():
            warnings.append(
                ".pre-commit-config.yaml has the kb gate but its hook is not installed — run `pre-commit install`"
            )
        steps.append(
            {
                "step": "lefthook",
                "status": OK,
                "detail": "gate already wired via pre-commit framework (.pre-commit-config.yaml runs rhizome check)",
            }
        )
        return _result(reg, repo_root, name, steps, warnings)
    if not lh["yml_exists"]:
        (repo_root / "lefthook.yml").write_text(LEFTHOOK_YML, encoding="utf-8")
        wrote_yml = True
    elif not lh["yml_has_kb_check"]:
        warnings.append(
            "existing lefthook.yml has no `rhizome check` command — left untouched, add the gate manually"
        )

    if lh["hooks_path"]:
        steps.append(
            {
                "step": "lefthook",
                "status": SKIPPED,
                "detail": f"core.hooksPath={lh['hooks_path']} is occupied — "
                + ("wrote lefthook.yml but " if wrote_yml else "")
                + "skipped lefthook install (not force-installing)",
            }
        )
    elif lh["installed"] and not wrote_yml:
        steps.append(
            {
                "step": "lefthook",
                "status": OK,
                "detail": "lefthook.yml present, hook installed",
            }
        )
    else:
        res = runner(
            ["lefthook", "install"], cwd=repo_root, capture_output=True, text=True
        )
        if getattr(res, "returncode", 1) != 0:
            raise AdoptError(
                f"lefthook install failed: {(getattr(res, 'stderr', '') or '').strip()}"
            )
        detail = ("wrote lefthook.yml, " if wrote_yml else "") + "lefthook install ok"
        steps.append({"step": "lefthook", "status": CHANGED, "detail": detail})

    return _result(reg, repo_root, name, steps, warnings)


def _result(
    reg: Path, repo_root: Path, name: str, steps: list[dict], warnings: list[str]
) -> dict:
    return {
        "repo": str(repo_root),
        "name": name,
        "registry": str(reg),
        "steps": steps,
        "warnings": warnings,
        "stray_md": count_stray_md(repo_root),
    }
