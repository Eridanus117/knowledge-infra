"""读路径可观测。

stale 拆两态:过期 = semantic 候选融合前 gate 掉(降级靠 lexical 召回, 标 unreliable,
不拒返);未索引 = 返回 hit 标"仅 lexical"。健康干净的 recall 字节级安静, 仅信号时
显示 banner。

职责:engine 自报原始事实(HealthCollector sink), recall.py 聚合成 RecallHealth
(status/fusion 判定依赖 lane 选择与降级策略, engine 不该知道)。
"""

from __future__ import annotations

from collections import Counter
from collections.abc import Mapping
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from memex.artifacts import Doc
    from memex.semantic import SemanticHit

# 字面量对齐写侧 sync.UNIT_MODE_WHOLE(不 import 写路径链)。
_UNIT_MODE_WHOLE = "whole"

# stale 原因五值: embedding_profile_missing 在 central 查询
# filter(must embedding_profile=...)下不可达, 删;新增 compiled_doc_missing(向量在、
# compiled doc 已删的孤儿点)。
STALE_REASONS: tuple[str, ...] = (
    "source_hash_mismatch",
    "compiled_hash_mismatch",
    "embedding_unit_mismatch",
    "text_hash_missing",
    "compiled_doc_missing",
)


@dataclass(frozen=True)
class StaleDrop:
    object_key: str
    reason: str  # ∈ STALE_REASONS


@dataclass
class HealthCollector:
    """engine 自报原始事实的 sink;recall 聚合成 RecallHealth。"""

    stale_drops: list[StaleDrop] = field(default_factory=list)


def stale_drop_reason(hit: SemanticHit, doc: Doc | None) -> str | None:  # noqa: PLR0911 — guard-clause ladder: 每个 return 是一条独立 stale 判据, 合并会更难读
    """semantic 候选的 stale 判定;None = 新鲜保留。

    legacy 形状(payload 无 unit_mode 且无 text_hash)整体 no-op —— 迁移期, 不背 gate。
    """
    if hit.unit_mode is None and hit.text_hash is None:
        return None
    if doc is None:
        return "compiled_doc_missing"
    if doc.compiled_hash is None:  # legacy 语料(防御;正常 central doc 必有)
        return None
    if hit.unit_mode != _UNIT_MODE_WHOLE:
        return "embedding_unit_mismatch"
    if not hit.text_hash:
        return "text_hash_missing"
    if hit.source_hash is not None and hit.source_hash != doc.source_hash:
        return "source_hash_mismatch"
    if hit.compiled_hash != doc.compiled_hash:
        return "compiled_hash_mismatch"
    return None


def gate_semantic_hits(
    hits: list[SemanticHit],
    docs_by_key: Mapping[tuple[str, str], Doc],
) -> tuple[list[SemanticHit], list[StaleDrop]]:
    """融合前 stale gate: 过期向量从 semantic 池丢弃(该 key 降级靠 lexical 召回)。"""
    kept: list[SemanticHit] = []
    drops: list[StaleDrop] = []
    for h in hits:
        reason = stale_drop_reason(h, docs_by_key.get((h.repo, h.object_key)))
        if reason is None:
            kept.append(h)
        else:
            drops.append(StaleDrop(object_key=h.object_key, reason=reason))
    return kept, drops


@dataclass(frozen=True)
class RecallHealth:
    status: str  # "ok" | "degraded"
    semantic: str  # "on" | "off"
    semantic_reason: str | None  # off 时必填(降级原因 / "lane=lexical")
    freshness: str  # "ok" | "stale"
    stale_dropped: tuple[StaleDrop, ...]
    fusion: str  # "hybrid" | "lexical_only"(降级) | "lexical" | "semantic"
    missing_kind: int  # 全语料缺 kind 计数(central;legacy 恒 0)
    unindexed: int  # 返回 hits 里 semantic_indexed=False 的数量
    notes: tuple[str, ...]

    @property
    def has_signal(self) -> bool:
        # healthSignal 规则: 健康干净时字节级安静。
        return (
            self.status != "ok"
            or self.semantic != "on"
            or self.freshness != "ok"
            or bool(self.notes)
        )

    def _freshness_text(self) -> str:
        if not self.stale_dropped:
            return self.freshness
        reasons = Counter(d.reason for d in self.stale_dropped)
        detail = ", ".join(f"{r} x{n}" for r, n in sorted(reasons.items()))
        return (
            f"stale ({len(self.stale_dropped)} 过期向量降级→lexical, "
            f"unreliable: {detail}; 需 reindex)"
        )

    def banner(self) -> str:
        parts = [
            f"status={self.status}",
            f"语义 {self.semantic}"
            + (f" ({self.semantic_reason})" if self.semantic_reason else ""),
            f"新鲜度 {self._freshness_text()}",
            f"融合 {self.fusion}",
        ]
        line = "! health: " + " · ".join(parts)
        for n in self.notes:
            line += f"\n! {n}"
        return line

    def to_dict(self) -> dict[str, Any]:
        return {
            "status": self.status,
            "semantic": self.semantic,
            "semantic_reason": self.semantic_reason,
            "freshness": self.freshness,
            "stale_dropped": [
                {"object_key": d.object_key, "reason": d.reason}
                for d in self.stale_dropped
            ],
            "fusion": self.fusion,
            "missing_kind": self.missing_kind,
            "unindexed": self.unindexed,
            "notes": list(self.notes),
        }
