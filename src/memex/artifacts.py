"""读 `.legacy-index/index/artifacts` 的 lexical projection。

artifact 是索引产物;Python lexical lane 直接消费同一份(零迁移)。
只取选定 index_profile + schema 的 object-artifact,按 source_object_key 去重。
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

# sidecar profile / schema(保证 A/B 同一语料)。
INDEX_PROFILE = "hybrid-qwen3-qdrant-v0"
ARTIFACT_SCHEMA = "object-artifact-v0"


@dataclass(frozen=True)
class Doc:
    object_key: str
    title: str
    body: str
    path: str
    # facet 字段(compiled kb-note-v1 才有;legacy artifact 留空 → legacy 路径不支持 facet 过滤)
    kind: str = ""
    domain_prefixes: tuple[str, ...] = ()
    keywords: tuple[str, ...] = ()
    # 读侧可观测字段(compiled 才有, legacy 留 None → stale gate 自然 no-op)
    kind_explicit: bool = True
    source_hash: str | None = None
    compiled_hash: str | None = None


def load_artifacts(artifacts_dir: Path) -> list[Doc]:
    """加载一个仓的 artifacts 目录 → 去重 Doc 列表。"""
    docs: dict[str, Doc] = {}
    for f in sorted(artifacts_dir.glob("*.json")):
        d = json.loads(f.read_text(encoding="utf-8"))
        if (
            d.get("index_profile") != INDEX_PROFILE
            or d.get("artifact_schema") != ARTIFACT_SCHEMA
        ):
            continue
        key = d.get("source_object_key")
        if not key or key in docs:
            continue
        lp = d.get("lexical_projection") or {}
        meta = d.get("metadata_projection") or {}
        docs[key] = Doc(
            object_key=key,
            title=lp.get("title") or meta.get("title") or "",
            body=lp.get("body_text") or "",
            path=d.get("source_path") or "",
        )
    return list(docs.values())
