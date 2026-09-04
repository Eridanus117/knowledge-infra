"""Semantic lane —— 调外部 embedding 服务 + qdrant。

通过「调同一外部服务、同参数」保证排序稳定。常量与行为:
- embed 请求体 = {"model": "qwen3-embedding-8b", "input": [text]},**无 query instruct 前缀**
- qdrant search:named vector "object" + filter must(point_kind/index_profile/embedding_profile)
  + with_payload;默认**不**按 object_type 筛(只在显式 filter 时加)
- 距离 = Cosine(故 query 向量是否归一化不影响排序)
- object_key = payload.source_object_key;score = point.score
- per-repo collection(中央 collection 切换 eval-gated,现状仍 per-root)
纯 stdlib http(urllib),不引第三方 client(零新 wheel)。
"""

from __future__ import annotations

import json
import os
import ssl
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from memex.artifacts import INDEX_PROFILE
from memex.config import Settings, settings
from memex.facets import Facets
from memex.registry import Source, active_sources

# semantic lane 的固定常量(读写同源)。
EMBEDDING_PROFILE_ID = "qwen3-embedding-8b-object-4096-v0"
VECTOR_FIELD = "object"
# 中央 collection 口径(kb-note-v1 写路径产物;写侧 indexing/sync.py 同源 import)。
CENTRAL_INDEX_PROFILE = "kb-central-v1"
CENTRAL_POINT_KIND = "note"
# 语义 fetch 深度:多取再按 object_key 去重(防 chunk 点挤掉 unique 对象),返回 top-k unique。
_DEDUP_DEPTH_FACTOR = 5
_DEDUP_DEPTH_MIN = 50


@dataclass(frozen=True)
class SemanticHit:
    object_key: str
    score: float
    source_path: str
    source_hash: str | None
    compiled_hash: str | None
    repo: str
    # stale gate 用(central payload 才有, legacy 留 None → gate no-op)
    text_hash: str | None = None
    unit_mode: str | None = None


class SemanticUnavailable(Exception):
    """semantic lane 基础设施不可用(embedding gateway / qdrant 网络或响应异常)。

    recall 层据此降级 lexical + 健康行标注;eval/显式 semantic lane 不捕, 照旧 loud。
    """


def _internal_ssl_context(_url: str) -> ssl.SSLContext | None:
    """Build SSLContext from KB_SEARCH_CA_BUNDLE env var if set."""
    ca = os.environ.get("KB_SEARCH_CA_BUNDLE")
    if not ca:
        return None
    p = Path(ca).expanduser()
    if not p.exists():
        return None
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    ctx.load_verify_locations(str(p))
    return ctx


def _post_json(url: str, body: dict[str, Any], timeout: float) -> dict[str, Any]:
    data = json.dumps(body).encode("utf-8")
    headers: dict[str, str] = {"Content-Type": "application/json"}
    token = os.environ.get("KB_SEARCH_BEARER_TOKEN")
    if token:
        headers["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(url, data=data, headers=headers, method="POST")
    ctx = _internal_ssl_context(url)
    try:
        with urllib.request.urlopen(req, timeout=timeout, context=ctx) as resp:
            return json.loads(resp.read())
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", "replace").strip()[:500]
        reason = getattr(exc, "reason", None) or exc.msg
        msg = f"HTTP {exc.code} {reason}"
        if detail:
            msg = f"{msg}: {detail}"
        raise OSError(msg) from exc


# URLError/TimeoutError ⊂ OSError;JSONDecodeError ⊂ ValueError;KeyError = 畸形响应。
_UNAVAILABLE_ERRORS = (OSError, ValueError, KeyError)


def _embedding_unavailable_message(
    exc: BaseException,
    s: Settings,
    batch_size: int,
    *,
    endpoint: str,
    lane: str,
) -> str:
    timeout = f"{s.embed_timeout_secs:g}s"
    return (
        "embedding unreachable: request failed "
        f"(lane={lane}, endpoint={endpoint}, model={s.embedding_model}, "
        f"batch={batch_size}, timeout={timeout}; "
        "set KB_SEARCH_EMBED_TIMEOUT_SECS to lower while diagnosing): "
        f"{exc}"
    )


def embed_texts(
    texts: list[str],
    s: Settings = settings,
    *,
    endpoint: str | None = None,
    lane: str = "query",
) -> list[list[float]]:
    """批量 embed(OpenAI 兼容 /v1/embeddings)。按 index 还原顺序,校验维度。

    基础设施异常(网络/畸形响应/维度不符)统一转 SemanticUnavailable。
    """
    if not texts:
        return []
    url = endpoint or s.embedding_url
    try:
        resp = _post_json(
            url,
            {"model": s.embedding_model, "input": texts},
            s.embed_timeout_secs,
        )
        rows = sorted(resp["data"], key=lambda r: r["index"])
        vectors = [r["embedding"] for r in rows]
        if len(vectors) != len(texts):
            raise ValueError(f"embedding 返回 {len(vectors)} 向量, 期望 {len(texts)}")
        for v in vectors:
            if len(v) != s.embedding_dimensions:
                raise ValueError(f"embedding 维度 {len(v)} != {s.embedding_dimensions}")
    except _UNAVAILABLE_ERRORS as exc:
        raise SemanticUnavailable(
            _embedding_unavailable_message(exc, s, len(texts), endpoint=url, lane=lane)
        ) from exc
    return vectors


def search_collection(
    collection: str, vector: list[float], repo: str, k: int = 10, s: Settings = settings
) -> list[SemanticHit]:
    """单 collection 向量检索 → 按 object_key 去重的 top-k unique。"""
    depth = max(k * _DEDUP_DEPTH_FACTOR, _DEDUP_DEPTH_MIN)
    url = f"{s.qdrant_url}/collections/{collection}/points/search"
    body = {
        "vector": {"name": VECTOR_FIELD, "vector": vector},
        "limit": depth,
        "with_payload": True,
        "with_vector": False,
        "filter": {
            "must": [
                {"key": "point_kind", "match": {"value": "object"}},
                {"key": "index_profile", "match": {"value": INDEX_PROFILE}},
                {"key": "embedding_profile", "match": {"value": EMBEDDING_PROFILE_ID}},
            ]
        },
    }
    try:
        resp = _post_json(url, body, s.qdrant_timeout_secs)
    except _UNAVAILABLE_ERRORS as exc:
        raise SemanticUnavailable(f"qdrant unreachable: {exc}") from exc
    seen: set[str] = set()
    out: list[SemanticHit] = []
    for p in resp.get("result", []):
        pl = p.get("payload") or {}
        key = pl.get("source_object_key", "")
        if not key or key in seen:  # qdrant 已按 score desc → 首见即最高分
            continue
        seen.add(key)
        out.append(
            SemanticHit(
                object_key=key,
                score=float(p.get("score", 0.0)),
                source_path=pl.get("source_path", ""),
                source_hash=pl.get("source_hash"),
                compiled_hash=pl.get("compiled_hash"),
                repo=repo,
            )
        )
        if len(out) >= k:
            break
    return out


def search_central(
    vector: list[float],
    k: int = 10,
    repo: str | None = None,
    s: Settings = settings,
    facets: Facets | None = None,
) -> list[SemanticHit]:
    """中央 collection 检索(read_from_central): kb-note-v1 口径。

    filter = point_kind=note + index_profile=kb-central-v1 + embedding_profile
    (+ facet 收窄 domain_prefixes/kind/keywords, server-side);
    object_key = payload.identity。repo 收窄 = identity 前缀客户端过滤
    (中央无 repo facet), 故多取一档深度再筛。
    """
    depth = max(k * _DEDUP_DEPTH_FACTOR, _DEDUP_DEPTH_MIN)
    url = f"{s.qdrant_url}/collections/{s.central_collection}/points/search"
    must: list[dict[str, Any]] = [
        {"key": "point_kind", "match": {"value": CENTRAL_POINT_KIND}},
        {"key": "index_profile", "match": {"value": CENTRAL_INDEX_PROFILE}},
        {"key": "embedding_profile", "match": {"value": EMBEDDING_PROFILE_ID}},
    ]
    if facets:
        must.extend(facets.qdrant_must())
    body = {
        "vector": {"name": VECTOR_FIELD, "vector": vector},
        "limit": depth,
        "with_payload": True,
        "with_vector": False,
        "filter": {"must": must},
    }
    try:
        resp = _post_json(url, body, s.qdrant_timeout_secs)
    except _UNAVAILABLE_ERRORS as exc:
        raise SemanticUnavailable(f"qdrant unreachable: {exc}") from exc
    prefix = f"{repo}:" if repo is not None else None
    seen: set[str] = set()
    out: list[SemanticHit] = []
    for p in resp.get("result", []):
        pl = p.get("payload") or {}
        identity = pl.get("identity", "")
        if not identity or identity in seen:
            continue
        if prefix is not None and not identity.startswith(prefix):
            continue
        seen.add(identity)
        out.append(
            SemanticHit(
                object_key=identity,
                score=float(p.get("score", 0.0)),
                source_path=pl.get("source_path", ""),
                source_hash=pl.get("source_hash"),
                compiled_hash=pl.get("compiled_hash"),
                repo=identity.split(":", 1)[0],
                text_hash=pl.get("text_hash"),
                unit_mode=pl.get("unit_mode"),
            )
        )
        if len(out) >= k:
            break
    return out


class SemanticEngine:
    def __init__(
        self, sources: list[Source] | None = None, s: Settings = settings
    ) -> None:
        self.s = s
        # 双源 flag: central 时不碰 per-root collection 清单(显式 sources 注入除外, 测试用)。
        self.central = s.read_from_central and sources is None
        self.collections: dict[str, str] = (
            {}
            if self.central
            else {
                src.name: src.collection
                for src in (sources if sources is not None else active_sources())
            }
        )

    def embed(self, texts: list[str]) -> list[list[float]]:
        return embed_texts(texts, self.s)

    def search_vec(
        self,
        vector: list[float],
        k: int = 10,
        repo: str | None = None,
        facets: Facets | None = None,
    ) -> list[SemanticHit]:
        if self.central:
            return search_central(vector, k=k, repo=repo, s=self.s, facets=facets)
        if facets:
            # legacy per-root payload 无 domain_prefixes/kind facet, 静默空结果会撒谎。
            raise ValueError(
                "facet 收窄(domain/kind/tag)需要中央 collection 读路径(read_from_central)"
            )
        if repo is not None:
            if repo not in self.collections:
                raise KeyError(f"未知源仓: {repo}(active: {sorted(self.collections)})")
            return search_collection(self.collections[repo], vector, repo, k, self.s)
        merged: list[SemanticHit] = []
        for name, coll in self.collections.items():
            merged.extend(search_collection(coll, vector, name, k, self.s))
        merged.sort(key=lambda h: h.score, reverse=True)
        return merged[:k]

    def search(
        self,
        query: str,
        k: int = 10,
        repo: str | None = None,
        facets: Facets | None = None,
    ) -> list[SemanticHit]:
        return self.search_vec(self.embed([query])[0], k=k, repo=repo, facets=facets)
