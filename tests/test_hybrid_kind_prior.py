from types import SimpleNamespace

from memex.hybrid import KIND_PRIOR_WEIGHT, RRF_K, HybridEngine
from memex.lexical import Hit
from memex.semantic import SemanticHit


class _FakeLex:
    def __init__(self, hits: list[Hit], kinds: dict[str, str] | None = None) -> None:
        self._hits = hits
        docs = [SimpleNamespace(object_key=k, kind=v) for k, v in (kinds or {}).items()]
        self.repos = {"r": SimpleNamespace(name="r", docs=docs)}

    def search(self, query: str, k: int = 10, repo: str | None = None) -> list[Hit]:
        return self._hits[:k]


class _FakeSem:
    def __init__(self, hits: list[SemanticHit]) -> None:
        self._hits = hits

    def embed(self, texts: list[float]) -> list[list[float]]:
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


def test_default_on_and_flag_off_path_unchanged() -> None:
    # 默认随 settings(eval-gate 过闸后 on);显式 off 时分数 = 纯 RRF,无 prior 项。
    assert HybridEngine(lexical=_FakeLex([]), semantic=_FakeSem([])).kind_prior is True
    eng = HybridEngine(
        lexical=_FakeLex([_lex("a")], kinds={"a": "note"}),
        semantic=_FakeSem([]),
        kind_prior=False,
    )
    assert eng.kind_prior is False
    hits = eng.search("widget v1", k=10, repo="r")
    assert abs(hits[0].score - 1.0 / (RRF_K + 1)) < 1e-9


def test_rrf_tie_broken_by_kind_tier() -> None:
    # a: lex-only#1(note, T4);b: sem-only#1(reference, T1)。等权下 RRF 同分,
    # prior 把 T1 排 T4 前(issue 原文「reference 排 ephemeral 前」)。
    lex = _FakeLex([_lex("a")], kinds={"a": "note", "b": "reference"})
    eng = HybridEngine(lexical=lex, semantic=_FakeSem([_sem("b")]), kind_prior=True)
    hits = eng.search("widget v1", k=10, repo="r")
    assert [h.object_key for h in hits] == ["b", "a"]
    assert (
        abs(hits[0].score - (1.0 / (RRF_K + 1) + KIND_PRIOR_WEIGHT / (RRF_K + 1)))
        < 1e-9
    )
    assert (
        abs(hits[1].score - (1.0 / (RRF_K + 1) + KIND_PRIOR_WEIGHT / (RRF_K + 4)))
        < 1e-9
    )


def test_prior_does_not_flip_strong_relevance() -> None:
    # a 双 lane 高位(note)对 b 单 lane(spec):prior 极差 ~8e-4 翻不动整条 lane 票。
    lex = _FakeLex([_lex("a"), _lex("b")], kinds={"a": "note", "b": "spec"})
    eng = HybridEngine(lexical=lex, semantic=_FakeSem([_sem("a")]), kind_prior=True)
    hits = eng.search("widget v1", k=10, repo="r")
    assert hits[0].object_key == "a"


def test_missing_kind_falls_back_t4() -> None:
    # sem-only hit 不在 lexical 在册表(legacy 形状过 stale gate)→ T4 兜底不炸。
    eng = HybridEngine(
        lexical=_FakeLex([]), semantic=_FakeSem([_sem("ghost")]), kind_prior=True
    )
    hits = eng.search("widget v1", k=10, repo="r")
    assert (
        abs(hits[0].score - (1.0 / (RRF_K + 1) + KIND_PRIOR_WEIGHT / (RRF_K + 4)))
        < 1e-9
    )


def test_plan_exposes_kind_prior() -> None:
    eng = HybridEngine(lexical=_FakeLex([]), semantic=_FakeSem([]), kind_prior=True)
    assert eng.plan("widget v1")["kind_prior"] is True
