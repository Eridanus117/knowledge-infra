from __future__ import annotations

import json
import sys
from typing import Any

import typer

from memex import telemetry
from memex.logging_setup import setup_logging

app = typer.Typer(no_args_is_help=True, add_completion=False, help="memex")


@app.callback()
def _root() -> None:
    """保留子命令名空间 (typer 单命令会塌缩, 加 callback 防止)。"""


def _engine_for(lane: str):
    if lane == "lexical":
        from memex.engine import Engine

        return Engine()
    if lane == "semantic":
        from memex.semantic import SemanticEngine

        return SemanticEngine()
    if lane == "hybrid":
        from memex.hybrid import HybridEngine

        return HybridEngine()
    raise typer.BadParameter(f"未知 lane: {lane}(lexical|semantic|hybrid)")


@app.command()
def query(
    text: str = typer.Argument(..., help="检索 query"),
    hits: int = typer.Option(10, help="返回 top-k"),
    repo: str = typer.Option(None, help="收窄到单个源仓 (默认跨所有 active 源仓)"),
    lane: str = typer.Option("lexical", help="lexical | semantic | hybrid"),
    fmt: str = typer.Option("text", "--format", help="text | json"),
) -> None:
    """检索 (lexical=候选A BM25 / semantic=qdrant 向量 / hybrid=weighted-RRF, 默认无类型过滤)。"""
    setup_logging()
    results = _engine_for(lane).search(text, k=hits, repo=repo)
    if fmt == "json":
        # eval 适配契约: {hits:[{object_key,...}]}。
        payload = {
            "hits": [
                {
                    "object_key": h.object_key,
                    "score": round(h.score, 6),
                    "repo": h.repo,
                    "title": getattr(h, "title", ""),
                    "path": getattr(h, "path", None) or getattr(h, "source_path", ""),
                }
                for h in results
            ]
        }
        typer.echo(json.dumps(payload, ensure_ascii=False))
    else:
        for i, h in enumerate(results, 1):
            typer.echo(f"{i:2}. [{h.repo}] {h.object_key}  ({h.score:.4f})")


@app.command()
def recall(
    text: str = typer.Argument(..., help="检索 query"),
    limit: int = typer.Option(10, help="返回 top-k(默认 10)"),
    repo: str = typer.Option(None, help="收窄到单个源仓 (默认跨所有 active 源仓)"),
    domain: str = typer.Option(
        None,
        help="按域前缀收窄 (INDEX.md 域路径, 如 decisions / tools/foo; 前缀语义)",
    ),
    kind: str = typer.Option(
        None, help="按 kind 收窄 (spec|reference|decision|research|runbook|note|index)"
    ),
    tag: str = typer.Option(None, help="按 frontmatter keyword 收窄 (精确 match)"),
    lane: str = typer.Option(
        "hybrid",
        help="hybrid(默认=最佳) | lexical | semantic;远端 embedding 不可用时降 lexical",
    ),
    fmt: str = typer.Option("text", "--format", help="text | json"),
    preview: bool = typer.Option(
        False, "--preview", help="每条附正文摘要片段(判相关性用)"
    ),
) -> None:
    """唯一最佳召回入口:hybrid + lexical-dependent protection(评测集上 gold@10 ≈ 0.997)+ title/path 富化。

    联邦 = 一次中央 search(无 fan-out/--root);--domain/--kind/--tag 是
    server-side facet 收窄。与低层 query 的区别 = recall 锁定生产最佳配置, 不必懂
    lane 调参。tier 排序 / --include-inferred 过滤 deferred(待写路径流入 lifecycle/
    authored_from)。
    """
    setup_logging()
    from memex.facets import Facets
    from memex.recall import recall as do_recall
    from memex.semantic import SemanticUnavailable

    facets = Facets(domain=domain, kind=kind, tag=tag)
    try:
        res = do_recall(
            text,
            limit=limit,
            repo=repo,
            lane=lane,
            facets=facets if facets else None,
            with_preview=preview,
        )
    except (ValueError, SemanticUnavailable) as exc:
        # SemanticUnavailable 只在显式 --lane semantic 时穿透(点名要 semantic 不降级)。
        typer.echo(f"error: {exc}", err=True)
        raise typer.Exit(2) from exc
    hits = res.hits
    if fmt == "json":
        payload = {
            "hits": [
                {
                    "object_key": h.object_key,
                    "repo": h.repo,
                    "title": h.title,
                    "path": h.path,
                    "abs_path": h.abs_path,
                    "score": h.score,
                    "lexical_rank": h.lexical_rank,
                    "semantic_rank": h.semantic_rank,
                    "semantic_indexed": h.semantic_indexed,
                    "legacy": h.legacy,
                    "raw": h.raw,
                    "unverified": h.unverified,
                    "preview": h.preview,
                }
                for h in hits
            ],
            "health": res.health.to_dict(),
        }
        typer.echo(json.dumps(payload, ensure_ascii=False))
        return
    # text: 健康干净时字节级安静, 仅信号时打 banner。
    if res.health.has_signal:
        typer.echo(res.health.banner())
    if not hits:
        typer.echo("(no hits)")
    else:
        for i, h in enumerate(hits, 1):
            mark = "  ⚠ legacy/raw 未核验" if h.unverified else ""
            typer.echo(
                f"{i:2}. [{h.repo}] {h.title or h.object_key}  ({h.score:.4f}){mark}"
            )
            typer.echo(f"     {h.object_key}")
            if h.abs_path:
                typer.echo(f"     → {h.abs_path}")
            if h.preview:
                typer.echo(f"     ┄ {h.preview}")


def _clip_text(text: str, max_chars: int) -> tuple[str, bool]:
    if max_chars <= 0 or len(text) <= max_chars:
        return text, False
    return text[:max_chars], True


def _candidate_text(title: str, path: str, body: str) -> str:
    return "\n\n".join(p for p in (title, path, body) if p)


def _export_hit_payload(
    h: Any, docs: dict[tuple[str, str], Any], max_text_chars: int
) -> dict[str, Any]:
    d = docs.get((h.repo, h.object_key))
    body = getattr(d, "body", "") if d else ""
    full_text = _candidate_text(h.title, h.path, body)
    candidate_text, truncated = _clip_text(full_text, max_text_chars)
    return {
        "object_key": h.object_key,
        "repo": h.repo,
        "title": h.title,
        "path": h.path,
        "score": h.score,
        "lexical_rank": h.lexical_rank,
        "semantic_rank": h.semantic_rank,
        "semantic_indexed": h.semantic_indexed,
        "legacy": h.legacy,
        "raw": h.raw,
        "unverified": h.unverified,
        "kind": getattr(d, "kind", "") if d else "",
        "domain_prefixes": list(getattr(d, "domain_prefixes", ())) if d else [],
        "keywords": list(getattr(d, "keywords", ())) if d else [],
        "candidate_text": candidate_text,
        "candidate_text_chars": len(full_text),
        "candidate_text_truncated": truncated,
    }


@app.command("recall-export")
def recall_export(
    text: str = typer.Argument(..., help="检索 query"),
    limit: int = typer.Option(10, help="返回 top-k(默认 10)"),
    repo: str = typer.Option(None, help="收窄到单个源仓 (默认跨所有 active 源仓)"),
    domain: str = typer.Option(
        None,
        help="按域前缀收窄 (INDEX.md 域路径, 如 decisions / tools/foo; 前缀语义)",
    ),
    kind: str = typer.Option(
        None, help="按 kind 收窄 (spec|reference|decision|research|runbook|note|index)"
    ),
    tag: str = typer.Option(None, help="按 frontmatter keyword 收窄 (精确 match)"),
    lane: str = typer.Option(
        "hybrid",
        help="hybrid(默认=最佳) | lexical | semantic;远端 embedding 不可用时降 lexical",
    ),
    max_text_chars: int = typer.Option(
        12000,
        help="每条候选文本最大字符数;0 表示不截断",
        min=0,
    ),
) -> None:
    """导出 reranker/eval 候选:生产 recall 元数据 + title/path/body candidate_text。"""
    setup_logging()
    from memex.facets import Facets
    from memex.recall import _doc_lookup
    from memex.recall import recall as do_recall
    from memex.semantic import SemanticUnavailable

    facets = Facets(domain=domain, kind=kind, tag=tag)
    try:
        res = do_recall(
            text, limit=limit, repo=repo, lane=lane, facets=facets if facets else None
        )
    except (ValueError, SemanticUnavailable) as exc:
        typer.echo(f"error: {exc}", err=True)
        raise typer.Exit(2) from exc
    docs = _doc_lookup(repo)
    payload = {
        "schema": "memex-recall-export-v1",
        "query": text,
        "lane": lane,
        "limit": limit,
        "repo": repo,
        "facets": {
            "domain": facets.domain,
            "kind": facets.kind,
            "tag": facets.tag,
        },
        "hits": [
            _export_hit_payload(h, docs, max_text_chars=max_text_chars)
            for h in res.hits
        ],
        "health": res.health.to_dict(),
    }
    typer.echo(json.dumps(payload, ensure_ascii=False))


@app.command()
def stats() -> None:
    """本地用量统计: per-verb 调用次数 / p50·p95 耗时 / 错误率 (零网络, 见 telemetry.py)。"""
    typer.echo(telemetry.stats())


def run() -> None:
    """Console-script entry: 在 per-invocation telemetry 捕获下跑 CLI。
    wrapper 负责 stdout/stderr 捕获 + exit-code 映射, 然后向本地 SQLite ledger 写一行
    ($MEMEX_TELEMETRY_OFF 或 DO_NOT_TRACK 关闭)。"""
    raise SystemExit(telemetry.run_instrumented(app, sys.argv[1:]))


if __name__ == "__main__":
    run()
