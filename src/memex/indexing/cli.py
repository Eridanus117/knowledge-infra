"""memex-sync CLI —— 写路径入口。

  memex-sync compile  --repo <name>=<path> [...] [--out DIR] [--dry-run]
  memex-sync sync     --repo <name>=<path> [...] [--apply] [--force]
  memex-sync sync-all [--apply] [--force]

sync 内部先 compile;默认 dry-run(零写入), --apply 才动 qdrant + 落盘 compiled。
sync-all = orchestrator: 遍历 registry(真相收敛到 kb-sources.toml)串行各仓,
单仓失败记录继续。退出码: 0 = 全绿;1 = 任一仓硬失败;2 = 无硬失败但有
prune 守卫拒绝(需人工确认后 --force);3 = 无 1/2 但有内容完整性发现
(0-doc 仓 / 域内静默 skip,供日审 cadence 检测告警)。
"""

from __future__ import annotations

from pathlib import Path

import typer

from memex.config import settings
from memex.indexing.integrity import IntegrityReport
from memex.indexing.pipeline import (
    PruneResult,
    compile_repo,
    persist,
    prune_stale_compiled,
)
from memex.registry import load_source_registry

app = typer.Typer(
    no_args_is_help=True, add_completion=False, help="memex-sync 写路径编译"
)

# M-3: 退出码语义(主线已拍)。
EXIT_FAILURE = 1  # 硬失败(单仓 error / 单篇失败 / 崩溃)
EXIT_NEEDS_FORCE = 2  # 无硬失败, 但 prune 守卫拒绝 → 需人工介入(--force)
# 无硬失败、无 prune 拒绝, 但有内容完整性发现(0-doc 仓 / 域内静默 skip)→ distinct
# 退出码, 供日审 cadence 检测告警。优先级低于 NEEDS_FORCE/FAILURE(那两个要人立刻
# 处置), 这个是可被自动检测的告警信号。
EXIT_INTEGRITY = 3


@app.callback()
def _root() -> None:
    """保留子命令名空间(typer 单命令会塌缩, 加 callback 防止)。"""


def _resolve_repos(repo_args: list[str] | None) -> tuple[dict[str, Path], str | None]:
    """--repo name=path ... → ({name: path}, 降级警示)。

    无 --repo 时取 registry 收敛真相;降级(toml 不可用)→ 返回 WARN 文案,
    调用方 echo 进运行输出(M-2: 降级不能只埋日志)。
    """
    if not repo_args:
        reg = load_source_registry()
        warn = (
            f"WARN: 源仓清单降级到内置默认(kb-sources.toml 不可用: {reg.reason})"
            if reg.degraded
            else None
        )
        return reg.repos, warn
    out: dict[str, Path] = {}
    for raw in repo_args:
        if "=" not in raw:
            raise typer.BadParameter(f"--repo 需 name=path 形式, 收到: {raw!r}")
        name, _, path = raw.partition("=")
        name, path = name.strip(), path.strip()
        if not name or not path:
            raise typer.BadParameter(f"--repo name/path 不可为空: {raw!r}")
        out[name] = Path(path).expanduser()
    return out, None


def _echo_prune(pr: PruneResult, repo: str, out_dir: Path) -> bool:
    """echo compiled stale 清理结果, 返回是否守卫拒绝。"""
    if pr.refused:
        typer.echo(f"  compiled-prune REFUSED: {pr.refused}")
        return True
    if pr.stale:
        verb = "deleted" if pr.deleted else "would delete"
        typer.echo(
            f"  compiled-prune {verb} {len(pr.stale)} stale doc(s) ← {out_dir / repo}"
        )
        for name in pr.stale:
            typer.echo(f"    - {name}")
    return False


def _echo_integrity(integ: IntegrityReport) -> None:
    """显眼打印跨仓完整性 section(无发现则静默)。"""
    section = integ.render()
    if section:
        typer.echo("")
        typer.echo(section)


@app.command(name="compile")
def compile_cmd(
    repo: list[str] = typer.Option(
        None, "--repo", help="name=path(可重复);省略则用默认源仓列表"
    ),
    out: Path = typer.Option(
        None, "--out", help=f"compiled doc 落点(默认 {settings.compiled_dir})"
    ),
    dry_run: bool = typer.Option(False, "--dry-run", help="只打报告不落盘"),
    force: bool = typer.Option(
        False, "--force", help="放行 compiled stale mass-delete 守卫(待删 >50%)"
    ),
) -> None:
    """扫源仓 → 编译 kb-note-v1 → 落盘 + stale 清理 + 报告(--dry-run 只报告)。

    退出码: 0 全绿 / 2 stale 清理守卫拒绝需人工 --force / 3 内容完整性发现
    (0-doc 仓 / 域内静默 skip)。
    """
    repos, degraded_warn = _resolve_repos(repo)
    if degraded_warn:
        typer.echo(degraded_warn)
    out_dir = out.expanduser() if out is not None else settings.compiled_dir

    total_indexed = 0
    total_written = 0
    any_needs_force = False
    integ = IntegrityReport()
    for name, path in repos.items():
        result = compile_repo(name, path)
        typer.echo(result.report.render())
        integ.add_repo(result.report)
        total_indexed += result.report.indexed
        if not dry_run and result.docs:
            written = persist(result.docs, out_dir, result.canonical_repo)
            total_written += written
            typer.echo(f"  wrote {written} doc(s) → {out_dir / result.canonical_repo}")
        if not (result.report.error or result.report.duplicate_error):
            pr = prune_stale_compiled(
                result.docs,
                out_dir,
                result.canonical_repo,
                apply=not dry_run,
                force=force,
            )
            any_needs_force = (
                _echo_prune(pr, result.canonical_repo, out_dir) or any_needs_force
            )
        typer.echo("")

    mode = "dry-run" if dry_run else "compiled"
    tail = "" if dry_run else f", wrote {total_written}"
    typer.echo(
        f"[{mode}] indexed {total_indexed} doc(s){tail} across {len(repos)} repo(s)"
    )
    _echo_integrity(integ)
    if any_needs_force:
        raise typer.Exit(code=EXIT_NEEDS_FORCE)
    if integ.has_findings:
        raise typer.Exit(code=EXIT_INTEGRITY)


@app.command(name="sync")
def sync_cmd(
    repo: list[str] = typer.Option(
        None, "--repo", help="name=path(可重复);省略则用默认源仓列表"
    ),
    apply: bool = typer.Option(
        False, "--apply", help="真写 qdrant + 落盘 compiled(默认 dry-run 零写入)"
    ),
    force: bool = typer.Option(
        False, "--force", help="放行 mass-prune 守卫(单仓待删 >50%)"
    ),
    out: Path = typer.Option(
        None, "--out", help=f"compiled doc 落点(默认 {settings.compiled_dir})"
    ),
) -> None:
    """compile + qdrant sync(两级 reuse + prune 守卫);默认 dry-run 只报告。

    退出码: 0 全绿 / 1 硬失败 / 2 prune 守卫拒绝需人工 --force / 3 内容完整性发现
    (仅在无 1/2 时)。
    """
    from memex.indexing.sync import sync_repo

    repos, degraded_warn = _resolve_repos(repo)
    if degraded_warn:
        typer.echo(degraded_warn)
    out_dir = out.expanduser() if out is not None else settings.compiled_dir

    any_error = False
    any_needs_force = False
    integ = IntegrityReport()
    for name, path in repos.items():
        c_out, s_rep = sync_repo(name, path, apply=apply, force=force)
        typer.echo(c_out.report.render())
        integ.add_repo(c_out.report)
        typer.echo(s_rep.render())
        if apply and c_out.docs:
            written = persist(c_out.docs, out_dir, c_out.canonical_repo)
            typer.echo(
                f"  wrote {written} compiled doc(s) → {out_dir / c_out.canonical_repo}"
            )
        if not (c_out.report.error or c_out.report.duplicate_error):
            pr = prune_stale_compiled(
                c_out.docs, out_dir, c_out.canonical_repo, apply=apply, force=force
            )
            any_needs_force = (
                _echo_prune(pr, c_out.canonical_repo, out_dir) or any_needs_force
            )
        any_error = any_error or bool(s_rep.error) or bool(s_rep.failures)
        any_needs_force = any_needs_force or s_rep.needs_force
        typer.echo("")
    _echo_integrity(integ)
    if any_error:
        raise typer.Exit(code=EXIT_FAILURE)
    if any_needs_force:
        raise typer.Exit(code=EXIT_NEEDS_FORCE)
    if integ.has_findings:
        raise typer.Exit(code=EXIT_INTEGRITY)


@app.command(name="sync-all")
def sync_all_cmd(
    apply: bool = typer.Option(
        False, "--apply", help="真写 qdrant + 落盘 compiled(默认 dry-run 零写入)"
    ),
    force: bool = typer.Option(
        False, "--force", help="放行 mass-prune 守卫(单仓待删 >50%)"
    ),
    out: Path = typer.Option(
        None, "--out", help=f"compiled doc 落点(默认 {settings.compiled_dir})"
    ),
) -> None:
    """orchestrator: 遍历 registry 串行 sync 各源仓;单仓失败继续。

    退出码: 0 全绿 / 1 任一仓硬失败 / 2 无硬失败但有 prune 拒绝(需人工 --force)/
    3 无 1/2 但有内容完整性发现(日审 cadence 据此告警)。
    """
    from memex.indexing.sync import sync_repo

    reg = load_source_registry()
    out_dir = out.expanduser() if out is not None else settings.compiled_dir

    summaries: list[str] = []
    all_failures: list[tuple[str, str]] = []
    needs_force: list[tuple[str, str]] = []  # (repo, prune_refused 文案)
    failed_repos = 0
    integ = IntegrityReport()
    for name, path in reg.repos.items():
        try:
            c_out, s_rep = sync_repo(name, path, apply=apply, force=force)
        except Exception as exc:  # 单仓意外崩溃不中断全批(D4)
            summaries.append(f"{name}: CRASH — {exc}")
            all_failures.append((name, f"crash: {exc}"))
            failed_repos += 1
            continue
        typer.echo(c_out.report.render())
        integ.add_repo(c_out.report)
        typer.echo(s_rep.render())
        if apply and c_out.docs:
            written = persist(c_out.docs, out_dir, c_out.canonical_repo)
            typer.echo(
                f"  wrote {written} compiled doc(s) → {out_dir / c_out.canonical_repo}"
            )
        if not (c_out.report.error or c_out.report.duplicate_error):
            pr = prune_stale_compiled(
                c_out.docs, out_dir, c_out.canonical_repo, apply=apply, force=force
            )
            if _echo_prune(pr, c_out.canonical_repo, out_dir):
                needs_force.append((name, pr.refused or ""))
        typer.echo("")
        summaries.append(f"indexed {c_out.report.indexed} | {s_rep.summary_line()}")
        if s_rep.error:
            all_failures.append((name, s_rep.error))
        all_failures.extend((f"{name}:{ident}", err) for ident, err in s_rep.failures)
        if s_rep.error or s_rep.failures:
            failed_repos += 1
        if s_rep.needs_force:
            needs_force.append((name, s_rep.prune_refused or ""))

    typer.echo(f"=== sync-all 汇总 [{'apply' if apply else 'dry-run'}] ===")
    if reg.degraded:
        typer.echo(
            f"WARN: 源仓清单降级到内置默认(kb-sources.toml 不可用: {reg.reason})"
        )
    for line in summaries:
        typer.echo(line)
    if all_failures:
        typer.echo(f"总失败清单 [{len(all_failures)}]:")
        for ident, err in all_failures:
            typer.echo(f"  - {ident}: {err}")
    if needs_force:
        typer.echo(f"需人工介入(--force) [{len(needs_force)}]:")
        for name, msg in needs_force:
            typer.echo(f"  - {name}: {msg}")
    _echo_integrity(integ)
    if failed_repos:
        typer.echo(f"{failed_repos} repo(s) failed")
        raise typer.Exit(code=EXIT_FAILURE)
    if needs_force:
        raise typer.Exit(code=EXIT_NEEDS_FORCE)
    if integ.has_findings:
        raise typer.Exit(code=EXIT_INTEGRITY)


def run() -> None:
    """Console-script entry。"""
    app()


if __name__ == "__main__":
    run()
