"""rhizome relocate — cross-repo / cross-domain KB note migration, frozen-aware.

Moving a §存 note to another domain or another KB source repo is not a plain
`mv`: identity is position-derived (`<repo>:<domain>:<slug>`), and the commit
gate treats a staged delete/rename of a frozen doc as a violation. This command
makes the move first-class.

What it does (dry-run by default; `--apply` to execute):

  1. Resolve the SOURCE note: identity, kind, frozen status, and every reverse
     reference to its slug across all registered KB sources (frontmatter
     ``links:`` entries + body ``[[slug]]`` wikilinks).
  2. Validate the TARGET: ``<repo>`` is a registered KB source and ``<domain>``
     is a real domain (a directory with its own INDEX.md → non-empty C2 chain).
  3. Move the file, recompute identity, and — only when the slug actually
     changes — rewrite every non-frozen referrer globally.
  4. Frozen content-preserving relocate: a §存 note's links are slug-based and
     position-independent, so a slug-preserving move never edits the note's own
     bytes. The content hash is therefore invariant — a frozen relocate is a
     pure position change, not a content edit. We record provenance (old→new
     identity + sha256 + time) to an append-only ``.relocate-ledger`` in each
     repo, which the frozen gate reads to distinguish an audited relocate from
     an illicit delete.

The tool is filesystem-only: it moves bytes and writes ledger lines. Staging
and committing in each repo is left to the repo's own mechanism (watcher /
manual), exactly like `rhizome new`.
"""

from __future__ import annotations

import hashlib
import re
import subprocess
import tomllib
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path

from . import contract, sources

# One append-only provenance ledger per repo, at the repo root (the frozen gate
# that reads it is itself repo-level). Mirrors amend's `.frozen-amend-ledger`
# pattern: greppable audit trail, one line per move.
RELOCATE_LEDGER_NAME = ".relocate-ledger"
_LEDGER_FIELDS = 6  # ts, content_hash, old_identity, new_identity, old_rel, new_rel

# Directories that never hold KB content (mirrors sources._SKIP_DIRS).
_SKIP_DIRS = frozenset({".git", ".venv", "__pycache__", "node_modules", "dist"})


class RelocateError(Exception):
    """Relocate cannot proceed (not a note, bad target, collision) → exit 1."""


class RelocateUsageError(RelocateError):
    """Bad arguments (no source/--to, malformed --to, no --apply target) → exit 2."""


# ---- content hash + ledger ------------------------------------------------


def content_hash(text: str) -> str:
    """sha256 of the note's UTF-8 bytes — the content-preserving invariant."""
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def read_ledger(repo_root: Path) -> list[dict]:
    """Parse a repo's ``.relocate-ledger`` into records (empty if absent)."""
    ledger = repo_root / RELOCATE_LEDGER_NAME
    if not ledger.is_file():
        return []
    records: list[dict] = []
    for line in ledger.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        parts = line.split("\t")
        if len(parts) < _LEDGER_FIELDS:
            continue
        records.append(
            {
                "ts": parts[0],
                "content_hash": parts[1],
                "old_identity": parts[2],
                "new_identity": parts[3],
                "old_rel": parts[4],
                "new_rel": parts[5],
            }
        )
    return records


def find_record(repo_root: Path, old_rel: str, chash: str) -> dict | None:
    """The ledger record matching this old path AND content hash, or None.

    Both must match: the path pins which note, the hash proves the recorded
    content is exactly what is being removed (content-preserving).
    """
    for r in read_ledger(repo_root):
        if r["old_rel"] == old_rel and r["content_hash"] == chash:
            return r
    return None


def is_relocate_recorded(repo_root: Path, old_rel: str, chash: str) -> bool:
    """True if an audited relocate of ``old_rel`` with this content hash exists."""
    return find_record(repo_root, old_rel, chash) is not None


def append_ledger(  # noqa: PLR0913
    repo_root: Path,
    *,
    chash: str,
    old_identity: str,
    new_identity: str,
    old_rel: str,
    new_rel: str,
) -> Path:
    """Append one provenance line; create the ledger if absent. Returns its path."""
    ledger = repo_root / RELOCATE_LEDGER_NAME
    ts = datetime.now(UTC).isoformat(timespec="seconds")
    line = "\t".join([ts, chash, old_identity, new_identity, old_rel, new_rel]) + "\n"
    with ledger.open("a", encoding="utf-8") as fh:
        fh.write(line)
    return ledger


# ---- git helpers (filesystem-only tool, but reads HEAD for safety checks) --


def _git_show(repo_root: Path, ref: str) -> str | None:
    """Content of ``ref`` (e.g. ``HEAD:path``), or None if absent / no git."""
    try:
        proc = subprocess.run(
            ["git", "-C", str(repo_root), "show", ref],
            capture_output=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if proc.returncode != 0:
        return None
    return proc.stdout.decode("utf-8", errors="replace")


# ---- reference scanning + rewriting ---------------------------------------


def _wikilink_re(slug: str) -> re.Pattern[str]:
    """Match a body ``[[slug]]`` wikilink, allowing ``#anchor`` / ``|alias``.

    Two capture groups bracket the slug so a rewrite swaps ONLY the slug token,
    leaving any anchor/alias intact.
    """
    return re.compile(
        r"(\[\[\s*)" + re.escape(slug) + r"(\s*(?:#[^\]|]*)?(?:\|[^\]]*)?\]\])"
    )


def _frontmatter_link_slugs(fm: dict | None) -> list[str]:
    links = fm.get("links") if fm else None
    if not isinstance(links, list):
        return []
    return [str(x).strip() for x in links if str(x).strip()]


def scan_references(
    slug: str, *, registry: Path | None = None, exclude: Path | None = None
) -> list[dict]:
    """Every note across registered KB sources that references ``slug``.

    A reference is a frontmatter ``links:`` entry equal to the slug OR a body
    ``[[slug]]`` wikilink. Each result: {path, repo, frontmatter, wikilink,
    frozen}. The ``exclude`` path (the note being moved) is skipped.
    """
    exclude_resolved = exclude.resolve() if exclude else None
    pat = _wikilink_re(slug)
    refs: list[dict] = []
    try:
        srcs = sources.load_sources(registry)
    except sources.SourcesError:
        return refs
    for name, path in srcs:
        if not path.is_dir():
            continue
        for p in path.rglob("*.md"):
            if _SKIP_DIRS & set(p.parts):
                continue
            if exclude_resolved is not None and p.resolve() == exclude_resolved:
                continue
            try:
                text = p.read_text(encoding="utf-8")
            except OSError:
                continue
            try:
                fm = contract.parse_frontmatter(text)
            except contract.ContractError:
                fm = None
            fm_ref = slug in _frontmatter_link_slugs(fm)
            body_ref = bool(pat.search(text))
            if fm_ref or body_ref:
                refs.append(
                    {
                        "path": p,
                        "repo": name,
                        "frontmatter": fm_ref,
                        "wikilink": body_ref,
                        "frozen": contract.is_frozen_fm(fm),
                    }
                )
    return refs


def _replace_links_field(block: str, old_slug: str, new_slug: str) -> tuple[str, int]:
    """Rewrite ``old_slug``→``new_slug`` inside the top-level ``links:`` field of
    a frontmatter block, preserving every other line verbatim. Returns
    (new_block, n_replaced).

    Mirrors contract.strip_fields' multi-line span detection (spanning flow
    list / block list) so the whole links value is replaced as a unit; the
    fresh value is rendered as a single-line flow list.
    """
    raw = block.splitlines(keepends=True)
    out: list[str] = []
    n = 0
    i, total = 0, len(raw)
    while i < total:
        line = raw[i]
        bare = line.rstrip("\n")
        i += 1
        m = contract._FM_KEY_RE.match(bare)
        if not m or bare[:1] in (" ", "\t") or m.group(1) != "links":
            out.append(line)
            continue
        rest = contract._strip_comment(m.group(2).strip())
        # consume this key's continuation lines (flow list / block list)
        cont: list[str] = []
        if rest.startswith("[") and not rest.rstrip().endswith("]"):
            while i < total and not contract._strip_comment(
                raw[i].rstrip("\n").strip()
            ).rstrip().endswith("]"):
                cont.append(raw[i])
                i += 1
            if i < total:
                cont.append(raw[i])
                i += 1
        elif rest == "":
            while (
                i < total
                and raw[i][:1] in (" ", "\t")
                and raw[i].strip().startswith("- ")
            ):
                cont.append(raw[i])
                i += 1
        # parse the existing items, swap, re-render as one flow line
        fm = contract.parse_frontmatter("---\n" + line + "".join(cont) + "---\n")
        slugs = _frontmatter_link_slugs(fm)
        if old_slug not in slugs:
            out.append(line)
            out.extend(cont)
            continue
        new_slugs = [new_slug if s == old_slug else s for s in slugs]
        n += slugs.count(old_slug)
        out.append("links: " + contract._flow_list(new_slugs) + "\n")
    return "".join(out), n


def rewrite_references(text: str, old_slug: str, new_slug: str) -> tuple[str, int]:
    """Rewrite a referrer's frontmatter ``links:`` + body ``[[slug]]`` from
    ``old_slug`` to ``new_slug``. Returns (new_text, n_replacements)."""
    split = contract.split_frontmatter(text)
    n = 0
    if split is None:
        block, body, had_fm = "", text, False
    else:
        block, body = split
        had_fm = True
        block, fm_n = _replace_links_field(block, old_slug, new_slug)
        n += fm_n
    new_body, body_n = _wikilink_re(old_slug).subn(
        lambda m: m.group(1) + new_slug + m.group(2), body
    )
    n += body_n
    if n == 0:
        return text, 0
    if had_fm:
        return "---\n" + block + "---\n" + new_body, n
    return new_body, n


# ---- target parsing + planning --------------------------------------------


def parse_target(spec: str) -> tuple[str, str, str | None]:
    """``<repo>:<domain>`` or ``<repo>:<domain>:<slug>`` → (repo, domain, slug?).

    Domain may contain ``/`` (a node-chain path); neither repo, domain segment,
    nor slug contains ``:``, so a plain split is unambiguous.
    """
    parts = spec.split(":")
    target_parts_without_slug = 2
    target_parts_with_slug = 3
    if len(parts) == target_parts_without_slug:
        repo, domain = parts
        slug: str | None = None
    elif len(parts) == target_parts_with_slug:
        repo, domain, slug = parts
    else:
        raise RelocateUsageError(
            f"--to {spec!r} must be <repo>:<domain> or <repo>:<domain>:<slug>"
        )
    repo = repo.strip()
    domain = domain.strip().strip("/")
    if not repo or not domain:
        raise RelocateUsageError(f"--to {spec!r}: empty repo or domain")
    if slug is not None:
        slug = contract.validate_topic(slug.strip())  # raises ContractError
    return repo, domain, slug


def _resolve_repo(repo_token: str, registry: Path | None) -> tuple[str, Path]:
    """Map a target repo token to (name, repo_root) via the KB-source registry."""
    for name, path in sources.load_sources(registry):
        if name == repo_token:
            if not path.is_dir():
                raise RelocateError(
                    f"target repo {repo_token!r} registered but missing at {path}"
                )
            return name, path.resolve()
    raise RelocateError(
        f"target repo {repo_token!r} is not a registered KB source "
        "(rhizome adopt it first)"
    )


@dataclass
class RelocatePlan:
    source_path: Path
    source_repo: str
    source_repo_root: Path
    old_rel: str
    old_identity: str
    target_repo: str
    target_repo_root: Path
    target_domain: str
    target_domain_dir: Path
    new_slug: str
    dest_path: Path
    new_rel: str
    new_identity: str
    frozen: bool
    content: str
    chash: str
    slug_changed: bool
    cross_repo: bool
    references: list[dict] = field(default_factory=list)
    rewrites: list[dict] = field(default_factory=list)
    cross_repo_refs: list[dict] = field(default_factory=list)
    frozen_blocked_refs: list[dict] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)

    def to_dict(self) -> dict:
        def _ref(r: dict) -> dict:
            return {
                "path": str(r["path"]),
                "repo": r["repo"],
                "frontmatter": r["frontmatter"],
                "wikilink": r["wikilink"],
                "frozen": r["frozen"],
            }

        return {
            "source": str(self.source_path),
            "old_identity": self.old_identity,
            "new_identity": self.new_identity,
            "dest": str(self.dest_path),
            "old_rel": self.old_rel,
            "new_rel": self.new_rel,
            "frozen": self.frozen,
            "content_hash": self.chash,
            "slug_changed": self.slug_changed,
            "cross_repo": self.cross_repo,
            "references": [_ref(r) for r in self.references],
            "rewrites": [_ref(r) for r in self.rewrites],
            "cross_repo_refs": [_ref(r) for r in self.cross_repo_refs],
            "frozen_blocked_refs": [_ref(r) for r in self.frozen_blocked_refs],
            "warnings": list(self.warnings),
        }


def plan_relocate(  # noqa: C901, PLR0912, PLR0915
    source: str, to: str, *, cwd: Path, registry: Path | None = None
) -> RelocatePlan:
    """Build a RelocatePlan for one note. Pure (no disk writes). Raises
    RelocateUsageError (exit 2) / RelocateError (exit 1) / ContractError."""
    src = Path(source).expanduser()
    if not src.is_absolute():
        src = cwd / src
    src = src.resolve()
    if not src.is_file():
        raise RelocateUsageError(f"no such note: {src}")

    src_repo_root = contract.find_repo_root(src.parent)
    if src_repo_root is None:
        raise RelocateUsageError(f"source not inside a git repo: {src}")
    src_repo_root = src_repo_root.resolve()
    if not contract.is_note_location(src):
        raise RelocateError(
            f"{src} is not inside a KB domain (no INDEX.md ancestor) — not a §存 note"
        )

    content = src.read_text(encoding="utf-8")
    try:
        fm = contract.parse_frontmatter(content)
    except contract.ContractError as exc:
        raise RelocateError(f"{src}: malformed frontmatter — {exc}") from exc
    frozen = contract.is_frozen_fm(fm)

    src_repo = contract.repo_name(src_repo_root)
    old_slug = src.stem
    src_domain_dir = contract.find_domain_dir(src.parent, src_repo_root)
    if src_domain_dir is None:  # defensive; is_note_location already guaranteed it
        raise RelocateError(f"{src}: no domain (INDEX.md) ancestor")
    src_domain = contract.derive_node_chain_domain(src_domain_dir, src_repo_root)
    old_identity = contract.derive_identity(src_repo, src_domain, old_slug)
    old_rel = src.relative_to(src_repo_root).as_posix()

    # A HEAD-committed frozen note must be byte-identical to HEAD: relocate is
    # content-preserving, so an uncommitted edit is a smuggled content change.
    chash = content_hash(content)
    if frozen:
        head = _git_show(src_repo_root, f"HEAD:{old_rel}")
        if head is not None and content_hash(head) != chash:
            raise RelocateError(
                f"{old_rel} is a frozen doc with uncommitted edits — relocate is "
                "byte-for-byte; commit or revert the edit first (or `rhizome amend`)"
            )

    repo_token, domain_token, slug_override = parse_target(to)
    tgt_repo, tgt_repo_root = _resolve_repo(repo_token, registry)

    tgt_domain_dir = (tgt_repo_root / domain_token).resolve()
    if not (tgt_domain_dir == tgt_repo_root or tgt_repo_root in tgt_domain_dir.parents):
        raise RelocateError(f"--to domain {domain_token!r} escapes repo {tgt_repo!r}")
    if not tgt_domain_dir.is_dir():
        raise RelocateError(
            f"--to domain {domain_token!r}: no such directory in {tgt_repo!r} "
            "(create it + an INDEX.md, or `rhizome adopt`)"
        )
    if not contract.has_index(tgt_domain_dir):
        raise RelocateError(
            f"--to domain {domain_token!r}: no {contract.INDEX_FILENAME} at "
            f"{tgt_domain_dir} (a domain is a directory with its own "
            f"{contract.INDEX_FILENAME})"
        )
    tgt_domain = contract.derive_node_chain_domain(tgt_domain_dir, tgt_repo_root)
    if not tgt_domain:
        raise RelocateError(
            f"--to domain {domain_token!r} resolves to the repo root, which is not a domain"
        )

    new_slug = slug_override or old_slug
    dest = tgt_domain_dir / f"{new_slug}.md"
    if dest.resolve() == src:
        raise RelocateError("source and target are the same note — nothing to relocate")
    if dest.exists():
        raise RelocateError(
            f"target already exists: {dest} (rhizome does not overwrite; pick a new slug)"
        )
    new_rel = dest.relative_to(tgt_repo_root).as_posix()
    new_identity = contract.derive_identity(tgt_repo, tgt_domain, new_slug)
    cross_repo = tgt_repo_root != src_repo_root
    slug_changed = new_slug != old_slug

    references = scan_references(old_slug, registry=registry, exclude=src)

    rewrites: list[dict] = []
    frozen_blocked: list[dict] = []
    cross_repo_refs: list[dict] = []
    warnings: list[str] = []

    if slug_changed:
        for r in references:
            if r["frozen"]:
                frozen_blocked.append(r)
            else:
                rewrites.append(r)
        if frozen_blocked:
            warnings.append(
                f"{len(frozen_blocked)} frozen referrer(s) cannot be auto-rewritten "
                f"for the slug change {old_slug!r}→{new_slug!r} (would edit frozen "
                "content); fix via `rhizome amend` or supersede"
            )
    elif cross_repo:
        # Slug preserved: links stay valid by slug, but a same-repo reference
        # becomes a cross-repo reference → links-checker WARN (not a break).
        cross_repo_refs = [r for r in references if r["repo"] == src_repo]
        if cross_repo_refs:
            warnings.append(
                f"{len(cross_repo_refs)} reference(s) in {src_repo!r} will become "
                "cross-repo (links-checker WARN, not broken — links are slug-based)"
            )

    return RelocatePlan(
        source_path=src,
        source_repo=src_repo,
        source_repo_root=src_repo_root,
        old_rel=old_rel,
        old_identity=old_identity,
        target_repo=tgt_repo,
        target_repo_root=tgt_repo_root,
        target_domain=tgt_domain,
        target_domain_dir=tgt_domain_dir,
        new_slug=new_slug,
        dest_path=dest,
        new_rel=new_rel,
        new_identity=new_identity,
        frozen=frozen,
        content=content,
        chash=chash,
        slug_changed=slug_changed,
        cross_repo=cross_repo,
        references=references,
        rewrites=rewrites,
        cross_repo_refs=cross_repo_refs,
        frozen_blocked_refs=frozen_blocked,
        warnings=warnings,
    )


def apply_relocate(plan: RelocatePlan) -> dict:
    """Execute a planned relocate: move the file, rewrite referrers (slug change
    only), append provenance ledgers. Returns an applied-summary dict."""
    # 1. Move bytes verbatim (content-preserving; the note's own slug-based,
    #    position-independent links never need editing).
    plan.dest_path.write_text(plan.content, encoding="utf-8")
    plan.source_path.unlink()

    # 2. Rewrite non-frozen referrers ONLY when the slug actually changed.
    rewritten: list[str] = []
    if plan.slug_changed:
        for r in plan.rewrites:
            p: Path = r["path"]
            try:
                text = p.read_text(encoding="utf-8")
            except OSError:
                continue
            new_text, n = rewrite_references(text, plan.source_path.stem, plan.new_slug)
            if n:
                p.write_text(new_text, encoding="utf-8")
                rewritten.append(str(p))

    # 3. Provenance: a ledger line in the source repo (the frozen gate's D side)
    #    and, when cross-repo, an audit line in the target repo too.
    ledgers: list[str] = []
    src_ledger = append_ledger(
        plan.source_repo_root,
        chash=plan.chash,
        old_identity=plan.old_identity,
        new_identity=plan.new_identity,
        old_rel=plan.old_rel,
        new_rel=plan.new_rel,
    )
    ledgers.append(str(src_ledger))
    if plan.cross_repo:
        tgt_ledger = append_ledger(
            plan.target_repo_root,
            chash=plan.chash,
            old_identity=plan.old_identity,
            new_identity=plan.new_identity,
            old_rel=plan.old_rel,
            new_rel=plan.new_rel,
        )
        ledgers.append(str(tgt_ledger))

    return {
        "moved": {"from": str(plan.source_path), "to": str(plan.dest_path)},
        "old_identity": plan.old_identity,
        "new_identity": plan.new_identity,
        "rewritten": rewritten,
        "ledgers": ledgers,
        "frozen": plan.frozen,
    }


# ---- batch ----------------------------------------------------------------


def load_batch(plan_path: Path) -> list[dict]:
    """Parse a batch TOML plan: ``[[move]] source=.. to=..`` rows."""
    try:
        data = tomllib.loads(plan_path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise RelocateUsageError(f"{plan_path}: {exc}") from exc
    moves = data.get("move")
    if not isinstance(moves, list) or not moves:
        raise RelocateUsageError(
            f"{plan_path}: expected at least one [[move]] table with source/to"
        )
    out: list[dict] = []
    for i, m in enumerate(moves):
        if not isinstance(m, dict) or not m.get("source") or not m.get("to"):
            raise RelocateUsageError(
                f"{plan_path}: [[move]] #{i + 1} needs both 'source' and 'to'"
            )
        out.append({"source": str(m["source"]), "to": str(m["to"])})
    return out


def run_relocate(  # noqa: PLR0913
    *,
    source: str | None = None,
    to: str | None = None,
    batch: str | None = None,
    apply: bool = False,
    cwd: Path,
    registry: Path | None = None,
) -> dict:
    """Plan (and optionally apply) one note or a batch. Returns a result dict
    {"apply": bool, "moves": [{"plan": {...}, "applied": {...}?}]}."""
    if batch:
        if source or to:
            raise RelocateUsageError("--batch is mutually exclusive with source/--to")
        specs = load_batch(Path(batch).expanduser())
    else:
        if not source or not to:
            raise RelocateUsageError("give a source note and --to, or --batch <plan>")
        specs = [{"source": source, "to": to}]

    # Two-phase for batch atomicity: plan + validate ALL moves first, so a bad
    # move (missing target domain, collision, frozen with uncommitted edit)
    # aborts the whole batch before any file is touched. Apply only after every
    # plan succeeds.
    plans = [
        plan_relocate(spec["source"], spec["to"], cwd=cwd, registry=registry)
        for spec in specs
    ]
    moves: list[dict] = []
    for plan in plans:
        entry: dict = {"plan": plan.to_dict()}
        if apply:
            entry["applied"] = apply_relocate(plan)
        moves.append(entry)
    return {"apply": apply, "moves": moves}
