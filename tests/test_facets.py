from memex.artifacts import Doc
from memex.facets import Facets

DOC = Doc(
    object_key="myrepo:decisions/adr-004.md",
    title="DOC-004",
    body="中央 collection",
    path="decisions/adr-004.md",
    kind="decision",
    domain_prefixes=("decisions", "decisions/kb"),
    keywords=("kb", "qdrant"),
)


def test_normalization_strips_slashes_and_blanks() -> None:
    # 尾斜杠两 lane 分叉 → 归一收在构造一处。
    f = Facets(domain="decisions/", kind="  ", tag=None)
    assert f.domain == "decisions"
    assert f.kind is None
    assert bool(f)
    assert not Facets()
    assert not Facets(domain="", kind=" ", tag="/")


def test_qdrant_must_conditions() -> None:
    must = Facets(domain="decisions", kind="decision", tag="kb").qdrant_must()
    assert {"key": "domain_prefixes", "match": {"value": "decisions"}} in must
    assert {"key": "kind", "match": {"value": "decision"}} in must
    assert {"key": "keywords", "match": {"value": "kb"}} in must
    assert Facets(kind="decision").qdrant_must() == [
        {"key": "kind", "match": {"value": "decision"}}
    ]


def test_matches_doc_domain_prefix_semantics() -> None:
    # domain_prefixes 是累进数组 → 父域前缀命中, 兄弟域/更深无关域不命中。
    assert Facets(domain="decisions").matches_doc(DOC)
    assert Facets(domain="decisions/kb").matches_doc(DOC)
    assert not Facets(domain="decision").matches_doc(DOC)  # 非前缀段, 不误伤
    assert not Facets(domain="docs").matches_doc(DOC)


def test_matches_doc_kind_and_tag() -> None:
    assert Facets(kind="decision", tag="qdrant").matches_doc(DOC)
    assert not Facets(kind="note").matches_doc(DOC)
    assert not Facets(tag="missing").matches_doc(DOC)
    # legacy doc(facet 字段空)在任何 facet 下都不命中。
    legacy = Doc(object_key="k", title="t", body="b", path="p")
    assert not Facets(kind="decision").matches_doc(legacy)


def test_tag_casefold_eliminates_case_drift() -> None:
    # tag 与写侧 keywords 同口径 casefold,'KB'/'kb' 不再分叉。
    assert Facets(tag="KB").tag == "kb"
    assert Facets(tag="KB").matches_doc(DOC)  # DOC.keywords 含 "kb"
    assert {"key": "keywords", "match": {"value": "pm"}} in Facets(
        tag="PM"
    ).qdrant_must()
    # 最小爆炸半径:domain/kind 共用 _norm,仍只 strip 不 casefold。
    assert Facets(domain="Decisions").domain == "Decisions"
    assert Facets(kind="Decision").kind == "Decision"
