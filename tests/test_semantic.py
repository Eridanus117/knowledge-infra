from memex import semantic
from memex.config import Settings

S = Settings(embedding_dimensions=3)


def test_embed_reorders_by_index(monkeypatch) -> None:
    # OpenAI 兼容响应可能乱序 → 必须按 index 还原。
    monkeypatch.setattr(
        semantic,
        "_post_json",
        lambda *a, **k: {
            "data": [
                {"index": 1, "embedding": [9, 9, 9]},
                {"index": 0, "embedding": [1, 1, 1]},
            ]
        },
    )
    vecs = semantic.embed_texts(["a", "b"], S)
    assert vecs == [[1, 1, 1], [9, 9, 9]]


def test_embed_dim_mismatch_raises(monkeypatch) -> None:
    monkeypatch.setattr(
        semantic,
        "_post_json",
        lambda *a, **k: {"data": [{"index": 0, "embedding": [1, 1]}]},
    )
    import pytest

    # 基础设施异常(含维度不符)统一转 SemanticUnavailable, recall 层据此降级。
    with pytest.raises(semantic.SemanticUnavailable):
        semantic.embed_texts(["a"], S)


def test_search_dedups_by_object_key(monkeypatch) -> None:
    # qdrant 按 score desc;同 object_key 的 chunk 点应去重,首见(最高分)保留。
    monkeypatch.setattr(
        semantic,
        "_post_json",
        lambda *a, **k: {
            "result": [
                {
                    "score": 0.9,
                    "payload": {"source_object_key": "k1", "source_path": "a.md"},
                },
                {
                    "score": 0.8,
                    "payload": {"source_object_key": "k1", "source_path": "a.md"},
                },
                {
                    "score": 0.7,
                    "payload": {"source_object_key": "k2", "source_path": "b.md"},
                },
            ]
        },
    )
    hits = semantic.search_collection("coll", [0.0, 0.0, 0.0], repo="r", k=10, s=S)
    assert [h.object_key for h in hits] == ["k1", "k2"]
    assert hits[0].score == 0.9
    assert hits[0].repo == "r"


def test_search_central_facets_go_server_side(monkeypatch) -> None:
    from memex.facets import Facets

    captured: dict = {}

    def _fake_post(url, body, timeout):
        captured["body"] = body
        return {"result": []}

    monkeypatch.setattr(semantic, "_post_json", _fake_post)
    semantic.search_central(
        [0.0, 0.0, 0.0], k=5, s=S, facets=Facets(domain="decisions/", kind="decision")
    )
    must = captured["body"]["filter"]["must"]
    assert {
        "key": "domain_prefixes",
        "match": {"value": "decisions"},
    } in must  # 归一去尾斜杠
    assert {"key": "kind", "match": {"value": "decision"}} in must
    # 基础三条件仍在(facet 是追加, 不替换)。
    assert {
        "key": "point_kind",
        "match": {"value": semantic.CENTRAL_POINT_KIND},
    } in must


def test_search_central_no_facets_body_unchanged(monkeypatch) -> None:
    captured: dict = {}

    def _fake_post(url, body, timeout):
        captured["body"] = body
        return {"result": []}

    monkeypatch.setattr(semantic, "_post_json", _fake_post)
    semantic.search_central([0.0, 0.0, 0.0], k=5, s=S)
    assert len(captured["body"]["filter"]["must"]) == 3  # 默认路径零变化


def test_legacy_lane_rejects_facets() -> None:
    import pytest

    from memex.facets import Facets

    eng = semantic.SemanticEngine(sources=[], s=S)  # 显式 sources → legacy 路径
    with pytest.raises(ValueError, match="中央"):
        eng.search_vec([0.0, 0.0, 0.0], facets=Facets(kind="decision"))
