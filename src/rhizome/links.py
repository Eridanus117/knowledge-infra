"""link-checker: frontmatter links/code 断引用大声报。

删除真实的安全前提: 删 note 前能看见谁还引用它。判定规则:

  links  — slug 须在同仓可解析。断链 = ERROR(阻断 gate);跨仓命中 = WARN
           (契约只定义同仓 slug 引用);identity 形(含 ':')= ERROR(用 slug)。
  code   — 出处 hint,代码漂移预期内,一律最高 WARN:含 '…'/'...' 省略 = 不可
           核验;路径形(含 '/')解析不到 = 可能已漂移。非路径形(表名/符号)
           不判——它们是合法出处但本检查无法核验。

frozen 文档整篇豁免: 只读历史,链向已删 design 稿是故意留痕(蒸馏即删),
报了也不许改。
"""

from __future__ import annotations

import os
from pathlib import Path

from . import contract, sources
from .check import _WALK_SKIP_DIRS, ERROR, WARN, Finding

# code 解析根: 仓自身 / workspace / workspace 下已知聚合目录(按需扩展)。
# 公开默认为空(中性); 项目特有的聚合目录名经 env 注入, 有效集合 = 默认 + env。
_EXTRA_CODE_ROOT_NAMES: tuple[str, ...] = ()
_CODE_ROOTS_ENV = "RHIZOME_CODE_ROOTS"


def _extra_code_root_names() -> tuple[str, ...]:
    """有效 code 聚合目录名 = 默认 + $RHIZOME_CODE_ROOTS(逗号分隔, 去重去空)。

    env 未设 → 纯默认; 空串/多余空格/空项忽略; 保序去重。
    """
    seen: list[str] = list(_EXTRA_CODE_ROOT_NAMES)
    raw = os.environ.get(_CODE_ROOTS_ENV)
    if raw:
        for part in raw.split(","):
            name = part.strip()
            if name and name not in seen:
                seen.append(name)
    return tuple(seen)


_slug_cache: dict[Path, set[str]] = {}
_foreign_cache: dict[str, set[str]] = {}


def _slug_index(repo_root: Path) -> set[str]:
    cached = _slug_cache.get(repo_root)
    if cached is not None:
        return cached
    slugs: set[str] = set()
    for p in repo_root.rglob("*.md"):
        if _WALK_SKIP_DIRS & set(p.parts):
            continue
        slugs.add(p.stem)
    _slug_cache[repo_root] = slugs
    return slugs


def _foreign_repos(repo_root: Path) -> dict[str, set[str]]:
    """registry 其他源仓的 slug 索引(跨仓命中降级 WARN 用);registry 不可用则空。"""
    try:
        srcs = sources.load_sources()
    except Exception:
        return {}
    out: dict[str, set[str]] = {}
    for name, path in srcs:
        if path == repo_root:
            continue
        if name not in _foreign_cache:
            if not path.is_dir():
                _foreign_cache[name] = set()
            else:
                _foreign_cache[name] = _slug_index(path)
        out[name] = _foreign_cache[name]
    return out


def _workspace_root() -> Path:
    try:
        return sources._workspace_root()
    except Exception:
        return Path.home() / "workspace"


def _code_resolvable(entry: str, repo_root: Path) -> bool:
    rel = entry.split("#", 1)[0].strip()
    ws = _workspace_root()
    roots = [repo_root, ws, *(ws / n for n in _extra_code_root_names())]
    return any((r / rel).exists() for r in roots)


def link_findings(path: Path, text: str) -> list[Finding]:
    """一篇 note 的 links/code 引用检查;非 note / frozen / 无 frontmatter → 静默跳过。"""
    try:
        fm = contract.parse_frontmatter(text)
    except contract.ContractError:
        return []  # frontmatter 本身坏 → check_text 已报,不重复
    if fm is None or contract.is_frozen_fm(fm):
        return []
    if not contract.is_note_location(path):
        return []
    repo_root = contract.find_repo_root(path)
    if repo_root is None:
        return []

    findings: list[Finding] = []

    links = fm.get("links")
    if isinstance(links, list):
        own = _slug_index(repo_root)
        foreign: dict[str, set[str]] | None = None
        for raw in links:
            slug = str(raw).strip()
            if not slug:
                continue
            if ":" in slug:
                findings.append(
                    Finding(
                        ERROR,
                        "links",
                        f"{slug!r} 是 identity 形 — links 用 slug(note-contract)",
                    )
                )
                continue
            if slug in own:
                continue
            if foreign is None:
                foreign = _foreign_repos(repo_root)
            hits = sorted(name for name, slugs in foreign.items() if slug in slugs)
            if hits:
                findings.append(
                    Finding(
                        WARN,
                        "links",
                        f"{slug!r} 本仓无、命中他仓 {hits} — 跨仓引用契约未定义,确认归属",
                    )
                )
            else:
                findings.append(
                    Finding(
                        ERROR,
                        "links",
                        f"{slug!r} 断链 — 目标 note 不存在(已删则一并删此引用)",
                    )
                )

    code = fm.get("code")
    if isinstance(code, list):
        for raw in code:
            entry = str(raw).strip()
            if not entry:
                continue
            if "..." in entry or "…" in entry:
                findings.append(
                    Finding(
                        WARN,
                        "code",
                        f"{entry!r} 含省略号不可核验 — 写全路径或降为正文叙述",
                    )
                )
                continue
            if "/" not in entry:
                continue  # 表名/符号类出处,非路径,不判
            if not _code_resolvable(entry, repo_root):
                findings.append(
                    Finding(
                        WARN,
                        "code",
                        f"{entry!r} 解析不到(workspace/仓内) — 代码已迁移或引用未合入主干",
                    )
                )

    return findings
