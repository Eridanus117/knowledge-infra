"""Hybrid 融合 —— weighted-RRF(lexical + semantic)+ query planner。

默认形态(base 融合):
- RRF_K = 60;weighted_rrf 贡献 = weight / (RRF_K + rank)
- lexical_weight 恒 = 1.0
- semantic_weight = 2.0 当 query 为 zh_only_low_anchor(中文低锚),否则 1.0
- 中文低锚 semantic 候选 depth cap = 320(防中文好货不进 pool)
"""

from __future__ import annotations

from dataclasses import dataclass

from memex.config import settings
from memex.engine import Engine
from memex.facets import Facets
from memex.health import HealthCollector, gate_semantic_hits
from memex.planner import is_strongly_anchored, is_zh_low_anchor
from memex.semantic import SemanticEngine

RRF_K = 60.0
SEMANTIC_DEPTH_CAP = 320
LOW_ANCHOR_SEMANTIC_WEIGHT = 2.0  # 默认(weight3 profile=3.0 是 eval 备选)
# 选项 2 开关 on 时:强锚定 query 的 lexical 权重(对称于低锚 semantic 加权)。
STRONG_ANCHOR_LEXICAL_WEIGHT = 2.0
# kind 排序 prior(免调参公式: 以档位为 rank 的伪 lane 票, 4 档位)。
# 极差 1/61-1/64≈8e-4: 只翻 RRF 近分位次,不翻盘双 lane 强相关。kind 缺失/未知 → T4。
KIND_TIER = {
    "spec": 1,
    "reference": 1,
    "runbook": 1,
    "decision": 2,
    "index": 2,
    "research": 3,
    "note": 4,
}
KIND_TIER_FALLBACK = 4
KIND_PRIOR_WEIGHT = 1.0


@dataclass(frozen=True)
class HybridHit:
    object_key: str
    score: float
    repo: str
    lexical_rank: int | None
    semantic_rank: int | None


def _rrf(rank: int, weight: float) -> float:
    return weight / (RRF_K + rank)


class HybridEngine:
    def __init__(
        self,
        lexical: Engine | None = None,
        semantic: SemanticEngine | None = None,
        protect_anchored: bool | None = None,
        kind_prior: bool | None = None,
    ) -> None:
        self.lexical = lexical if lexical is not None else Engine()
        self.semantic = semantic if semantic is not None else SemanticEngine()
        # 选项 2 特性开关(默认随 settings, 默认 off)。
        self.protect_anchored = (
            settings.lexical_dependent_protection
            if protect_anchored is None
            else protect_anchored
        )
        # kind prior 开关(默认随 settings, 默认 off)。
        self.kind_prior = settings.kind_prior if kind_prior is None else kind_prior
        # stale gate 的 artifact 侧真相 = lexical 已加载的语料, 零额外 IO。
        # key=(repo, object_key): 裸 object_key 跨仓会撞。getattr 防御: 测试 stub 无 repos。
        self._docs_by_key = {
            (idx.name, d.object_key): d
            for idx in getattr(self.lexical, "repos", {}).values()
            for d in getattr(idx, "docs", [])
        }

    def _weights(self, query: str) -> tuple[float, float]:
        """(lexical_weight, semantic_weight)。低锚抬 semantic;开关 on 时强锚定抬 lexical。"""
        sem = LOW_ANCHOR_SEMANTIC_WEIGHT if is_zh_low_anchor(query) else 1.0
        lex = (
            STRONG_ANCHOR_LEXICAL_WEIGHT
            if self.protect_anchored and is_strongly_anchored(query)
            else 1.0
        )
        return lex, sem

    def plan(self, query: str) -> dict[str, object]:
        """planner trace(§I5 R6 的人类可读投影)。"""
        low = is_zh_low_anchor(query)
        lex_w, sem_w = self._weights(query)
        return {
            "language_hint": "zh_only_low_anchor" if low else "mixed_or_anchored",
            "low_anchor": low,
            "strongly_anchored": is_strongly_anchored(query),
            "protect_anchored": self.protect_anchored,
            "kind_prior": self.kind_prior,
            "fusion_algorithm": "weighted_rrf",
            "rrf_k": RRF_K,
            "lexical_weight": lex_w,
            "semantic_weight": sem_w,
            "semantic_depth_cap": SEMANTIC_DEPTH_CAP if low else None,
        }

    def search(
        self,
        query: str,
        k: int = 10,
        repo: str | None = None,
        facets: Facets | None = None,
        collect: HealthCollector | None = None,
    ) -> list[HybridHit]:
        lex_weight, sem_weight = self._weights(query)

        # facet 收窄打进两 lane(只过 semantic 会让 lexical 漏未过滤 hit 进融合)。
        # 不收窄时不传 kwarg → 默认路径与旧契约逐字节一致。
        fkw: dict[str, Facets] = {"facets": facets} if facets else {}
        lex_hits = self.lexical.search(query, k=SEMANTIC_DEPTH_CAP, repo=repo, **fkw)
        vec = self.semantic.embed([query])[0]
        sem_hits = self.semantic.search_vec(vec, k=SEMANTIC_DEPTH_CAP, repo=repo, **fkw)
        # stale gate: 过期向量不进融合, 该 key 降级靠 lexical 召回。
        # gate 永远跑(正确性);drops 仅在 collect 注入时记录(eval 不传 = 纯 gate)。
        sem_hits, drops = gate_semantic_hits(sem_hits, self._docs_by_key)
        if collect is not None:
            collect.stale_drops.extend(drops)

        lex_rank = {h.object_key: i for i, h in enumerate(lex_hits, 1)}
        sem_rank = {h.object_key: i for i, h in enumerate(sem_hits, 1)}
        repo_of = {h.object_key: h.repo for h in lex_hits}
        repo_of.update({h.object_key: h.repo for h in sem_hits})

        fused: list[HybridHit] = []
        for key in lex_rank.keys() | sem_rank.keys():
            lr = lex_rank.get(key)
            sr = sem_rank.get(key)
            score = (_rrf(lr, lex_weight) if lr else 0.0) + (
                _rrf(sr, sem_weight) if sr else 0.0
            )
            if self.kind_prior:
                doc = self._docs_by_key.get((repo_of[key], key))
                tier = KIND_TIER.get(
                    getattr(doc, "kind", None) or "", KIND_TIER_FALLBACK
                )
                score += _rrf(tier, KIND_PRIOR_WEIGHT)
            fused.append(HybridHit(key, score, repo_of[key], lr, sr))
        fused.sort(key=lambda h: h.score, reverse=True)
        return fused[:k]
