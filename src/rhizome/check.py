"""rhizome check — validate notes against the §存 contract.

Severity model (fix-on-touch — 别让老 frontmatter 越积累越多):

  ERROR (blocks commit) — the universal spine + the killed/derived legacy fields.
    The latter are losslessly reconstructible (domain←INDEX.md / title←H1 /
    time←git) or pure noise (object_id, ...), so `rhizome check --fix` strips
    them mechanically — no reason to leave them:
      malformed/unterminated frontmatter; description missing/empty;
      keywords missing/empty; kind present but not in the enum;
      killed fields (object_id, ...); derived fields (domain/title/verified).

  WARN (surfaces, never blocks) — soft signals that need human judgment, not a
    mechanical strip:
      decision-only fields on a non-decision note; unknown asset prefixes;
      assets hint vs body 触达资产 section mismatch; externalize candidates;
      unknown fields.

Files without frontmatter are skipped (not notes); the "should be a note but
isn't" case is the indexer's loud-skip job, complementary to this.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

from . import config, contract
from .contract import ContractError

ERROR = "error"
WARN = "warn"

# `git diff --name-status` lines are tab-separated: status field + at least one path.
_NAME_STATUS_MIN_FIELDS = 2
# A domain mapped to fewer than this many dirs is not a duplicate.
_DUPLICATE_MIN_DIRS = 2

# Narrow, single-file frozen-amend approval channel.
# `rhizome amend` sets this env var to ONE absolute file path before spawning the
# real `git commit`; lefthook inherits the env and runs `rhizome check`, which
# then drops ONLY that file's frozen-modification block (logging the approved
# amend) while every other check — including frozen blocks on every other file —
# still fires. Deliberately one path, never a list or wildcard: a provenance
# channel for one approved edit, not a reusable escape hatch.
_AMEND_APPROVED_ENV = "RHIZOME_AMEND_APPROVED"
_MERMAID_VALIDATOR_DIR_ENV = "RHIZOME_MERMAID_VALIDATOR_DIR"

# Directories that never hold KB content (mirrors sources._SKIP_DIRS); the
# repo-level INDEX.md walk skips these plus any hidden dir (.git/.venv/...).
_WALK_SKIP_DIRS = frozenset({".git", ".venv", "__pycache__", "node_modules", "dist"})
_FENCE_START_RE = re.compile(r"^\s*([`~]{3,})\s*([A-Za-z0-9_-]+)?\b.*$")


class Finding:
    __slots__ = ("field", "message", "severity")

    def __init__(self, severity: str, field: str | None, message: str):
        self.severity = severity
        self.field = field
        self.message = message

    def __repr__(self) -> str:  # for test readability
        loc = f"{self.field}: " if self.field else ""
        return f"{self.severity}: {loc}{self.message}"


def check_text(text: str) -> list[Finding]:  # noqa: C901, PLR0912
    """Validate one note's raw text; return findings (possibly empty)."""
    try:
        fm = contract.parse_frontmatter(text)
    except ContractError as exc:
        return [Finding(ERROR, None, str(exc))]
    if fm is None:
        return []  # no frontmatter → not a note → skip (loud-skip backstops)

    findings: list[Finding] = []

    # --- ERROR: the required spine -----------------------------------------
    desc = fm.get("description")
    if not isinstance(desc, str) or not desc.strip():
        findings.append(
            Finding(ERROR, "description", "required — a non-empty one-line string")
        )

    kw = fm.get("keywords")
    if not isinstance(kw, list) or not any(str(x).strip() for x in kw if x is not None):
        findings.append(Finding(ERROR, "keywords", "required — a non-empty list"))

    kind = fm.get("kind")
    if (
        kind is not None and kind not in contract.KINDS
    ):  # null/absent kind → default note
        findings.append(
            Finding(ERROR, "kind", f"must be one of {list(contract.KINDS)}")
        )

    # --- ERROR: status is single-value ---
    if (
        contract.STATUS_FIELD in fm
        and fm.get(contract.STATUS_FIELD) not in contract.STATUS_ALLOWED
    ):
        findings.append(
            Finding(
                ERROR,
                contract.STATUS_FIELD,
                "only the single value 'frozen' is allowed — remove or run `rhizome check --fix`",
            )
        )

    # --- ERROR: legacy killed/derived fields (`rhizome check --fix` strips) ---
    for k in sorted(set(fm) & contract.KILLED_FIELDS):
        findings.append(
            Finding(ERROR, k, "killed field — remove (run `rhizome check --fix`)")
        )

    for k in sorted(set(fm) & contract.DERIVED_FIELDS):
        findings.append(
            Finding(
                ERROR,
                k,
                "derived (domain←INDEX.md / title←H1 / time←git) — remove (run `rhizome check --fix`)",
            )
        )

    sup = fm.get("supersedes")
    if sup not in (None, "", []) and fm.get("kind") != "decision":
        findings.append(
            Finding(
                WARN, "supersedes", "only valid on kind: decision (留痕 for decisions)"
            )
        )

    body_assets = body_asset_ids(text)
    assets = fm.get("assets")
    if assets not in (None, "", []):
        if fm.get("kind") != "decision":
            findings.append(
                Finding(
                    WARN, "assets", "only valid on kind: decision (delivery assets)"
                )
            )
        if not isinstance(assets, list):
            findings.append(
                Finding(WARN, "assets", "must be a list of namespace-prefixed strings")
            )
        else:
            clean_assets = [
                asset for asset in assets if isinstance(asset, str) and asset.strip()
            ]
            if len(clean_assets) > contract.ASSET_EXTERNALIZE_COUNT_THRESHOLD:
                findings.append(
                    Finding(
                        WARN,
                        "assets",
                        "externalize candidate: "
                        f"{len(clean_assets)} assets in one decision doc "
                        f"(threshold {contract.ASSET_EXTERNALIZE_COUNT_THRESHOLD})",
                    )
                )
            for asset in assets:
                if not isinstance(asset, str) or not asset.strip():
                    findings.append(
                        Finding(
                            WARN,
                            "assets",
                            "contains a non-string or empty asset identifier",
                        )
                    )
                    continue
                known = contract.known_asset_prefixes()
                prefix = contract.asset_prefix(asset)
                if prefix not in known:
                    findings.append(
                        Finding(
                            WARN,
                            "assets",
                            f"unknown asset prefix in {asset!r}; canonical prefixes: {sorted(known)}",
                        )
                    )
            for asset in sorted(set(clean_assets) - body_assets):
                findings.append(
                    Finding(
                        WARN,
                        "assets",
                        f"{asset!r} is in frontmatter assets but not in body 触达资产 section; frontmatter is only a hint",
                    )
                )

    if body_assets and not isinstance(assets, list):
        findings.append(
            Finding(
                WARN,
                "assets",
                "body 触达资产 section has asset_id values but frontmatter assets hint is missing",
            )
        )
    elif body_assets and isinstance(assets, list):
        clean_assets = {
            asset.strip()
            for asset in assets
            if isinstance(asset, str) and asset.strip()
        }
        for asset in sorted(body_assets - clean_assets):
            findings.append(
                Finding(
                    WARN,
                    "assets",
                    f"{asset!r} appears in body 触达资产 section but is missing from frontmatter assets hint",
                )
            )

    known = (
        set(contract.ALLOWED_FIELDS)
        | contract.DECISION_ONLY_FIELDS
        | contract.DERIVED_FIELDS
        | contract.KILLED_FIELDS
        | {contract.STATUS_FIELD}
    )
    for k in sorted(set(fm) - known):
        findings.append(Finding(WARN, k, "unknown field, not in §存 contract"))

    findings.extend(mermaid_findings(text))

    return findings


def check_path(path: Path) -> list[Finding]:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        return [Finding(ERROR, None, f"unreadable: {exc}")]
    if not contract.is_note_location(path):
        return []  # outside any KB domain → not a note (e.g. PM issues/) → skip
    from . import links  # 延迟 import: links 反向依赖本模块的 Finding

    return (
        check_text(text)
        + frozen_gate_findings(path, text)
        + links.link_findings(path, text)
    )


# ---- frozen gate (frozen-gate spec) -----------------
#
# Frozen docs are read-only history: increments go through a NEW superseding
# doc, never edits. The gate judges by the HEAD version's frontmatter, NOT the
# working-tree version — otherwise "flip status away from frozen, then edit"
# in the same commit would walk straight through (the downgrade itself counts
# as editing a frozen doc and must go through supersede).


def _git_head_text(repo_root: Path, rel: str) -> str | None:
    """Content of HEAD:<rel>, or None if absent (new file / no HEAD / no git)."""
    try:
        proc = subprocess.run(
            ["git", "-C", str(repo_root), "show", f"HEAD:{rel}"],
            capture_output=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if proc.returncode != 0:
        return None
    return proc.stdout.decode("utf-8", errors="replace")


def _git_staged_text(repo_root: Path, rel: str) -> str | None:
    """Content of the staged blob ``:<rel>`` (the index), or None if absent."""
    try:
        proc = subprocess.run(
            ["git", "-C", str(repo_root), "show", f":{rel}"],
            capture_output=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if proc.returncode != 0:
        return None
    return proc.stdout.decode("utf-8", errors="replace")


def _head_frozen_text(path: Path) -> str | None:
    """HEAD content of `path` if that HEAD version is frozen, else None."""
    repo_root = contract.find_repo_root(path.parent)
    if repo_root is None:
        return None
    try:
        rel = path.resolve().relative_to(repo_root.resolve()).as_posix()
    except ValueError:
        return None
    head = _git_head_text(repo_root, rel)
    if head is None:
        return None
    try:
        head_fm = contract.parse_frontmatter(head)
    except ContractError:
        return None
    return head if contract.is_frozen_fm(head_fm) else None


def head_frozen(path: Path) -> bool:
    """True if the committed (HEAD) version of `path` is a frozen doc."""
    return _head_frozen_text(path) is not None


def _amend_approved_for(path: Path) -> bool:
    """True iff `path` is the single file named by RHIZOME_AMEND_APPROVED.

    The env var holds exactly one absolute path; the match is by resolved path
    so it cannot be tricked by `.`/`..`/symlink spelling. An unset/empty var
    means no approval (the normal commit path), so the frozen guard fires.
    """
    approved = os.environ.get(_AMEND_APPROVED_ENV)
    if not approved:
        return False
    try:
        return Path(approved).resolve() == path.resolve()
    except OSError:
        return False


def frozen_gate_findings(path: Path, text: str) -> list[Finding]:
    """ERROR if `path` diverges from a frozen HEAD version. New files pass.

    Exactly one narrow exception: the file named by RHIZOME_AMEND_APPROVED gets
    a logged, approved bypass of THIS block (via `rhizome amend`).
    The bypass is provenance, not a gate relaxation — it covers only the frozen
    block for the one named file; all other findings (frontmatter, Mermaid,
    links) on this file and the frozen block on every other file are untouched.
    """
    head = _head_frozen_text(path)
    if head is None or head == text:
        return []
    if _amend_approved_for(path):
        # Loud, single-line, greppable audit trail in the hook output; the
        # durable record is the commit's Frozen-Amend-Approved trailer.
        print(f"rhizome check: approved amend: {path}", file=sys.stderr)
        return []
    return [
        Finding(
            ERROR,
            None,
            "frozen document modified — HEAD version is a read-only snapshot "
            "(status: frozen / kind: decision); supersede with a new doc instead; "
            "bypass requires maintainer approval "
            "(audited amend: `rhizome amend <file> -m <reason>`)",
        )
    ]


def _relocate_exempt(repo_root: Path, op: str, parts: list[str], head: str) -> bool:  # noqa: PLR0911
    """True iff this staged D/R of a frozen doc is an audited, content-preserving
    relocate (`rhizome relocate`), not an illicit delete or a smuggled edit.

    Requires ALL of: (1) a `.relocate-ledger` record matching this old path AND
    the content hash of its HEAD version — provenance + proof the recorded
    content is exactly what is leaving; (2) the relocated copy actually exists
    content-identical — verified against the staged blob for a within-repo move
    (where the add is in this same commit), and trusted to the ledger for a
    cross-repo move (the target's add lives in another repo, unseeable here).

    The semantics stay intact: an in-place edit is an `M` (handled by the
    per-file gate, untouched), and a delete with no ledger record never matches.
    """
    from . import contract as _contract  # local: avoid top-level coupling churn
    from . import relocate

    old_rel = parts[1]
    rec = relocate.find_record(repo_root, old_rel, relocate.content_hash(head))
    if rec is None:
        return False  # no provenance → block (illicit / unrecorded delete)
    this_repo = _contract.repo_name(repo_root)
    new_repo = rec["new_identity"].split(":", 1)[0] if rec["new_identity"] else ""
    if new_repo and new_repo != this_repo:
        # cross-repo: the content-identical copy lives in the TARGET repo. Its add
        # is in another repo's commit (unseeable in git here), but the target's
        # working tree IS readable locally — so verify the byte-identical copy
        # actually exists there, rather than trusting the (writable) ledger alone.
        # The HEAD hash is public, so a hand-forged ledger line would otherwise
        # green-light deleting any frozen doc with no real copy anywhere (and a
        # within-repo delete could mislabel itself cross-repo to skip the staged
        # byte-check below). Reading the target's file shuts both down.
        from . import sources

        try:
            srcs = sources.load_sources(None)
        except sources.SourcesError:
            return False
        tgt_root = next((p for n, p in srcs if n == new_repo), None)
        if tgt_root is None:
            return False
        tgt_root = tgt_root.resolve()
        tgt_file = (tgt_root / rec["new_rel"]).resolve()
        if tgt_root not in tgt_file.parents:  # containment: new_rel escapes target
            return False
        try:
            tgt_text = tgt_file.read_text(encoding="utf-8")
        except OSError:
            return False
        return relocate.content_hash(tgt_text) == relocate.content_hash(head)
    # within-repo: prove the relocated copy is staged here, byte-for-byte.
    new_rel = parts[2] if op == "R" and len(parts) > 2 else rec["new_rel"]  # noqa: PLR2004
    staged = _git_staged_text(repo_root, new_rel)
    return staged is not None and relocate.content_hash(
        staged
    ) == relocate.content_hash(head)


def staged_frozen_findings(repo_root: Path) -> list[Finding]:  # noqa: C901
    """Repo-level: ERROR on staged delete/rename of a HEAD-frozen KB note.

    Per-file checks never see deletions (the path is gone from {staged_files}),
    so the D/R channel is closed here from `git diff --cached --name-status`.
    Only paths inside a KB domain are gated (PM issues etc. stay out of scope).

    One narrow exception: an audited, content-preserving `rhizome relocate`
    (recorded in `.relocate-ledger`) is a position change, not a content edit,
    so its staged delete/rename is allowed (see `_relocate_exempt`).
    """
    try:
        proc = subprocess.run(
            ["git", "-C", str(repo_root), "diff", "--cached", "--name-status", "-M"],
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return []
    if proc.returncode != 0:
        return []
    findings: list[Finding] = []
    for line in proc.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) < _NAME_STATUS_MIN_FIELDS or not parts[0]:
            continue
        op = parts[0][0]
        if op not in ("D", "R"):
            continue
        old_rel = parts[1]
        if not old_rel.endswith(".md"):
            continue
        if contract.find_domain_dir((repo_root / old_rel).parent, repo_root) is None:
            continue  # outside any KB domain → not a note → not gated
        head = _git_head_text(repo_root, old_rel)
        if head is None:
            continue
        try:
            head_fm = contract.parse_frontmatter(head)
        except ContractError:
            continue
        if not contract.is_frozen_fm(head_fm):
            continue
        if _relocate_exempt(repo_root, op, parts, head):
            continue  # audited content-preserving relocate — position change, not edit
        verb = "deletion" if op == "D" else "rename"
        findings.append(
            Finding(
                ERROR,
                None,
                f"{old_rel}: staged {verb} of a frozen document (status: frozen / "
                "kind: decision in HEAD) — frozen docs are read-only history; supersede instead",
            )
        )
    return findings


def has_errors(findings: list[Finding]) -> bool:
    return any(f.severity == ERROR for f in findings)


# ---- Mermaid parser gate ----------------------------------------------------
#
# Mermaid diagrams are reader-facing structure in KB docs. A broken diagram is
# not a soft style issue, so kb check treats parser failures as ERROR. The actual
# parser lives in a tiny Node sidecar pinned under tools/kb/mermaid-validator:
# using Mermaid's own JS parser is more correct than a Python regex linter, while
# avoiding mmdc/Chromium rendering keeps the hook fast and deterministic.


def _tool_root() -> Path:
    return Path(__file__).resolve().parents[2]


def _mermaid_validator_dir() -> Path:
    env = os.environ.get(_MERMAID_VALIDATOR_DIR_ENV)
    if env:
        return config.expand_path(env)

    cfg = config.load_config()
    mermaid = cfg.get("mermaid")
    if mermaid is not None:
        if not isinstance(mermaid, dict):
            raise config.ConfigError("[mermaid] must be a TOML table")
        raw = mermaid.get("validator_dir")
        if raw is not None:
            if not isinstance(raw, str) or not raw.strip():
                raise config.ConfigError(
                    "mermaid.validator_dir must be a non-empty string"
                )
            return config.expand_path(raw)

    return _tool_root() / "mermaid-validator"


def _mermaid_validator_script() -> Path:
    return _mermaid_validator_dir() / "validate-mermaid.mjs"


def mermaid_parser_available() -> bool:
    """True when the local Mermaid JS parser sidecar is installed."""
    try:
        root = _mermaid_validator_dir()
    except config.ConfigError:
        return False
    return (
        shutil.which("node") is not None
        and _mermaid_validator_script().is_file()
        and (root / "node_modules" / "mermaid").is_dir()
    )


def mermaid_blocks(text: str) -> list[tuple[int, str]]:
    """Return (start_line, code) for fenced ```mermaid blocks."""
    blocks: list[tuple[int, str]] = []
    in_block = False
    fence = ""
    start_line = 0
    buf: list[str] = []
    for lineno, raw in enumerate(text.splitlines(), start=1):
        if not in_block:
            match = _FENCE_START_RE.match(raw)
            if not match:
                continue
            lang = (match.group(2) or "").lower()
            if lang != "mermaid":
                continue
            fence = match.group(1)
            in_block = True
            start_line = lineno + 1
            buf = []
            continue
        stripped = raw.lstrip()
        if stripped.startswith(fence):
            blocks.append((start_line, "\n".join(buf)))
            in_block = False
            fence = ""
            buf = []
            continue
        buf.append(raw)
    return blocks


def mermaid_findings(text: str) -> list[Finding]:  # noqa: PLR0911
    blocks = mermaid_blocks(text)
    if not blocks:
        return []

    node = shutil.which("node")
    if node is None:
        return [
            Finding(
                ERROR,
                "mermaid",
                "Mermaid code block requires the pinned JS parser, but node is not on PATH",
            )
        ]

    try:
        root = _mermaid_validator_dir()
        script = _mermaid_validator_script()
    except config.ConfigError as exc:
        return [Finding(ERROR, "mermaid", f"config error: {exc}")]
    if not script.is_file():
        return [Finding(ERROR, "mermaid", f"validator script missing: {script}")]
    if not (root / "node_modules" / "mermaid").is_dir():
        return [
            Finding(
                ERROR,
                "mermaid",
                "parser dependencies missing — run "
                f"`npm install --prefix {root}` before committing Mermaid diagrams",
            )
        ]

    payload = [{"line": line, "code": code} for line, code in blocks]
    try:
        proc = subprocess.run(
            [node, str(script)],
            input=json.dumps(payload, ensure_ascii=False),
            capture_output=True,
            text=True,
            timeout=20,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        return [Finding(ERROR, "mermaid", f"validator failed to run: {exc}")]

    try:
        result = json.loads(proc.stdout)
    except json.JSONDecodeError:
        detail = (proc.stderr or proc.stdout).strip()
        return [
            Finding(ERROR, "mermaid", f"validator returned non-JSON output: {detail}")
        ]

    if proc.returncode not in (0, 1):
        detail = result.get("tool_error") or (proc.stderr or proc.stdout).strip()
        return [Finding(ERROR, "mermaid", f"validator tool error: {detail}")]

    findings: list[Finding] = []
    for item in result.get("findings", []):
        line = item.get("line", "?")
        msg = str(item.get("message") or "Mermaid parser rejected this diagram").strip()
        findings.append(Finding(ERROR, "mermaid", f"line {line}: {msg}"))
    return findings


# ---- repo-level: duplicate-domain guard ------------------------------------
#
# 守卫左移: this walks the whole repo's INDEX.md tree at commit time, derives
# each domain's C2 node-chain path (contract.derive_node_chain_domain), and
# FAILs if two distinct physical INDEX.md directories collapse onto the same
# domain path — keeping the bad layout out of the commit. The fix: give the
# colliding branches a distinguishing INDEX.md so their node chains differ.


def _iter_index_dirs(repo_root: Path):
    """Yield every directory under ``repo_root`` that holds an INDEX.md.

    Pure filesystem walk, pruning .git/.venv/node_modules/etc. and any hidden
    directory so it stays in the hundred-millisecond range on a real repo. The
    repo root itself is included (its INDEX.md, if any, derives to "" and is
    ignored by the collision pass — a root INDEX.md is not a domain).
    """
    import os

    repo_root = repo_root.resolve()
    for dirpath, dirnames, filenames in os.walk(repo_root):
        dirnames[:] = [
            d for d in dirnames if d not in _WALK_SKIP_DIRS and not d.startswith(".")
        ]
        if contract.INDEX_FILENAME in filenames:
            yield Path(dirpath)


def duplicate_domain_findings(repo_root: Path) -> list[Finding]:
    """Scan the repo's INDEX.md tree; ERROR on C2 node-chain domain collisions.

    Two distinct physical INDEX.md directories that derive to the same C2 domain
    path (because a non-domain intermediate segment differs) collide and would
    silently merge at index time — this returns one ERROR Finding per colliding
    domain, naming the physical paths and the contended domain, with the fix hint.
    Repo-root INDEX.md (empty domain) never collides.
    """
    repo_root = repo_root.resolve()
    by_domain: dict[str, list[str]] = {}
    for idx_dir in _iter_index_dirs(repo_root):
        try:
            domain = contract.derive_node_chain_domain(idx_dir, repo_root)
        except ValueError:
            continue  # outside the repo (shouldn't happen for a walked path)
        if not domain:
            continue  # repo-root INDEX.md is not a domain
        rel = idx_dir.relative_to(repo_root).as_posix()
        by_domain.setdefault(domain, []).append(rel)

    findings: list[Finding] = []
    for domain, dirs in sorted(by_domain.items()):
        if len(dirs) < _DUPLICATE_MIN_DIRS:
            continue
        dirs = sorted(dirs)
        findings.append(
            Finding(
                ERROR,
                "domain",
                f"duplicate-domain {domain!r}: physical paths "
                + " and ".join(repr(d) for d in dirs)
                + f" both derive to domain {domain!r} (C2 node-chain) — they would "
                "collide at index time. Add a distinguishing INDEX.md to the "
                "intermediate directory that differs so their node chains differ.",
            )
        )
    return findings


def body_asset_ids(text: str) -> set[str]:
    """Extract asset_id cells from a body `触达资产` / `Touched Assets` section.

    This is intentionally mechanical and narrow: it only reads explicit section
    tables/lists and never infers that a document should have such a section.
    """
    split = contract.split_frontmatter(text)
    body = split[1] if split else text
    assets: set[str] = set()
    in_section = False
    in_fence = False
    for raw in body.splitlines():
        line = raw.strip()
        if line.startswith("```"):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        if line.startswith("#"):
            heading = line.lstrip("#").strip().lower()
            if in_section:
                break
            if "触达资产" in heading or "touched assets" in heading:
                in_section = True
            continue
        if not in_section:
            continue
        cells = line.split("|") if "|" in line else [line]
        for cell in cells:
            candidate = cell.strip().strip("`").strip()
            if contract.asset_prefix(candidate) in contract.known_asset_prefixes():
                assets.add(candidate)
    return assets
