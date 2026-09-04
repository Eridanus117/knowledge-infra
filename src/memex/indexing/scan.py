"""扫源仓 + domain 派生(C2)+ identity(C3)—— 写路径核心。

域 = 放了 INDEX.md 的目录。域路径只由 INDEX 节点 basename 组成的链, 跳过
中间非域物理段(方案 A): `docs/source-notes/domain-map/widget-spec/use-cases/`
若仅 domain-map 与 use-cases 有 INDEX → 域路径 `domain-map/use-cases`。

repo 根若有 INDEX.md, 它是根域、贡献空段: 根域链从其直接子 INDEX 节点的
basename 开始。注: 上游写入工具把根 INDEX.md 当「非域」, 这与「根域贡献空段」
是同一外在效果(根 INDEX 不引入域段), 本模块按此口径实现。
"""

from __future__ import annotations

import unicodedata
from dataclasses import dataclass
from pathlib import Path

INDEX_FILENAME = "INDEX.md"
# 永不视为 KB 内容的目录(无域居于此)。
_SKIP_DIRS = frozenset(
    {".git", ".venv", ".legacy-index", "__pycache__", "node_modules", "dist"}
)


class ScanError(Exception):
    """域树派生硬错误(如 duplicate-domain 守卫触发)。"""


@dataclass(frozen=True)
class DomainNode:
    """一个 INDEX.md 节点定义的域。

    domain: 节点链 POSIX 路径(仅 INDEX 节点 basename, 跳过非域段)。根域 = ""。
    prefixes: 祖先链累进数组, 如 domain `a/b` → ["a", "a/b"];根域 → []。
    index_path: 该 INDEX.md 的绝对路径。
    dir: 该域目录的绝对路径。
    """

    domain: str
    prefixes: tuple[str, ...]
    index_path: Path
    dir: Path


def _iter_index_files(repo_root: Path):
    """递归找 INDEX.md(排除 skip 目录与隐藏目录)。

    p.name == INDEX_FILENAME 兜住大小写不敏感 FS(macOS APFS): rglob 也会匹到
    index.md, 显式过滤防 host 间域树分叉(对齐 kb 工具)。
    """
    for p in repo_root.rglob(INDEX_FILENAME):
        if p.name != INDEX_FILENAME:
            continue
        rel_parts = p.relative_to(repo_root).parts
        # 跳过 skip 目录, 以及任何隐藏目录段(. 开头, 但根自身的 "" 不算)。
        if any(part in _SKIP_DIRS or part.startswith(".") for part in rel_parts[:-1]):
            continue
        yield p


def _node_chain(
    index_dir: Path, repo_root: Path, index_dirs: set[Path]
) -> tuple[str, ...]:
    """从 repo 根到 index_dir(含)的祖先链中, 仅取 INDEX 节点目录的 basename。

    repo 根 INDEX(若有)贡献空段(不进链);其余非域物理段跳过。返回 basename 元组,
    根域目录(= repo_root)→ ()。
    """
    chain: list[str] = []
    cur = index_dir.resolve()
    root = repo_root.resolve()
    while cur != root:
        if cur in index_dirs:
            chain.append(cur.name)
        if cur.parent == cur:
            break
        cur = cur.parent
    chain.reverse()
    return tuple(chain)


def discover_domains(repo_root: Path) -> list[DomainNode]:
    """发现一个仓的全部域节点(C2)。duplicate-domain → loud ScanError 点名两处。"""
    repo_root = repo_root.resolve()
    index_paths = sorted(_iter_index_files(repo_root))
    index_dirs = {p.parent.resolve() for p in index_paths}

    by_domain: dict[str, DomainNode] = {}
    for idx in index_paths:
        idx_dir = idx.parent.resolve()
        chain = _node_chain(idx_dir, repo_root, index_dirs)
        domain = "/".join(chain)
        prefixes = tuple("/".join(chain[: i + 1]) for i in range(len(chain)))
        node = DomainNode(domain=domain, prefixes=prefixes, index_path=idx, dir=idx_dir)
        if domain in by_domain:
            other = by_domain[domain]
            raise ScanError(
                f"duplicate domain {domain!r} in {repo_root.name}: "
                f"{other.index_path} 与 {idx} 派生出同一域路径(C2 守卫拒绝索引)"
            )
        by_domain[domain] = node
    return sorted(by_domain.values(), key=lambda d: d.domain)


def _nearest_domain(
    note_path: Path, repo_root: Path, nodes: list[DomainNode]
) -> DomainNode | None:
    """note 的域 = 最近祖先 INDEX 节点(C2)。无则 None(不在任何域子树内)。"""
    note_path = note_path.resolve()
    root = repo_root.resolve()
    if note_path != root and root not in note_path.parents:
        return None
    by_dir = {n.dir: n for n in nodes}
    cur = note_path.parent.resolve()
    while True:
        if cur in by_dir:
            return by_dir[cur]
        if cur in (root, cur.parent):
            return None
        cur = cur.parent


def _slug(note_path: Path, domain_dir: Path) -> str:
    """slug = note 相对其域目录的路径去 .md(可含 /, C3)。"""
    rel = note_path.resolve().relative_to(domain_dir.resolve())
    posix = rel.as_posix()
    if posix.endswith(".md"):
        posix = posix[: -len(".md")]
    return posix


def derive_identity(repo: str, domain: str, slug: str) -> str:
    """identity = <repo>:<domain>:<slug>(C3, 始终位置派生)。"""
    return f"{repo}:{domain}:{slug}"


def _fold(identity: str) -> str:
    """大小写折叠 + NFC 归一化(C3 重复检测: macOS FS 不敏感)。"""
    return unicodedata.normalize("NFC", identity).casefold()


@dataclass(frozen=True)
class ScannedNote:
    """扫描到的一个 .md(可索引性闸门尚未应用): 路径 + 域 + identity。"""

    path: Path  # 绝对路径
    source_path: str  # repo 相对 POSIX
    node: DomainNode
    slug: str
    identity: str
    is_index: bool  # 是否为 INDEX.md 自身(域首页)


def repo_name(repo_root: Path) -> str:
    """规范仓名 = 主 checkout 的 basename。

    worktree 下 .git 是文件 `gitdir: <main>/.git/worktrees/<wt>`, 规范仓 = <main>,
    否则 worktree 里的同一路径 note 会拿到不同 `<repo>:...` identity(索引键)。
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


def scan_notes(
    repo: str, repo_root: Path, nodes: list[DomainNode]
) -> list[ScannedNote]:
    """扫描各 INDEX 节点子树下的 *.md → ScannedNote(含 INDEX.md 自身)。

    扫描范围 = 任一域节点子树内的 .md(排除 skip / 隐藏目录);不在任何域节点子树内
    的 .md 不扫。identity 重复(NFC + casefold 撞)→ loud ScanError 点名两文件。
    """
    repo_root = repo_root.resolve()
    out: list[ScannedNote] = []
    seen_fold: dict[str, ScannedNote] = {}
    for md in sorted(repo_root.rglob("*.md")):
        rel_parts = md.relative_to(repo_root).parts
        if any(part in _SKIP_DIRS or part.startswith(".") for part in rel_parts[:-1]):
            continue
        is_index = md.name == INDEX_FILENAME
        if is_index:
            # INDEX.md 自身 domain = 它定义的节点(域首页, C2)。
            node = next((n for n in nodes if n.dir == md.parent.resolve()), None)
        else:
            node = _nearest_domain(md, repo_root, nodes)
        if node is None:
            continue  # 不在任何域子树内 → 不扫(C/scan)
        slug = _slug(md, node.dir)
        identity = derive_identity(repo, node.domain, slug)
        note = ScannedNote(
            path=md,
            source_path=md.relative_to(repo_root).as_posix(),
            node=node,
            slug=slug,
            identity=identity,
            is_index=is_index,
        )
        folded = _fold(identity)
        if folded in seen_fold:
            prev = seen_fold[folded]
            raise ScanError(
                f"identity collision in {repo} after NFC+casefold: {identity!r} "
                f"({prev.source_path} 与 {note.source_path}, C3 守卫拒绝)"
            )
        seen_fold[folded] = note
        out.append(note)
    return out


def scan_legacy_notes(repo: str, repo_root: Path) -> list[ScannedNote]:
    """扫描 legacy/raw source 的全部 markdown。

    legacy source 不要求 INDEX.md 域树；所有 .md 统一落到 `legacy` 域。
    这条路径只由 source-level `legacy = true` 触发，不放宽普通 KB 的 C1/C2 闸门。
    """
    repo_root = repo_root.resolve()
    node = DomainNode(
        domain="legacy",
        prefixes=("legacy",),
        index_path=repo_root / INDEX_FILENAME,
        dir=repo_root,
    )
    out: list[ScannedNote] = []
    seen_fold: dict[str, ScannedNote] = {}
    for md in sorted(repo_root.rglob("*.md")):
        rel = md.relative_to(repo_root)
        rel_parts = rel.parts
        if any(part in _SKIP_DIRS or part.startswith(".") for part in rel_parts[:-1]):
            continue
        posix = rel.as_posix()
        slug = posix[: -len(".md")] if posix.endswith(".md") else posix
        identity = derive_identity(repo, node.domain, slug)
        note = ScannedNote(
            path=md,
            source_path=posix,
            node=node,
            slug=slug,
            identity=identity,
            is_index=md.name == INDEX_FILENAME,
        )
        folded = _fold(identity)
        if folded in seen_fold:
            prev = seen_fold[folded]
            raise ScanError(
                f"identity collision in {repo} after NFC+casefold: {identity!r} "
                f"({prev.source_path} 与 {note.source_path}, C3 守卫拒绝)"
            )
        seen_fold[folded] = note
        out.append(note)
    return out
