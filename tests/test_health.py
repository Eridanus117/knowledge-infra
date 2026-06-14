"""读路径可观测: stale gate / 健康行 / 降级 / 未索引 / 缺 kind。

口径: 过期降级不拒返、未索引标"仅 lexical"、健康时字节级安静。
"""

from __future__ import annotations

import json

from typer.testing import CliRunner

from memex.artifacts import Doc
from memex.cli import app
from memex.config import Settings
from memex.health import (
    HealthCollector,
    RecallHealth,
    StaleDrop,
    gate_semantic_hits,
    stale_drop_reason,
)
from memex.hybrid import HybridEngine, HybridHit
from memex.lexical import Hit
from memex.semantic import SemanticHit, SemanticUnavailable

runner = CliRunner()


def _doc(key: str = "kb:d:a", **kw) -> Doc:
    base = {
        "object_key": key,
        "title": "t",
        "body": "b",
        "path": "p.md",
        "source_hash": "s1",
        "compiled_hash": "c1",
    }
    base.update(kw)
    return Doc(**base)


def _sem(key: str = "kb:d:a", repo: str = "kb", **kw) -> SemanticHit:
    base = {
        "object_key": key,
        "score": 1.0,
        "source_path": "p.md",
        "source_hash": "s1",
        "compiled_hash": "c1",
        "repo": repo,
        "text_hash": "t1",
        "unit_mode": "whole",
    }
    base.update(kw)
    return SemanticHit(**base)


# ---- stale_drop_reason 各分支 -------------------------------------------------


def test_stale_drop_reason_each_branch() -> None:
    assert stale_drop_reason(_sem(), _doc()) is None  # 新鲜
    assert stale_drop_reason(_sem(), None) == "compiled_doc_missing"
    assert (
        stale_drop_reason(_sem(unit_mode="chunk"), _doc()) == "embedding_unit_mismatch"
    )
    assert stale_drop_reason(_sem(text_hash=None), _doc()) == "text_hash_missing"
    assert stale_drop_reason(_sem(source_hash="OLD"), _doc()) == "source_hash_mismatch"
    assert (
        stale_drop_reason(_sem(compiled_hash="OLD"), _doc()) == "compiled_hash_mismatch"
    )


def test_gate_noop_for_legacy_shapes() -> None:
    # legacy payload 形状(无 unit_mode/text_hash)→ 整体 no-op, 即使 doc 缺。
    legacy_hit = _sem(text_hash=None, unit_mode=None)
    assert stale_drop_reason(legacy_hit, None) is None
    # legacy 语料 doc(无 compiled_hash)→ no-op。
    assert stale_drop_reason(_sem(), _doc(compiled_hash=None)) is None


def test_gate_drops_stale_keeps_fresh() -> None:
    fresh = _sem("kb:d:a")
    stale = _sem("kb:d:b", compiled_hash="OLD")
    docs = {("kb", "kb:d:a"): _doc("kb:d:a"), ("kb", "kb:d:b"): _doc("kb:d:b")}
    kept, drops = gate_semantic_hits([fresh, stale], docs)
    assert [h.object_key for h in kept] == ["kb:d:a"]
    assert drops == [StaleDrop("kb:d:b", "compiled_hash_mismatch")]


# ---- RecallHealth 信号/渲染 ---------------------------------------------------


def _health(**kw) -> RecallHealth:
    base = {
        "status": "ok",
        "semantic": "on",
        "semantic_reason": None,
        "freshness": "ok",
        "stale_dropped": (),
        "fusion": "hybrid",
        "missing_kind": 0,
        "unindexed": 0,
        "notes": (),
    }
    base.update(kw)
    return RecallHealth(**base)


def test_health_signal_quiet_when_all_ok() -> None:
    assert _health().has_signal is False
    assert _health(status="degraded").has_signal is True
    assert _health(semantic="off", semantic_reason="x").has_signal is True
    assert _health(freshness="stale").has_signal is True
    assert _health(notes=("n",)).has_signal is True


def test_banner_renders_freshness_and_reasons() -> None:
    h = _health(
        status="degraded",
        freshness="stale",
        stale_dropped=(
            StaleDrop("a", "compiled_hash_mismatch"),
            StaleDrop("b", "compiled_hash_mismatch"),
            StaleDrop("c", "text_hash_missing"),
        ),
        notes=("note-1",),
    )
    b = h.banner()
    assert "status=degraded" in b
    assert "3 过期向量降级→lexical" in b
    assert "compiled_hash_mismatch x2" in b
    assert "text_hash_missing x1" in b
    assert "! note-1" in b


# ---- hybrid: 融合前 gate + collector ------------------------------------------


class _Ns:
    def __init__(self, name: str, docs: list[Doc]) -> None:
        self.name = name
        self.docs = docs


class _FakeLexEngine:
    """带 repos 的 lexical fake(供 HybridEngine 建 _docs_by_key)。"""

    def __init__(self, hits: list[Hit], docs: list[Doc]) -> None:
        self._hits = hits
        self.repos = {"kb": _Ns("kb", docs)}

    def search(self, query, k=10, repo=None, facets=None):
        return self._hits[:k]


class _FakeSemEngine:
    def __init__(self, hits: list[SemanticHit]) -> None:
        self._hits = hits

    def embed(self, texts):
        return [[0.0]] * len(texts)

    def search_vec(self, vector, k=10, repo=None, facets=None):
        return self._hits[:k]


def test_stale_semantic_candidate_dropped_pre_fusion() -> None:
    # doc 现 compiled_hash=c1;semantic 点带旧 c0 → gate 掉, 该 key 只拿 lexical 贡献。
    doc = _doc("kb:d:a")
    lex = _FakeLexEngine(
        [Hit(object_key="kb:d:a", score=1.0, title="", path="", repo="kb")], [doc]
    )
    sem = _FakeSemEngine([_sem("kb:d:a", compiled_hash="c0")])
    eng = HybridEngine(lexical=lex, semantic=sem)
    collector = HealthCollector()
    hits = eng.search("q", k=5, collect=collector)
    assert len(hits) == 1
    assert hits[0].lexical_rank == 1
    assert hits[0].semantic_rank is None  # semantic 贡献被 gate 掉
    assert collector.stale_drops == [StaleDrop("kb:d:a", "compiled_hash_mismatch")]


def test_search_without_collector_signature_compatible() -> None:
    # eval 路径不传 collect: gate 照跑, 行为不变, 不炸。
    doc = _doc("kb:d:a")
    lex = _FakeLexEngine(
        [Hit(object_key="kb:d:a", score=1.0, title="", path="", repo="kb")], [doc]
    )
    sem = _FakeSemEngine([_sem("kb:d:a", compiled_hash="c0")])
    hits = HybridEngine(lexical=lex, semantic=sem).search("q", k=5)
    assert hits[0].semantic_rank is None


# ---- recall 聚合层(走 CLI 生产入口)--------------------------------------------


class _HybridOK:
    """健康 hybrid fake: 双 rank 都在 → 零未索引检查、零信号。"""

    def __init__(self, *_a, **_k) -> None:
        pass

    def search(self, text, k=10, repo=None, facets=None, collect=None):
        return [
            HybridHit(
                object_key="kb:doc:a",
                score=0.123456,
                repo="ekb",
                lexical_rank=1,
                semantic_rank=1,
            )
        ]


class _HybridDown(_HybridOK):
    def search(self, text, k=10, repo=None, facets=None, collect=None):
        raise SemanticUnavailable("embedding unreachable: boom")


class _LexFallback:
    def __init__(self, *_a, **_k) -> None:
        pass

    def search(self, text, k=10, repo=None, facets=None):
        return [
            Hit(object_key="kb:doc:a", score=2.5, title="A", path="a.md", repo="ekb")
        ]


def _central(monkeypatch) -> None:
    monkeypatch.setattr("memex.recall.settings", Settings(read_from_central=True))


def test_recall_text_byte_identical_when_healthy(monkeypatch) -> None:
    _central(monkeypatch)
    monkeypatch.setattr("memex.hybrid.HybridEngine", _HybridOK)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})
    result = runner.invoke(app, ["recall", "文档"])
    assert result.exit_code == 0, result.stdout
    # 健康干净 → 与改前输出逐字节一致(无 banner、无任何新增字节)。
    assert result.stdout == " 1. [ekb] kb:doc:a  (0.1235)\n     kb:doc:a\n"


def test_recall_json_has_health_and_hits_backcompat(monkeypatch) -> None:
    _central(monkeypatch)
    monkeypatch.setattr("memex.hybrid.HybridEngine", _HybridOK)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})
    result = runner.invoke(app, ["recall", "文档", "--format", "json"])
    assert result.exit_code == 0, result.stdout
    payload = json.loads(result.stdout)
    h = payload["hits"][0]
    for key in (
        "object_key",
        "repo",
        "title",
        "path",
        "score",
        "lexical_rank",
        "semantic_rank",
    ):
        assert key in h  # 旧 7 key 原样
    assert h["semantic_indexed"] is True
    assert payload["health"]["status"] == "ok"
    assert payload["health"]["semantic"] == "on"
    assert payload["health"]["freshness"] == "ok"


def test_semantic_outage_degrades_to_lexical(monkeypatch) -> None:
    _central(monkeypatch)
    monkeypatch.setattr("memex.hybrid.HybridEngine", _HybridDown)
    monkeypatch.setattr("memex.engine.Engine", _LexFallback)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})
    result = runner.invoke(app, ["recall", "文档", "--format", "json"])
    assert result.exit_code == 0, result.stdout
    payload = json.loads(result.stdout)
    assert payload["hits"][0]["object_key"] == "kb:doc:a"  # lexical 兜住, 不拒返
    health = payload["health"]
    assert health["status"] == "degraded"
    assert health["semantic"] == "off"
    assert "embedding unreachable" in health["semantic_reason"]
    assert health["fusion"] == "lexical_only"


def test_semantic_outage_text_banner(monkeypatch) -> None:
    _central(monkeypatch)
    monkeypatch.setattr("memex.hybrid.HybridEngine", _HybridDown)
    monkeypatch.setattr("memex.engine.Engine", _LexFallback)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})
    result = runner.invoke(app, ["recall", "文档"])
    assert result.exit_code == 0, result.stdout
    assert result.stdout.startswith("! health: status=degraded")
    assert "kb:doc:a" in result.stdout


def test_semantic_lane_outage_stays_loud(monkeypatch) -> None:
    # 显式 --lane semantic: 不降级, 异常穿透为 exit 2(点名要 semantic, 静默换道是撒谎)。
    class _SemDown:
        def __init__(self, *_a, **_k) -> None:
            pass

        def search(self, text, k=10, repo=None, facets=None):
            raise SemanticUnavailable("qdrant unreachable: boom")

    _central(monkeypatch)
    monkeypatch.setattr("memex.semantic.SemanticEngine", _SemDown)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})
    result = runner.invoke(app, ["recall", "文档", "--lane", "semantic"])
    assert result.exit_code == 2


def test_missing_kind_counted_from_corpus(monkeypatch) -> None:
    _central(monkeypatch)
    monkeypatch.setattr("memex.hybrid.HybridEngine", _HybridOK)
    docs = {
        ("ekb", "kb:doc:a"): _doc("kb:doc:a", kind_explicit=True),
        ("ekb", "kb:doc:b"): _doc("kb:doc:b", kind_explicit=False),
        ("ekb", "kb:doc:c"): _doc("kb:doc:c", kind_explicit=False),
    }
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: docs)
    result = runner.invoke(app, ["recall", "文档", "--format", "json"])
    assert result.exit_code == 0, result.stdout
    payload = json.loads(result.stdout)
    assert payload["health"]["missing_kind"] == 2
    assert any("2 篇缺 kind" in n for n in payload["health"]["notes"])
    # text 面: 缺 kind 是信号 → banner 出现。
    text_result = runner.invoke(app, ["recall", "文档"])
    assert "缺 kind" in text_result.stdout


class _HybridLexOnlyHit(_HybridOK):
    """命中无 semantic_rank → 触发未索引存在性检查。"""

    def search(self, text, k=10, repo=None, facets=None, collect=None):
        return [
            HybridHit(
                object_key="kb:doc:a",
                score=0.1,
                repo="ekb",
                lexical_rank=1,
                semantic_rank=None,
            )
        ]


def test_unindexed_hit_flagged_via_retrieve(monkeypatch) -> None:
    _central(monkeypatch)
    monkeypatch.setattr("memex.hybrid.HybridEngine", _HybridLexOnlyHit)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})
    monkeypatch.setattr(
        "memex.indexing.qdrant.Qdrant.retrieve",
        lambda self, coll, ids, with_vector=False: [],  # 向量不存在
    )
    result = runner.invoke(app, ["recall", "文档", "--format", "json"])
    assert result.exit_code == 0, result.stdout
    payload = json.loads(result.stdout)
    assert payload["hits"][0]["semantic_indexed"] is False
    assert payload["health"]["unindexed"] == 1
    assert any("仅 lexical" in n for n in payload["health"]["notes"])


def test_indexed_check_skipped_when_all_hits_have_semantic_rank(monkeypatch) -> None:
    _central(monkeypatch)
    monkeypatch.setattr("memex.hybrid.HybridEngine", _HybridOK)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})

    def _no_call(self, coll, ids, with_vector=False):
        raise AssertionError("全 hit 有 semantic_rank 时不应触发存在性查询")

    monkeypatch.setattr("memex.indexing.qdrant.Qdrant.retrieve", _no_call)
    result = runner.invoke(app, ["recall", "文档", "--format", "json"])
    assert result.exit_code == 0, result.stdout


def test_indexed_check_failure_noted_not_fatal(monkeypatch) -> None:
    from memex.indexing.qdrant import QdrantError

    _central(monkeypatch)
    monkeypatch.setattr("memex.hybrid.HybridEngine", _HybridLexOnlyHit)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})

    def _boom(self, coll, ids, with_vector=False):
        raise QdrantError("down")

    monkeypatch.setattr("memex.indexing.qdrant.Qdrant.retrieve", _boom)
    result = runner.invoke(app, ["recall", "文档", "--format", "json"])
    assert result.exit_code == 0, result.stdout
    payload = json.loads(result.stdout)
    assert payload["hits"][0]["semantic_indexed"] is None  # 未知, 不冒充 False
    assert any(
        "semantic-indexed check unavailable" in n for n in payload["health"]["notes"]
    )
