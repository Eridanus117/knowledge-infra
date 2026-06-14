from memex.hybrid import RRF_K, HybridEngine
from memex.lexical import Hit
from memex.semantic import SemanticHit


class _FakeLex:
    def __init__(self, hits: list[Hit]) -> None:
        self._hits = hits

    def search(self, query: str, k: int = 10, repo: str | None = None) -> list[Hit]:
        return self._hits[:k]


class _FakeSem:
    def __init__(self, hits: list[SemanticHit]) -> None:
        self._hits = hits

    def embed(self, texts: list[str]) -> list[list[float]]:
        return [[0.0]] * len(texts)

    def search_vec(
        self, vector, k: int = 10, repo: str | None = None
    ) -> list[SemanticHit]:
        return self._hits[:k]


def _lex(key: str) -> Hit:
    return Hit(object_key=key, score=1.0, title="", path="", repo="r")


def _sem(key: str) -> SemanticHit:
    return SemanticHit(
        object_key=key,
        score=1.0,
        source_path="",
        source_hash=None,
        compiled_hash=None,
        repo="r",
    )


def test_rrf_fuses_both_lanes() -> None:
    # a: lex#1 sem#3; b: lex#2 sem#1 → b 综合更高(出现在两路高位)。
    lex = _FakeLex([_lex("a"), _lex("b")])
    sem = _FakeSem([_sem("b"), _sem("x"), _sem("a")])
    eng = HybridEngine(lexical=lex, semantic=sem)
    hits = eng.search("widget template v1", k=10, repo="r")  # 非低锚 → 等权
    keys = [h.object_key for h in hits]
    assert keys[0] == "b"
    assert set(keys) == {"a", "b", "x"}


def test_low_anchor_weights_semantic_double() -> None:
    eng = HybridEngine(lexical=_FakeLex([]), semantic=_FakeSem([]))
    plan = eng.plan("文档模板配置")
    assert plan["low_anchor"] is True
    assert plan["semantic_weight"] == 2.0
    assert plan["semantic_depth_cap"] == 320
    plan2 = eng.plan("widget v1")
    assert plan2["semantic_weight"] == 1.0


def test_protection_flag_raises_lexical_weight() -> None:
    q = "point ID object_id uuid_v5 CHUNKED_EMBEDDING"  # 强锚定
    off = HybridEngine(
        lexical=_FakeLex([]), semantic=_FakeSem([]), protect_anchored=False
    )
    on = HybridEngine(
        lexical=_FakeLex([]), semantic=_FakeSem([]), protect_anchored=True
    )
    assert off.plan(q)["lexical_weight"] == 1.0
    assert on.plan(q)["lexical_weight"] == 2.0
    # 非强锚定 query 即使开关 on 也不抬。
    assert on.plan("文档模板配置")["lexical_weight"] == 1.0


def test_rrf_score_value() -> None:
    # kind_prior off: 隔离验证纯 RRF 分值(prior 项的分值在 test_hybrid_kind_prior)。
    eng = HybridEngine(
        lexical=_FakeLex([_lex("a")]), semantic=_FakeSem([]), kind_prior=False
    )
    hits = eng.search("widget v1", k=10, repo="r")  # 非低锚, lexical 权重 1
    assert abs(hits[0].score - 1.0 / (RRF_K + 1)) < 1e-9
    assert hits[0].lexical_rank == 1
    assert hits[0].semantic_rank is None
