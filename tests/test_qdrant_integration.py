"""qdrant 集成测试: 本机 :6333 可达才跑, 只动 kb_test_ 前缀 scratch collection。

不真调 :3002 —— 向量用随机 floats(monkeypatch embed_texts)。
"""

from __future__ import annotations

import random
import urllib.request
from pathlib import Path
from typing import Any

import pytest

from memex.config import Settings
from memex.indexing.qdrant import Qdrant
from memex.indexing.sync import (
    SyncMode,
    doc_embed_text,
    ensure_collection,
    point_id,
    sync_repo,
)

SCRATCH = "kb_test_scratch"
_BASE = Settings().qdrant_url


def _qdrant_up() -> bool:
    try:
        with urllib.request.urlopen(f"{_BASE}/collections", timeout=2):
            return True
    except OSError:
        return False


pytestmark = pytest.mark.skipif(not _qdrant_up(), reason=f"qdrant {_BASE} 不可达")

DIM = 4096

FM = """---
description: "集成测试 note"
keywords: [integration, scratch]
kind: note
---

# 集成测试

正文 {n}。
"""


def _note(path: Path, n: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(FM.replace("{n}", str(n)), encoding="utf-8")


def _index(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        '---\ndescription: "home"\nkeywords: [idx]\nkind: index\n---\n\n# home\n',
        encoding="utf-8",
    )


def _rand_vec() -> list[float]:
    return [random.random() for _ in range(DIM)]


def _fake_embed(texts: list[str], s: Any = None) -> list[list[float]]:
    return [_rand_vec() for _ in texts]


@pytest.fixture
def scratch() -> Any:
    s = Settings(central_collection=SCRATCH, embed_batch_size=4)
    client = Qdrant(s)
    if client.collection_exists(SCRATCH):
        client.delete_collection(SCRATCH)
    yield s, client
    if client.collection_exists(SCRATCH):
        client.delete_collection(SCRATCH)


def test_ensure_collection_idempotent(scratch: Any) -> None:
    s, client = scratch
    ensure_collection(client, s)
    assert client.collection_exists(SCRATCH)
    ensure_collection(client, s)  # 已存在 → no-op 不报错
    assert client.collection_exists(SCRATCH)


def test_upsert_retrieve_scroll_roundtrip(scratch: Any) -> None:
    s, client = scratch
    ensure_collection(client, s)
    pid = point_id("itest:d:a")
    payload = {
        "identity": "itest:d:a",
        "point_kind": "note",
        "index_profile": "kb-central-v1",
        "embedding_profile": "test-profile",
        "unit_mode": "whole",
        "text_hash": "th-roundtrip",
    }
    client.upsert(
        SCRATCH, [{"id": pid, "vector": {"object": _rand_vec()}, "payload": payload}]
    )
    got = client.retrieve(SCRATCH, [pid])
    assert len(got) == 1
    assert got[0]["payload"]["identity"] == "itest:d:a"
    # 按 text_hash 过滤 scroll(两级 reuse ② 的真实查询形态)+ 取向量
    pts, _ = client.scroll(
        SCRATCH,
        flt={
            "must": [
                {"key": "text_hash", "match": {"value": "th-roundtrip"}},
                {"key": "embedding_profile", "match": {"value": "test-profile"}},
            ]
        },
        limit=1,
        with_vector=True,
    )
    assert len(pts) == 1
    vec = pts[0]["vector"]["object"]
    assert len(vec) == DIM


def test_full_sync_apply_rerun_prune(
    scratch: Any, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    s, client = scratch
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md", 1)
    _note(tmp_path / "d" / "b.md", 2)

    # 首灌
    _, rep = sync_repo("itest", tmp_path, client=client, s=s, mode=SyncMode(apply=True))
    assert rep.error is None and not rep.failures
    assert len(rep.embedded) == 3

    # 重跑 → 全 unchanged(零 embed: boom 守门)
    def _boom(texts: list[str], _s: Any = None) -> list[list[float]]:
        raise AssertionError("不应 re-embed")

    monkeypatch.setattr("memex.indexing.sync.embed_texts", _boom)
    _, rep2 = sync_repo(
        "itest", tmp_path, client=client, s=s, mode=SyncMode(apply=True)
    )
    assert len(rep2.unchanged) == 3 and not rep2.failures

    # 移动 b → re-key 复用现存向量(零 embed), 旧点 prune(1/3 ≤ 50%)
    (tmp_path / "d" / "b.md").rename(tmp_path / "d" / "moved.md")
    out3, rep3 = sync_repo(
        "itest", tmp_path, client=client, s=s, mode=SyncMode(apply=True)
    )
    repo = out3.canonical_repo
    assert rep3.rekeyed == [f"{repo}:d:moved"]
    assert rep3.pruned == [f"{repo}:d:b"]
    assert not rep3.failures

    # 终态: 3 点(INDEX/a/moved), 旧 b 点已删
    remaining = client.retrieve(
        SCRATCH,
        [point_id(f"{repo}:d:{slug}") for slug in ("INDEX", "a", "moved", "b")],
    )
    idents = sorted(p["payload"]["identity"] for p in remaining)
    assert idents == sorted([f"{repo}:d:INDEX", f"{repo}:d:a", f"{repo}:d:moved"])


def test_write_read_roundtrip_central_flag(
    scratch: Any, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """端到端回环: 写路径写 scratch → flag=True 读路径真查回。

    semantic query 向量 = 写入时记录的该篇向量(cosine top-1 必中), 不调 :3002。
    """
    from memex.engine import Engine
    from memex.hybrid import HybridEngine
    from memex.indexing.pipeline import persist
    from memex.semantic import SemanticEngine

    s_write, client = scratch
    vectors: dict[str, list[float]] = {}

    def _capture_embed(texts: list[str], s: Any = None) -> list[list[float]]:
        return [vectors.setdefault(t, _rand_vec()) for t in texts]

    monkeypatch.setattr("memex.indexing.sync.embed_texts", _capture_embed)
    src = tmp_path / "src"
    _index(src / "d" / "INDEX.md")
    (src / "d" / "widget.md").write_text(
        '---\ndescription: "文档模板配置规则"\nkeywords: [文档模板, 配置]\n---\n\n# 文档模板\n\n文档模板正文。\n',
        encoding="utf-8",
    )
    (src / "d" / "example.md").write_text(
        '---\ndescription: "示例文档描述"\nkeywords: [示例标题, 示例]\n---\n\n# 示例标题\n\n示例描述正文。\n',
        encoding="utf-8",
    )

    out, rep = sync_repo(
        "itest", tmp_path / "src", client=client, s=s_write, mode=SyncMode(apply=True)
    )
    assert rep.error is None and not rep.failures
    repo = out.canonical_repo
    compiled_dir = tmp_path / "compiled"
    persist(out.docs, compiled_dir, repo)

    s_read = Settings(
        read_from_central=True, central_collection=SCRATCH, compiled_dir=compiled_dir
    )
    widget_id = f"{repo}:d:widget"

    # lexical: compiled 目录 → BM25 命中
    eng = Engine(s=s_read)
    lex_hits = eng.search("文档模板", k=3)
    assert lex_hits and lex_hits[0].object_key == widget_id

    # semantic: 中央 collection 真查回(query 向量 = 该篇写入向量)
    doc_a = next(d for d in out.docs if d.source_path == "d/widget.md")
    qvec = vectors[doc_embed_text(doc_a)]
    sem = SemanticEngine(s=s_read)
    sem_hits = sem.search_vec(qvec, k=3)
    assert sem_hits and sem_hits[0].object_key == widget_id
    assert sem_hits[0].repo == repo  # identity 前缀派生

    # repo 收窄 = identity 前缀过滤
    assert sem.search_vec(qvec, k=3, repo="不存在的仓") == []

    # hybrid 融合端到端(query embed 被 monkeypatch 成写入向量, 零 :3002)
    hyb = HybridEngine(lexical=eng, semantic=sem)
    monkeypatch.setattr(sem, "embed", lambda texts: [qvec])
    hh = hyb.search("文档模板", k=3)
    assert hh and hh[0].object_key == widget_id


def test_recall_full_path_central_flag(
    scratch: Any, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """recall() 全路径回环 —— _resolve_engine 按 flag 装配 + query-time embed +
    _doc_lookup 同流富化, 全走生产入口, 不手工注入引擎。query embed 被 monkeypatch
    返回写入向量(零 :3002)。"""
    from memex.config import settings as global_settings
    from memex.indexing.pipeline import persist
    from memex.recall import recall

    s_write, client = scratch
    vectors: dict[str, list[float]] = {}

    def _capture_embed(texts: list[str], s: Any = None) -> list[list[float]]:
        return [vectors.setdefault(t, _rand_vec()) for t in texts]

    monkeypatch.setattr("memex.indexing.sync.embed_texts", _capture_embed)
    src = tmp_path / "src"
    _index(src / "d" / "INDEX.md")
    (src / "d" / "widget.md").write_text(
        '---\ndescription: "文档模板配置规则"\nkeywords: [文档模板, 配置]\n---\n\n# 文档模板\n\n文档模板正文。\n',
        encoding="utf-8",
    )
    out, rep = sync_repo(
        "itest", src, client=client, s=s_write, mode=SyncMode(apply=True)
    )
    assert rep.error is None and not rep.failures
    repo = out.canonical_repo
    compiled_dir = tmp_path / "compiled"
    persist(out.docs, compiled_dir, repo)

    # 翻全局 settings(recall → HybridEngine → Engine/SemanticEngine 都读同一实例)
    monkeypatch.setattr(global_settings, "read_from_central", True)
    monkeypatch.setattr(global_settings, "central_collection", SCRATCH)
    monkeypatch.setattr(global_settings, "compiled_dir", compiled_dir)

    # query-time embed → 写入时记录的该篇向量(cosine top-1 必中)
    doc_a = next(d for d in out.docs if d.source_path == "d/widget.md")
    qvec = vectors[doc_embed_text(doc_a)]
    monkeypatch.setattr(
        "memex.semantic.embed_texts", lambda texts, s=None: [qvec for _ in texts]
    )

    res = recall("文档模板", limit=3, lane="hybrid")
    hits = res.hits
    widget_id = f"{repo}:d:widget"
    assert hits, "recall 全路径零命中(M3 断链形状)"
    assert hits[0].object_key == widget_id
    assert hits[0].repo == repo
    # _doc_lookup 富化字段来自 compiled doc
    assert hits[0].title == "文档模板"
    assert hits[0].path == "d/widget.md"
    # 双 lane 都到场(hybrid 融合非单边)
    assert hits[0].lexical_rank is not None and hits[0].semantic_rank is not None
    # 刚 sync 的新鲜语料 → 健康干净, freshness ok, 无 stale drop
    assert res.health.status == "ok"
    assert res.health.freshness == "ok"
    assert res.health.semantic == "on"
    assert hits[0].semantic_indexed is True
