"""§存 frontmatter contract — single source of truth.

Defines the storage contract:

  description  required  one-line recall summary
  keywords     required  list; recall signal + surface trigger + --tag
  kind         optional  facet + ranking prior (default: note)
  links        optional  slug list → other notes
  code         optional  repo/path list → code / external raw

Decision-only:
  supersedes   replaces an older decision while retaining audit history
  assets       flat namespace-prefixed delivery/runtime asset identifiers

Derived, never written to frontmatter:
  domain    ← nearest ancestor INDEX.md directory
  identity  ← <repo>:<domain>:<slug>, slug = filename (no uuid)
  time      ← git

KILLED_FIELDS / DERIVED_FIELDS are fields that must NOT appear in a
note's frontmatter (used by tests and `rhizome check`).
"""

from __future__ import annotations

import os
import re
from pathlib import Path

# ---- the contract ---------------------------------------------------------

REQUIRED_FIELDS: tuple[str, ...] = ("description", "keywords")
OPTIONAL_FIELDS: tuple[str, ...] = ("kind", "links", "code")
ALLOWED_FIELDS: tuple[str, ...] = REQUIRED_FIELDS + OPTIONAL_FIELDS

# kind: 7 values.
#   spec      约束性文档 (契约/规范/operating-spec, "必须怎样")
#   reference explanation / 交接 / 复盘 / 耐用现状评估
#   runbook   how-to / 排障
#   decision  ADR / PRD (留痕积累)
#   research  文献综述 / 调研
#   note      默认 facet
#   index     INDEX.md domain-homepage note
# design-doc 不是 kind: 设计的耐用产物裂解为 ADR=decision + 约束面=spec + 讲解=reference.
KINDS: tuple[str, ...] = (
    "spec",
    "reference",
    "runbook",
    "decision",
    "research",
    "note",
    "index",
)
DEFAULT_KIND = "note"

INDEX_FILENAME = "INDEX.md"

# Legacy fields removed from the contract ("杀").
KILLED_FIELDS = frozenset(
    {
        "object_id",
        "object_key",
        "topic",
        "workset",
        "schema_version",
        "updated_at",
        "created_at",
        "authored_from",
        "retrieval_hint",
    }
)
# status is an optional SINGLE-VALUE field — "frozen" marks a read-only
# snapshot; absence = editable living doc. Any other value (draft/living/...)
# is a contract violation and gets stripped by `rhizome check --fix`.
# `kind: decision` is implicitly frozen without the field.
STATUS_FIELD = "status"
STATUS_ALLOWED = frozenset({"frozen"})
# supersedes is only for `kind: decision` (留痕); validator must treat it
# as decision-conditional, not flat-killed.
DECISION_ONLY_FIELDS = frozenset({"supersedes", "assets"})
ASSET_PREFIXES: tuple[str, ...] = (
    # Common delivery/runtime asset namespaces — neutral public defaults.
    "repo",
    "img",
    "data",
    "pipeline",
    "fe",
    "log",
    "svc",
)
# Project-specific asset namespaces (e.g. internal middleware names) are kept
# out of the public default set and injected at runtime via a comma-separated
# env var; the effective set is always default + env (additive, never replace).
ASSET_PREFIXES_ENV = "RHIZOME_ASSET_PREFIXES"


def _env_csv(name: str) -> list[str]:
    """Comma-separated env var → cleaned list (empty/whitespace items dropped).

    Unset env → []. Robust to extra spaces, trailing commas, blank items.
    """
    raw = os.environ.get(name)
    if not raw:
        return []
    return [part.strip() for part in raw.split(",") if part.strip()]


def known_asset_prefixes() -> frozenset[str]:
    """Effective canonical asset prefixes = public defaults + $RHIZOME_ASSET_PREFIXES.

    Env-supplied prefixes are lowercased to match asset_prefix() normalization.
    """
    return frozenset(ASSET_PREFIXES) | {p.lower() for p in _env_csv(ASSET_PREFIXES_ENV)}


ASSET_EXTERNALIZE_COUNT_THRESHOLD = 12
ASSET_EXTERNALIZE_REUSE_THRESHOLD = 3
# Reconstructed elsewhere, so also never hand-written into frontmatter:
# domain ← INDEX.md path, title ← H1/filename, identity ← path, time(verified) ← git.
DERIVED_FIELDS = frozenset({"domain", "title", "identity", "verified"})
FORBIDDEN_FIELDS = KILLED_FIELDS | DERIVED_FIELDS

# slug = filename = identity tail; keep it predictable (lowercase kebab).
_SLUG_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")

# Slugs only locate/distinguish; semantics belong in description/keywords and
# humans read the domain INDEX (kb-note-contract §3, org-contract D5-D7).
SLUG_WORDS_SHOULD_MAX = 6
_ADR_PREFIX_RE = re.compile(r"^adr-\d+-")


def slug_excess_words(topic: str) -> int:
    """Words beyond the SHOULD cap, ignoring an `adr-NNN-` prefix; 0 if fine."""
    rest = _ADR_PREFIX_RE.sub("", topic.strip())
    return max(0, len(rest.split("-")) - SLUG_WORDS_SHOULD_MAX)


class ContractError(ValueError):
    """A field or value violates the §存 contract."""


def is_frozen_fm(fm: dict | None) -> bool:
    """True if a parsed frontmatter marks a read-only snapshot .

    Frozen ⟺ explicit `status: frozen` OR `kind: decision` (born-frozen, the
    field is implied and not written).
    """
    if not fm:
        return False
    return fm.get(STATUS_FIELD) == "frozen" or fm.get("kind") == "decision"


# ---- validation -----------------------------------------------------------


def validate_topic(topic: str) -> str:
    topic = topic.strip()
    if not _SLUG_RE.match(topic):
        raise ContractError(
            f"topic {topic!r} is not a valid slug; use lowercase kebab-case "
            f"(e.g. blue-fallback), no spaces/slashes/uppercase"
        )
    return topic


def validate_description(description: str) -> str:
    description = description.strip()
    if not description:
        raise ContractError("description is required (one line)")
    if "\n" in description:
        raise ContractError("description must be a single line")
    return description


def validate_keywords(keywords: list[str]) -> list[str]:
    cleaned = [k.strip() for k in keywords if k.strip()]
    if not cleaned:
        raise ContractError("keywords is required (at least one)")
    return cleaned


def validate_kind(kind: str) -> str:
    kind = kind.strip()
    if kind not in KINDS:
        raise ContractError(f"kind {kind!r} not in {list(KINDS)}")
    return kind


def validate_assets(assets: list[str]) -> list[str]:
    """Normalize decision-only delivery/runtime asset identifiers.

    Unknown prefixes are intentionally not rejected here; `kb check` surfaces
    them as warnings so new asset families can be recorded before the canonical
    prefix set catches up.
    """
    return [a.strip() for a in assets if a.strip()]


def asset_prefix(asset: str) -> str | None:
    head, sep, _ = asset.partition(":")
    if not sep or not head:
        return None
    return head.strip().lower()


# ---- identity / domain derivation (D2 / D4) -------------------------------


def find_repo_root(start: Path) -> Path | None:
    """Nearest ancestor (inclusive) containing a .git entry.

    .git is a directory in a normal checkout and a file in a worktree, so
    .exists() covers both.
    """
    cur = start.resolve()
    while True:
        if (cur / ".git").exists():
            return cur
        if cur.parent == cur:
            return None
        cur = cur.parent


def repo_name(repo_root: Path) -> str:
    """Canonical repo name = basename of the *main* checkout.

    In a normal checkout that's repo_root.name. In a git worktree, .git is a
    file `gitdir: <main>/.git/worktrees/<wt>`; the canonical repo is <main>, not
    the worktree dir — otherwise a note authored in a worktree would get a
    different `<repo>:...` identity than the same path on main (D2 index key).
    """
    gitpath = repo_root / ".git"
    if gitpath.is_file():
        text = gitpath.read_text(encoding="utf-8").strip()
        if text.startswith("gitdir:"):
            gitdir = text[len("gitdir:") :].strip()
            marker = "/.git/worktrees/"
            idx = gitdir.find(marker)
            if idx != -1:
                return Path(gitdir[:idx]).name
    return repo_root.name


def has_index(dirpath: Path) -> bool:
    """Exact-name INDEX.md presence check — APFS-safe.

    `(dir / "INDEX.md").is_file()` (and a literal-pattern rglob on Python
    3.13+, which resolves wildcard-free components by existence check) matches
    case-insensitively on macOS APFS, so a stray `index.md` would silently
    become a domain on one host and not another. Comparing against the real
    directory listing pins the exact byte name.
    """
    try:
        return any(p.name == INDEX_FILENAME for p in dirpath.iterdir())
    except OSError:
        return False


def find_domain_dir(start: Path, repo_root: Path) -> Path | None:
    """Nearest ancestor (inclusive of `start`) with an INDEX.md, bounded by repo_root.

    Returns None if no INDEX.md exists from `start` up to and including repo_root.
    """
    cur = start.resolve()
    root = repo_root.resolve()
    while True:
        if has_index(cur):
            return cur
        if cur in (root, cur.parent):
            return None
        cur = cur.parent


def derive_domain(domain_dir: Path, repo_root: Path) -> str:
    """Physical repo-relative path of a domain dir — INTERNAL LOCATOR ONLY.

    The canonical domain口径 everywhere (identity, domains, recall filter) is
    derive_node_chain_domain(); this function only answers "where does the
    directory physically sit" and must not leak into any external semantics.
    Raises on the repo root (not a domain).
    """
    rel = domain_dir.resolve().relative_to(repo_root.resolve())
    domain = rel.as_posix()
    if domain in ("", "."):
        raise ContractError(
            "INDEX.md at repo root is not a domain; domains are subdirectories "
            "(see contracts/kb-source-repository-contract.md)"
        )
    return domain


def derive_identity(repo: str, domain: str, slug: str) -> str:
    """identity = <repo>:<domain>:<slug> (all position-derived, no uuid)."""
    return f"{repo}:{domain}:{slug}"


# ---- C2 node-chain domain derivation ----------------------------------------
#
# THE domain口径 is the C2 node chain: identity, kb domains, recall filter and
# the duplicate-domain guard all use it. derive_domain() above is DEMOTED to
# an internal physical locator (where the file physically sits, e.g. for
# landing a new note) and must not feed any external semantics. The two
# functions still differ whenever a domain dir sits below a non-domain physical
# dir. C2 chain uniqueness is enforced by `rhizome check --duplicate-domains`
# (commit gate); a collision is fixed by giving the disambiguating intermediate
# dir its own INDEX.md.


def derive_node_chain_domain(domain_dir: Path, repo_root: Path) -> str:
    """C2 domain path = chain of ancestor-dir basenames that hold an INDEX.md.

    Walks from the repo root down to (and including) ``domain_dir``; each segment
    whose directory has its own INDEX.md contributes its basename, others are
    skipped. The repo root never contributes (it is not a domain even with a
    root INDEX.md). Returns "" for the repo root itself (a root INDEX.md is not
    a domain). Assumes ``domain_dir`` is at or under ``repo_root``.
    """
    repo_root = repo_root.resolve()
    domain_dir = domain_dir.resolve()
    rel = domain_dir.relative_to(repo_root)  # raises ValueError if outside repo
    segments: list[str] = []
    cur = repo_root
    for part in rel.parts:  # repo root excluded; descend one physical segment at a time
        cur = cur / part
        if has_index(cur):
            segments.append(part)
    return "/".join(segments)


# ---- frontmatter rendering ------------------------------------------------


def _dquote(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


# A flow item is left bare ONLY if it is unambiguously a plain string under both
# the YAML 1.2 core schema and the looser YAML 1.1 (PyYAML/older parsers): it
# starts with a Unicode letter (CJK included) and contains only
# letters/digits/_/./-. Everything else — numbers, dates, leading indicators,
# flow-special chars, whitespace, reserved words — is double-quoted so it can
# never coerce to int/float/bool/null/timestamp or break flow parsing.
# (gopkg.in/yaml.v3 coerces bare `2026`→int, `007`→7, `true`→bool,
# `~`/`null`→nil; quoting forces the string.)
_SAFE_BARE_RE = re.compile(r"^[^\W\d_][\w./-]*$", re.UNICODE)
_RESERVED_WORDS = frozenset(
    {
        "true",
        "false",
        "yes",
        "no",
        "on",
        "off",
        "null",
        "none",
        "nan",
        "inf",
    }
)


def _flow_item(s: str) -> str:
    s = s.strip()
    if _SAFE_BARE_RE.match(s) and s.lower() not in _RESERVED_WORDS:
        return s
    return _dquote(s)


def _flow_list(items: list[str]) -> str:
    return "[" + ", ".join(_flow_item(x) for x in items) + "]"


def render_frontmatter(
    *,
    description: str,
    keywords: list[str],
    kind: str = DEFAULT_KIND,
    links: list[str] | None = None,
    code: list[str] | None = None,
    assets: list[str] | None = None,
) -> str:
    """Render a compliant frontmatter block (field order: description/keywords/kind/links/code/assets).

    Required (description/keywords/kind) always emitted; links/code only when
    non-empty. No killed/derived fields are ever written.
    """
    description = validate_description(description)
    keywords = validate_keywords(keywords)
    kind = validate_kind(kind)
    links = [x.strip() for x in (links or []) if x.strip()]
    code = [x.strip() for x in (code or []) if x.strip()]
    assets = validate_assets(assets or [])

    lines = [
        "---",
        f"description: {_dquote(description)}",
        f"keywords: {_flow_list(keywords)}",
        f"kind: {kind}",
    ]
    if links:
        lines.append(f"links: {_flow_list(links)}")
    if code:
        lines.append(f"code: {_flow_list(code)}")
    if assets:
        lines.append(f"assets: {_flow_list(assets)}")
    lines.append("---")
    return "\n".join(lines) + "\n"


def render_note(frontmatter: str, body: str) -> str:
    """Frontmatter block + blank line + body (single trailing newline).

    No H1 is injected; the body (piped by the author) owns its title, and
    title is derived at index time from the H1/filename, not the frontmatter.
    """
    return frontmatter + "\n" + body.strip("\n") + "\n"


# ---- frontmatter parsing --------------------------------------------------
# A deliberately small reader for the FLAT mapping a §存 note carries — not a
# general YAML engine (no anchors/nesting/multidoc). Pure-Python, no compiled
# extensions. Handles: flow lists `[a, b]`, block lists (`- x`), quoted/bare
# scalars, and null (`~`/`null`/empty). Good enough for `rhizome check` and
# downstream indexers; lenient cases are caught as index-time loud-skips.

_FM_KEY_RE = re.compile(r"^([A-Za-z0-9_-]+):(.*)$")


def split_frontmatter(text: str) -> tuple[str, str] | None:
    """Split leading `---\\n…\\n---` into (block_without_fences, body).

    Returns None if there is no opening fence. Raises ContractError if the
    fence opens but never closes (a malformed note, not a fenceless file).
    Tolerates a leading BOM and CRLF line endings (yaml.v3 accepts both).
    """
    text = text.lstrip("\ufeff")
    lines = text.splitlines(keepends=True)
    if not lines or lines[0].strip() != "---":
        return None
    for i in range(1, len(lines)):
        if lines[i].strip() == "---":
            return "".join(lines[1:i]), "".join(lines[i + 1 :])
    raise ContractError("unterminated frontmatter (opening --- with no closing ---)")


def _unquote(s: str) -> str:
    s = s.strip()
    # A quoted scalar needs at least an opening and a closing quote.
    min_quoted_len = 2
    if len(s) >= min_quoted_len and s[0] == s[-1] and s[0] in "\"'":
        inner = s[1:-1]
        if s[0] == '"':
            inner = inner.replace('\\"', '"').replace("\\\\", "\\")
        return inner
    return s


def _parse_scalar(s: str):
    s = s.strip()
    if s == "" or s in ("~", "null", "Null", "NULL"):
        return None
    return _unquote(s)


def _parse_flow_list(s: str) -> list:
    inner = s.strip()[1:-1].strip()  # drop surrounding [ ]
    if not inner:
        return []
    items, cur, quote = [], "", None
    for ch in inner:
        if quote:
            cur += ch
            if ch == quote:
                quote = None
        elif ch in "\"'":
            quote = ch
            cur += ch
        elif ch == ",":
            items.append(cur)
            cur = ""
        else:
            cur += ch
    if cur.strip():
        items.append(cur)
    return [_unquote(x) for x in items]


def _strip_comment(s: str) -> str:
    """Drop an unquoted trailing `# comment` (YAML: # at start or after space)."""
    out, quote = [], None
    for i, ch in enumerate(s):
        if quote:
            out.append(ch)
            if ch == quote:
                quote = None
        elif ch in "\"'":
            quote = ch
            out.append(ch)
        elif ch == "#" and (i == 0 or s[i - 1] in " \t"):
            break
        else:
            out.append(ch)
    return "".join(out).strip()


def parse_frontmatter(text: str) -> dict | None:
    """Parse a note's frontmatter into a flat dict, or None if no frontmatter.

    Raises ContractError on unterminated frontmatter. Handles flow lists
    (incl. spanning lines), block lists (`- x`), block scalars (`|`/`>`, folded
    to one string), quoted/bare scalars, null (`~`/`null`/empty), and trailing
    `# comments`. Unparseable top-level lines are skipped (lenient — index-time
    loud-skip is the backstop). Not a general YAML engine.
    """
    split = split_frontmatter(text)
    if split is None:
        return None
    block, _ = split
    out: dict = {}
    lines = block.splitlines()
    i, n = 0, len(lines)
    while i < n:
        line = lines[i]
        i += 1
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if line[:1] in (" ", "\t"):  # stray indented line, not a top-level key
            continue
        m = _FM_KEY_RE.match(line)
        if not m:
            continue
        key, rest = m.group(1), m.group(2).strip()

        if rest and rest[0] in "|>":  # block scalar → fold following indented lines
            buf = []
            while i < n and (not lines[i].strip() or lines[i][:1] in (" ", "\t")):
                buf.append(lines[i].strip())
                i += 1
            out[key] = " ".join(x for x in buf if x) or None
            continue

        rest = _strip_comment(rest)

        if rest.startswith("["):  # flow list, possibly spanning lines
            while not rest.rstrip().endswith("]") and i < n:
                rest += " " + _strip_comment(lines[i].strip())
                i += 1
            out[key] = (
                _parse_flow_list(rest)
                if rest.rstrip().endswith("]")
                else _parse_scalar(rest)
            )
        elif rest == "":  # block list, or empty/null scalar
            block_items = []
            while (
                i < n
                and lines[i][:1] in (" ", "\t")
                and lines[i].strip().startswith("- ")
            ):
                block_items.append(_parse_scalar(_strip_comment(lines[i].strip()[2:])))
                i += 1
            out[key] = block_items if block_items else None
        else:
            out[key] = _parse_scalar(rest)
    return out


def strip_fields(text: str, remove: frozenset[str]) -> tuple[str, list[str]]:
    """Remove the given top-level frontmatter keys (with their continuation
    lines) from a note, preserving every other line and the body VERBATIM.

    Returns ``(new_text, removed)`` where ``removed`` is the sorted list of keys
    that were present and stripped. No frontmatter → text returned unchanged
    with ``[]``. Mirrors parse_frontmatter's multi-line consumption (block
    scalars `|`/`>`, spanning flow lists `[...]`, block lists `- x`) so a
    removed key takes its continuation lines with it; comments, blank lines and
    kept fields are left byte-for-byte. Used by ``kb check --fix`` to drop the
    losslessly-reconstructible derived/killed fields (domain/title/verified/
    object_id/...); it never touches required/optional contract fields or body.
    """
    split = split_frontmatter(text)
    if split is None:
        return text, []
    block, body = split
    raw = block.splitlines(keepends=True)
    kept: list[str] = []
    removed: set[str] = set()
    i, n = 0, len(raw)
    while i < n:
        line = raw[i]
        bare = line.rstrip("\n")
        i += 1
        if not bare.strip() or bare.lstrip().startswith("#") or bare[:1] in (" ", "\t"):
            kept.append(line)  # blank / comment / stray-indented → not a top-level key
            continue
        m = _FM_KEY_RE.match(bare)
        if not m:
            kept.append(line)
            continue
        key, rest = m.group(1), m.group(2).strip()

        cont: list[str] = []  # this key's continuation lines
        if rest and rest[0] in "|>":  # block scalar
            while i < n and (not raw[i].strip() or raw[i][:1] in (" ", "\t")):
                cont.append(raw[i])
                i += 1
        else:
            rest_nc = _strip_comment(rest)
            if rest_nc.startswith("[") and not rest_nc.rstrip().endswith(
                "]"
            ):  # spanning flow list
                while i < n and not _strip_comment(
                    raw[i].rstrip("\n").strip()
                ).rstrip().endswith("]"):
                    cont.append(raw[i])
                    i += 1
                if i < n:  # the closing-] line
                    cont.append(raw[i])
                    i += 1
            elif rest_nc == "":  # block list items
                while (
                    i < n
                    and raw[i][:1] in (" ", "\t")
                    and raw[i].strip().startswith("- ")
                ):
                    cont.append(raw[i])
                    i += 1

        if key in remove:
            removed.add(key)
        else:
            kept.append(line)
            kept.extend(cont)

    return "---\n" + "".join(kept) + "---\n" + body, sorted(removed)


def is_note_location(path: Path) -> bool:
    """True if `path` sits inside a KB domain (an ancestor dir has an INDEX.md).

    Domain-aware gating: a frontmatter'd file OUTSIDE any domain is not a KB
    note and must not be checked or --fix'd — e.g. a PM `issues/*.md` (which
    legitimately carries id/title/status under its own PM schema) or a repo-root
    scratch doc (the root is never a domain). Lets the same rhizome-check hook
    run safely in repos that mix KB and non-KB Markdown.

    Not inside a git repo → True (no domain concept; fall back to checking).
    """
    path = path.resolve()
    repo_root = find_repo_root(path.parent)
    if repo_root is None:
        return True
    return find_domain_dir(path.parent, repo_root) is not None
