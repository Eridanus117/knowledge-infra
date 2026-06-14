"""编译编排: 一个 repo → 扫描 + 编译 + 报告(+ 可选落盘)。

切片①只到 compiled doc 落盘;qdrant/embedding/orchestrator 是后续切片。
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from memex.indexing.compile import (
    CompiledDoc,
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
    repo_name,
    scan_notes,
)


@dataclass
class CompileOutput:
    """一个 repo 的编译产出: 报告 + 成功编译的 docs(dry-run 时不落盘)。

    canonical_repo = 规范仓名(identity 前缀 + 落盘子目录名), 可能 != registry 标签
    (worktree 取主 checkout basename)。
    """

    report: RepoReport
    docs: list[CompiledDoc]
    canonical_repo: str


def compile_repo(name: str, repo_root: Path) -> CompileOutput:
    """编译一个源仓 → (报告, docs)。仓不可用 / 守卫触发 → 报告携错, 不抛。"""
    repo_root = repo_root.expanduser()
    report = RepoReport(repo=name, repo_path=str(repo_root))
    if not repo_root.is_dir():
        report.error = f"repo path not found: {repo_root}"
        return CompileOutput(report=report, docs=[], canonical_repo=name)

    # 规范仓名(worktree 取主 checkout basename);name 是 registry 标签, 仅用于显示。
    repo = repo_name(repo_root)

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
        if not result.doc.kind_explicit:
            report.kind_missing.append(note.source_path)
        docs.append(result.doc)
        domains_with_notes.add(note.node.domain)

    report.indexed = len(docs)
    # 覆盖率 diff(C1): 发现的域 vs 实有 note 的域 → 空域。
    report.empty_domains = sorted(
        d for d in report.domains if d not in domains_with_notes
    )
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
