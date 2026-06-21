"""Canonical recall —— 对外唯一「最佳召回」verb 的引擎层。

recall = hybrid + lexical-dependent protection(生产默认配置, 评测集上 gold@10 ≈ 0.997)
+ title/path 富化(LLM 友好)。这是「对外只留一条 recall verb」收敛的引擎落点;
低层 query verb 仍在(调参 / 单 lane 调试用)。

联邦:一次中央 search + facet 收窄(domain 前缀/kind/tag, 见 facets.py),
无 fan-out/--root;facet 需要中央读路径(legacy artifact 无 facet 字段, 大声拒绝)。

deferred:tier 排序 / authored_from(inferred)过滤现状做不了 —— artifact 的
metadata_projection 只有 object_type/status/topic/workset/retrieval_hint/title,
无 lifecycle/authored_from;等写路径把这些 frontmatter 字段流进 projection 再补。
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from typing import Any

from memex.artifacts import load_artifacts
from memex.config import settings
from memex.facets import Facets
from memex.health import (
    HealthCollector,
    RecallHealth,
    StaleDrop,
    gate_semantic_hits,
)
from memex.registry import active_sources, load_source_registry


@dataclass(frozen=True)
class RecallHit:
    object_key: str
    repo: str
    title: str
    path: str
    score: float
    lexical_rank: int | None
    semantic_rank: int | None
    # False = 仅 lexical(无向量, 未索引);None = 未检查/检查失败。
    semantic_indexed: bool | None = None
    # True = 命中来自 legacy/raw 仓(迁移期, 内容未经实地核验)。
    legacy: bool = False
    raw: bool = False
    unverified: bool = False


@dataclass(frozen=True)
class RecallResult:
    hits: list[RecallHit]
    health: RecallHealth


def _resolve_engine(lane: str) -> Any:
    """lazy import 让 monkeypatch(memex.<mod>.<Engine>)生效, 与 cli._engine_for 一致。"""
    if lane == "hybrid":
        from memex.hybrid import HybridEngine

        return HybridEngine()
    if lane == "lexical":
        from memex.engine import Engine

        return Engine()
    if lane == "semantic":
        from memex.semantic import SemanticEngine

        return SemanticEngine()
    raise ValueError(f"未知 lane: {lane}(hybrid|lexical|semantic)")


def _doc_lookup(repo: str | None) -> dict[tuple[str, str], Any]:
    """(repo, object_key) → Doc(title/path 富化用), lane-independent, 零 BM25。

    双源: flag off 直读 .legacy-index artifact(现行为); flag on 读 compiled 目录
    (object_key=identity, repo=规范仓名, 与 central lane 的 hit 键一致)。
    """
    out: dict[tuple[str, str], Any] = {}
    if settings.read_from_central:
        from memex.compiled import load_compiled_corpus

        for name, docs in load_compiled_corpus(settings).items():
            if repo is not None and name != repo:
                continue
            for d in docs:
                out[(name, d.object_key)] = d
        return out
    for s in active_sources():
        if repo is not None and s.name != repo:
            continue
        for d in load_artifacts(s.artifacts_dir):
            out[(s.name, d.object_key)] = d
    return out


def _check_semantic_indexed(
    hits: list[RecallHit],
) -> tuple[dict[str, bool], str | None]:
    """对 semantic_rank is None 的 hit 批量 retrieve-by-id 查向量存在性。

    central 模式专用(object_key = identity, point_id 确定性可算)。全 hit 有
    semantic_rank → 零网络调用。失败不炸 recall, 返回 note。
    """
    unknown = [h for h in hits if h.semantic_rank is None]
    if not unknown:
        return {}, None
    # 写侧 import 读侧 semantic, 反向 lazy import 无环;仅触发时加载。
    from memex.indexing.qdrant import Qdrant, QdrantError
    from memex.indexing.sync import point_id

    ids = {point_id(h.object_key): h.object_key for h in unknown}
    try:
        points = Qdrant(settings).retrieve(settings.central_collection, list(ids))
    except (QdrantError, OSError) as exc:
        return {}, f"semantic-indexed check unavailable: {exc}"
    found = {str(p.get("id")) for p in points}
    return {key: (pid in found) for pid, key in ids.items()}, None


def recall(  # noqa: C901, PLR0912, PLR0915 — lane 分派(lexical/semantic/hybrid)+ facet 校验 + 健康采集编排; 单一检索入口, 拆分会把 lane 路由逻辑打散
    text: str,
    *,
    limit: int = 10,
    repo: str | None = None,
    lane: str = "hybrid",
    facets: Facets | None = None,
) -> RecallResult:
    if facets and not settings.read_from_central:
        # legacy artifact 无 facet 字段, 静默空结果会撒谎 → 大声拒绝。
        raise ValueError(
            "facet 收窄(--domain/--kind/--tag)需要中央读路径(read_from_central)"
        )
    eng = _resolve_engine(lane)
    # 不收窄时不传 kwarg → 默认路径与旧契约逐字节一致。
    kwargs: dict[str, Any] = {"k": limit, "repo": repo}
    if facets:
        kwargs["facets"] = facets

    docs = _doc_lookup(repo)
    semantic = "on"
    semantic_reason: str | None = None
    fusion = lane
    stale_drops: list[StaleDrop] = []
    notes: list[str] = []

    if lane == "lexical":
        semantic = "off"
        semantic_reason = "lane=lexical"
        hits = eng.search(text, **kwargs)
    elif lane == "semantic":
        # 显式点名 semantic: 不降级, 基础设施异常照炸(静默换道是撒谎)。
        raw = eng.search(text, **kwargs)
        hits, stale_drops = gate_semantic_hits(raw, docs)
        if stale_drops and len(hits) < limit:
            notes.append(
                f"semantic lane: {len(stale_drops)} stale hit(s) dropped, "
                "results may be short"
            )
    else:  # hybrid(默认)
        from memex.semantic import SemanticUnavailable

        collector = HealthCollector()
        try:
            hits = eng.search(text, **kwargs, collect=collector)
            stale_drops = collector.stale_drops
        except SemanticUnavailable as exc:
            # 降级 lexical 不拒返, 健康行大声标注。
            semantic = "off"
            semantic_reason = str(exc)
            fusion = "lexical_only"
            hits = _resolve_engine("lexical").search(text, **kwargs)

    legacy_repos = load_source_registry().legacy
    out: list[RecallHit] = []
    for h in hits:
        d = docs.get((h.repo, h.object_key))
        sr = getattr(h, "semantic_rank", None)
        is_legacy = h.repo in legacy_repos
        out.append(
            RecallHit(
                object_key=h.object_key,
                repo=h.repo,
                title=(getattr(d, "title", "") if d else getattr(h, "title", "")),
                path=(getattr(d, "path", "") if d else getattr(h, "path", "")),
                score=round(float(h.score), 6),
                lexical_rank=getattr(h, "lexical_rank", None),
                semantic_rank=sr,
                semantic_indexed=True if sr is not None else None,
                legacy=is_legacy,
                raw=is_legacy,
                unverified=is_legacy,
            )
        )
    # legacy/raw 命中在消费时刻大声标注。
    n_legacy = sum(1 for h in out if h.legacy)
    if n_legacy:
        notes.append(
            f"{n_legacy} hit(s) 来自 legacy/raw 仓(未经实地核验) — 以实地核验为准"
        )

    # 未索引标注(central + semantic 可用时;降级时 qdrant 状态未知, 不再追打)。
    unindexed = 0
    if settings.read_from_central and semantic == "on":
        indexed_map, check_note = _check_semantic_indexed(out)
        if check_note:
            notes.append(check_note)
        if indexed_map:
            out = [
                h
                if h.object_key not in indexed_map
                else replace(h, semantic_indexed=indexed_map[h.object_key])
                for h in out
            ]
            unindexed = sum(1 for h in out if h.semantic_indexed is False)
            if unindexed:
                notes.append(f"{unindexed} hit(s) 仅 lexical(无向量, 需 semantic sync)")

    # 缺 kind 计数(③): 已加载语料零额外 IO;legacy doc 默认 explicit → 恒 0。
    missing_kind = sum(
        1 for d in docs.values() if not getattr(d, "kind_explicit", True)
    )
    if missing_kind:
        notes.append(f"语料 {missing_kind} 篇缺 kind(默认 note, 稀释 kind prior)")

    freshness = "stale" if stale_drops else "ok"
    degraded = (semantic == "off" and semantic_reason != "lane=lexical") or (
        freshness != "ok"
    )
    health = RecallHealth(
        status="degraded" if degraded else "ok",
        semantic=semantic,
        semantic_reason=semantic_reason,
        freshness=freshness,
        stale_dropped=tuple(stale_drops),
        fusion=fusion,
        missing_kind=missing_kind,
        unindexed=unindexed,
        notes=tuple(notes),
    )
    return RecallResult(hits=out, health=health)
