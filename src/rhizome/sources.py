"""KB source registry + domain-tree self-discovery.

One hand-maintained list of source repos (`kb-sources.toml`); domains are
self-discovered by walking INDEX.md files. This is the single source feeding
three consumers: the indexer (what to scan), the session-start surface-hook
(show the domain tree), and `recall --domains`.

Pure-Python, stdlib only (tomllib); no compiled extensions.
"""

from __future__ import annotations

import json
import os
import tomllib
import urllib.request
from pathlib import Path

from . import contract

REGISTRY_FILENAME = "kb-sources.toml"
_DEFAULT_WORKSPACE_ROOT = "~/workspace"
# Directories never treated as KB content (no domain lives here).
_SKIP_DIRS = {".git", ".venv", "__pycache__", "node_modules", "dist"}
# The central Qdrant collection for the completeness diff (`rhizome domains --diff`).
_CENTRAL_URL_ENV = "KB_QDRANT_URL"
_DEFAULT_CENTRAL_URL = "<embedding-endpoint>"
_CENTRAL_COLLECTION_ENV = "KB_CENTRAL_COLLECTION"
_DEFAULT_CENTRAL_COLLECTION = "kb_central"


class SourcesError(Exception):
    """Registry missing/unreadable or a source repo cannot be located."""


# ---- registry -------------------------------------------------------------


def find_registry(start: Path | None = None) -> Path:
    """Locate kb-sources.toml.

    Search order:
    1. $KB_SOURCES env var — points directly at the registry file.
    2. Walk up from cwd (or ``start``) looking for kb-sources.toml.
    3. $KB_WORKSPACE_ROOT/kb-sources.toml (workspace-root fallback).
    4. ~/.config/rhizome/sources.toml (user-config fallback).
    """
    env = os.environ.get("KB_SOURCES")
    if env:
        p = Path(env).expanduser()
        if not p.is_file():
            raise SourcesError(f"$KB_SOURCES points at a missing file: {p}")
        return p
    cur = (start or Path.cwd()).resolve()
    for d in (cur, *cur.parents):
        cand = d / REGISTRY_FILENAME
        if cand.is_file():
            return cand
    ws_cand = _workspace_root() / REGISTRY_FILENAME
    if ws_cand.is_file():
        return ws_cand
    user_cand = Path("~/.config/rhizome/sources.toml").expanduser()
    if user_cand.is_file():
        return user_cand
    raise SourcesError(
        f"no {REGISTRY_FILENAME} found "
        f"($KB_SOURCES / cwd ancestors / $KB_WORKSPACE_ROOT / ~/.config/rhizome/sources.toml)"
    )


def _workspace_root() -> Path:
    return Path(
        os.environ.get("KB_WORKSPACE_ROOT", _DEFAULT_WORKSPACE_ROOT)
    ).expanduser()


def load_sources(registry: Path | None = None) -> list[tuple[str, Path]]:
    """Return [(name, repo_path), ...]; KB_WORKSPACE_ROOT overrides the base."""
    reg = registry or find_registry()
    if not reg.is_file():
        raise SourcesError(f"registry not found: {reg}")
    try:
        data = tomllib.loads(reg.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise SourcesError(f"{reg}: {exc}") from exc
    base = Path(
        os.environ.get(
            "KB_WORKSPACE_ROOT", data.get("workspace_root", _DEFAULT_WORKSPACE_ROOT)
        )
    ).expanduser()
    out, seen = [], set()
    for entry in data.get("source", []):
        name = entry.get("name")
        if not name:
            continue
        if "/" in name or "\\" in name or ".." in name or name.startswith("~"):
            raise SourcesError(
                f"invalid source name {name!r} (no /, \\, .., leading ~)"
            )
        if name in seen:
            raise SourcesError(f"duplicate source name {name!r}")
        seen.add(name)
        path = Path(entry["path"]).expanduser() if entry.get("path") else base / name
        out.append((name, path))
    if not out:
        raise SourcesError(f"{reg}: no [[source]] entries")
    return out


# ---- domain discovery -----------------------------------------------------


def _iter_index_files(repo: Path):
    # os.walk hands back REAL directory-entry names, so the exact comparison
    # holds on case-insensitive filesystems (macOS APFS). rglob is unusable
    # here: on Python 3.13+ a literal (wildcard-free) pattern is resolved by
    # existence check, so rglob("INDEX.md") matches index.md AND reports it
    # under the pattern's case — producing host-divergent domain trees.
    for dirpath, dirnames, filenames in os.walk(repo):
        dirnames[:] = sorted(d for d in dirnames if d not in _SKIP_DIRS)
        if contract.INDEX_FILENAME in filenames:
            yield Path(dirpath) / contract.INDEX_FILENAME


def discover_domains(repo: Path) -> list[dict]:
    """Domains in one repo = directories holding an INDEX.md (D4). Repo-root
    INDEX.md is not a domain (repo root). Each: {domain, index_path, description}."""
    repo = repo.resolve()
    domains = []
    for idx in _iter_index_files(repo):
        domain = contract.derive_node_chain_domain(
            idx.parent, repo
        )  # C2 node-chain口径
        if not domain:
            continue  # repo-root INDEX.md is not a domain
        desc = None
        try:
            fm = contract.parse_frontmatter(
                idx.read_text(encoding="utf-8", errors="replace")
            )
            if fm:
                desc = fm.get("description")
        except contract.ContractError:
            pass
        domains.append({"domain": domain, "index_path": str(idx), "description": desc})
    domains.sort(key=lambda d: d["domain"])
    return domains


def central_note_index(fetch=None) -> dict[str, dict[str, str]]:
    """One scroll over the central collection → {repo: {identity: source_path}}.

    Distinct identities so chunked notes are not double-counted; identity is
    `repo:domain:slug`, so the repo key matches the registry source name.
    The payload `domain` is deliberately NOT used: diff() recomputes the domain
    from source_path against the LOCAL INDEX tree, so the coverage comparison
    stays self-consistent even when the central payload lags local INDEX changes.
    Raises SourcesError when Qdrant is unreachable — the diff must say so loudly,
    never silently report "not-indexed".
    """
    fetch = fetch or _qdrant_scroll_page
    out: dict[str, dict[str, str]] = {}
    offset = None
    while True:
        page = fetch(offset)
        for p in page.get("points") or []:
            payload = p.get("payload") if isinstance(p, dict) else None
            if not isinstance(payload, dict):
                continue
            identity = payload.get("identity")
            if not isinstance(identity, str) or ":" not in identity:
                continue  # malformed point — skip; sync-side validation owns it
            repo = identity.split(":", 1)[0]
            sp = payload.get("source_path")
            out.setdefault(repo, {})[identity] = sp if isinstance(sp, str) else ""
        offset = page.get("next_page_offset")
        if offset is None:
            break
    return out


def _qdrant_scroll_page(offset) -> dict:
    """POST one /points/scroll page (stdlib urllib only,."""
    url = os.environ.get(_CENTRAL_URL_ENV, _DEFAULT_CENTRAL_URL).rstrip("/")
    collection = os.environ.get(_CENTRAL_COLLECTION_ENV, _DEFAULT_CENTRAL_COLLECTION)
    body: dict = {"limit": 512, "with_payload": ["identity", "source_path"]}
    if offset is not None:
        body["offset"] = offset
    req = urllib.request.Request(
        f"{url}/collections/{collection}/points/scroll",
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=5) as resp:
            data = json.loads(resp.read().decode("utf-8"))
    except (OSError, ValueError) as exc:  # URLError subclasses OSError
        raise SourcesError(
            f"central index unreachable ({url}, collection {collection!r}): {exc}"
        ) from exc
    result = data.get("result") if isinstance(data, dict) else None
    return result if isinstance(result, dict) else {}


def note_domain(note_path: Path, repo: Path) -> str | None:
    """The domain a note belongs to = C2 node chain of its nearest ancestor
    INDEX.md dir , or None.

    Position-derived, so it works for old and new frontmatter alike (新旧并存).
    Returns None for notes outside the repo or under the repo root only."""
    repo = repo.resolve()
    note_path = note_path.resolve()
    if note_path != repo and repo not in note_path.parents:
        return (
            None  # escaped the repo → no domain (and don't read stray INDEX.md above)
        )
    domain_dir = contract.find_domain_dir(note_path.parent, repo)
    if domain_dir is None:
        return None
    try:
        return (
            contract.derive_node_chain_domain(domain_dir, repo) or None
        )  # C2口径 ; "" = root
    except ValueError:
        return None


# ---- the tree + completeness diff -----------------------------------------


def build_tree(registry: Path | None = None) -> list[dict]:
    """[{name, path, exists, domains:[...]}] — feeds surface-hook & recall."""
    tree = []
    for name, path in load_sources(registry):
        node = {"name": name, "path": str(path), "exists": path.is_dir(), "domains": []}
        if node["exists"]:
            node["domains"] = discover_domains(path)
        tree.append(node)
    return tree


def diff(
    registry: Path | None = None, central: dict[str, dict[str, str]] | None = None
) -> list[dict]:
    """Completeness: discovered domains vs actually-indexed in the central
    collection.

    Per repo reports: missing repo, 0 domains, not-yet-indexed (absent from the
    central collection), empty domains (INDEX.md but no indexed notes), and
    orphan notes (indexed, no domain). Raises SourcesError if the central
    collection is unreachable (loud — never silently lie)."""
    if central is None:
        central = central_note_index()
    report = []
    for name, path in load_sources(registry):
        row: dict = {"name": name, "path": str(path)}
        if not path.is_dir():
            row["status"] = "missing-repo"
            report.append(row)
            continue
        discovered = {d["domain"] for d in discover_domains(path)}
        row["domains_discovered"] = sorted(discovered)
        if not discovered:
            row["status"] = "no-domains"  # repo has no INDEX.md (e.g. ops/pm)
            report.append(row)
            continue
        notes = central.get(name)
        if notes is None:
            row["status"] = "not-indexed"  # absent from the central collection
            report.append(row)
            continue
        # Physical-path口径, recomputed locally from source_path (matches
        # discover_domains; the central payload domain is C2口径,.
        idx: dict[str, int] = {}
        for sp in notes.values():
            dom = note_domain(path / sp, path) if sp else None
            key = dom if dom is not None else "(no domain)"
            idx[key] = idx.get(key, 0) + 1
        indexed_set = {k for k in idx if k != "(no domain)"}
        row["status"] = "ok"

        # A domain is covered if any indexed note resolves to it OR to a
        # path-segment descendant (domain scope is a prefix filter).
        # `d + "/"` keeps it segment-aware: `foo` ≠ `foo-private`.
        def _covered(d: str, indexed_set: set[str] = indexed_set) -> bool:
            return any(k == d or k.startswith(d + "/") for k in indexed_set)

        row["domains_indexed"] = sum(1 for d in discovered if _covered(d))
        row["empty_domains"] = sorted(d for d in discovered if not _covered(d))
        row["orphan_notes"] = idx.get("(no domain)", 0)  # indexed but under no domain
        report.append(row)
    return report
