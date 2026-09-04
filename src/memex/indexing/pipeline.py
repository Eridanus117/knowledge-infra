"""编译编排: 一个 repo → 扫描 + 编译 + 报告(+ 可选落盘)。

切片①只到 compiled doc 落盘;qdrant/embedding/orchestrator 是后续切片。
"""

from __future__ import annotations

import shutil
from dataclasses import dataclass
from pathlib import Path

from memex.indexing.compile import (
    CompiledDoc,
    compile_legacy_note,
    compile_note,
    safe_filename,
    write_compiled,
)
from memex.indexing.report import (
    KindDowngrade,
    RepoReport,
    SkipEntry,
)
from memex.indexing.scan import (
    ScanError,
    discover_domains,
    scan_legacy_notes,
    scan_notes,
)


@dataclass
class CompileOutput:
    """一个 repo 的编译产出: 报告 + 成功编译的 docs(dry-run 时不落盘)。

    canonical_repo = repo identity(identity 前缀 + 落盘子目录名)。起
    = registry 逻辑 name(kb-sources.toml), 与物理目录名/位置解耦。
    """

    report: RepoReport
    docs: list[CompiledDoc]
    canonical_repo: str


def compile_repo(name: str, repo_root: Path, *, legacy: bool = False) -> CompileOutput:
    """编译一个源仓 → (报告, docs)。仓不可用 / 守卫触发 → 报告携错, 不抛。"""
    repo_root = repo_root.expanduser()
    report = RepoReport(repo=name, repo_path=str(repo_root))
    if not repo_root.is_dir():
        report.error = f"repo path not found: {repo_root}"
        return CompileOutput(report=report, docs=[], canonical_repo=name)

    # repo identity = registry 逻辑 name(kb-sources.toml), 与物理目录名/位置
    # 解耦。不取磁盘 basename — 仓迁移/改名(leaf≠name)不再改 identity。
    repo = name

    if legacy:
        return _compile_legacy_repo(name, repo_root, repo, report)

    try:
        nodes = discover_domains(repo_root)
    except ScanError as exc:
        report.duplicate_error = str(exc)
        return CompileOutput(report=report, docs=[], canonical_repo=repo)

    report.domains = [n.domain for n in nodes]

    try:
        scanned = scan_notes(repo, repo_root, nodes)
    except ScanError as exc:
        report.duplicate_error = str(exc)
        return CompileOutput(report=report, docs=[], canonical_repo=repo)

    docs: list[CompiledDoc] = []
    domains_with_notes: set[str] = set()
    for note in scanned:
        result = compile_note(note, repo_root)
        if result.skipped_no_frontmatter:
            report.skipped.append(
                SkipEntry(source_path=note.source_path, reason="no frontmatter")
            )
            continue
        assert result.doc is not None
        if result.kind_downgraded_from is not None:
            report.kind_downgrades.append(
                KindDowngrade(
                    source_path=note.source_path,
                    identity=note.identity,
                    from_kind=result.kind_downgraded_from,
                )
            )
        # 一旦文档显式进入 lifecycle,kind 只是可选检索标签,不再是写入门禁。
        # 仅对完全没有 status 的迁移文档保留旧的缺 kind 信号。
        if not result.doc.kind_explicit and not result.doc.status:
            report.kind_missing.append(note.source_path)
        docs.append(result.doc)
        domains_with_notes.add(note.node.domain)

    report.indexed = len(docs)
    # 覆盖率 diff(C1): 发现的域 vs 实有 note 的域 → 空域。
    report.empty_domains = sorted(
        d for d in report.domains if d not in domains_with_notes
    )
    return CompileOutput(report=report, docs=docs, canonical_repo=repo)


def _compile_legacy_repo(
    _name: str, repo_root: Path, repo: str, report: RepoReport
) -> CompileOutput:
    """legacy/raw source 编译路径：无 INDEX/frontmatter 闸门，但低可信标记强制入产物。"""
    report.domains = ["legacy"]
    try:
        scanned = scan_legacy_notes(repo, repo_root)
    except ScanError as exc:
        report.duplicate_error = str(exc)
        return CompileOutput(report=report, docs=[], canonical_repo=repo)

    docs: list[CompiledDoc] = []
    for note in scanned:
        result = compile_legacy_note(note, repo_root)
        assert result.doc is not None
        docs.append(result.doc)

    report.indexed = len(docs)
    if not docs:
        report.empty_domains = ["legacy"]
    return CompileOutput(report=report, docs=docs, canonical_repo=repo)


def persist(docs: list[CompiledDoc], compiled_dir: Path, repo: str) -> int:
    """落盘一个仓的 docs 到 <compiled_dir>/<repo>/, 返回写入数。"""
    out_dir = compiled_dir.expanduser() / repo
    for doc in docs:
        write_compiled(doc, out_dir)
    return len(docs)


@dataclass
class PruneResult:
    """compiled 目录 stale 清理结果。

    stale = 本轮 compile 结果之外的残留文件名;deleted=True 表示已真删;
    refused = mass-delete 守卫拒绝文案(>50%, 需 --force)。
    """

    stale: list[str]
    deleted: bool = False
    refused: str | None = None


def prune_stale_compiled(
    docs: list[CompiledDoc],
    compiled_dir: Path,
    repo: str,
    *,
    apply: bool,
    force: bool = False,
) -> PruneResult:
    """清理 <compiled_dir>/<repo>/ 里 scan 结果之外的 stale 产物。

    域退出 INDEX 链后 qdrant 点被 sync prune, 但 compiled 落盘端残留 →
    lexical/semantic 两 lane 语料歪斜。守卫口径同 qdrant prune: 待删 >50%
    拒绝(需 force);默认 dry-run(apply=False 只报告)。文件名按 identity
    编码(自带 repo 前缀)且按仓子目录收窄, 不会误删他仓产物。
    """
    out_dir = compiled_dir.expanduser() / repo
    if not out_dir.is_dir():
        return PruneResult(stale=[])
    expected = {safe_filename(d.identity) for d in docs}
    existing = sorted(p.name for p in out_dir.glob("*.json"))
    stale = [n for n in existing if n not in expected]
    if not stale:
        return PruneResult(stale=[])
    if len(stale) * 2 > len(existing) and not force:
        verb = "将" if not apply else ""
        return PruneResult(
            stale=stale,
            refused=(
                f"compiled 待删 {len(stale)} > 本仓现存 {len(existing)} 的 50%, "
                f"拒绝删除;stale 产物{verb}保留, 确认无误后 --force 清理"
            ),
        )
    if not apply:
        return PruneResult(stale=stale)
    for name in stale:
        (out_dir / name).unlink()
    return PruneResult(stale=stale, deleted=True)


@dataclass
class RetiredRepoPrune:
    """整仓退役清理结果(compiled 子目录级)。

    retired = compiled_dir 下不在 active sources 的整个 repo 子目录名;
    deleted=True 已真删;refused = mass-delete 守卫拒绝文案(>50% 仓目录, 需 --force)。
    """

    retired: list[str]
    deleted: bool = False
    refused: str | None = None


def prune_retired_repos(
    compiled_dir: Path,
    active_repos: set[str] | frozenset[str],
    *,
    apply: bool,
    force: bool = False,
) -> RetiredRepoPrune:
    """清理 <compiled_dir> 下整个退役 repo 子目录(不在 active sources 清单内)。

    prune_stale_compiled 只清「同仓子目录内 scan 之外的单篇」, 覆盖不到「整个 repo
    改名/退役后旧子目录整体成孤儿」(物理 leaf→registry name 改名后, 旧 leaf
    名 compiled 目录无人 scan, 永远不进单篇 prune)。本函数按 active 仓名集合判退役,
    整目录删。守卫同口径: 待删仓目录 >50% 拒绝(需 force);默认 dry-run(apply=False)。

    active_repos 必须是「全量 registry」的仓名集合 —— 用 --repo 子集调用会把其余仓
    全判退役, 调用方须只在全量 sync 路径传入。
    """
    base = compiled_dir.expanduser()
    if not base.is_dir():
        return RetiredRepoPrune(retired=[])
    existing = sorted(p.name for p in base.iterdir() if p.is_dir())
    retired = [n for n in existing if n not in active_repos]
    if not retired:
        return RetiredRepoPrune(retired=[])
    if len(retired) * 2 > len(existing) and not force:
        verb = "将" if not apply else ""
        return RetiredRepoPrune(
            retired=retired,
            refused=(
                f"compiled 待删整仓目录 {len(retired)} > 现存 {len(existing)} 的 50%, "
                f"拒绝删除;退役目录{verb}保留, 确认无误后 --force 清理"
            ),
        )
    if not apply:
        return RetiredRepoPrune(retired=retired)
    for name in retired:
        shutil.rmtree(base / name)
    return RetiredRepoPrune(retired=retired, deleted=True)
