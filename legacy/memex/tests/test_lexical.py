from memex.artifacts import Doc
from memex.lexical import RepoIndex

DOCS = [
    Doc(
        object_key="kb:doc:widget",
        title="文档模板配置",
        body="文档模板的配置规则与配置",
        path="kb/widget.md",
    ),
    Doc(
        object_key="kb:doc:address",
        title="寄件地址管理",
        body="常用寄件地址的增删改查",
        path="kb/address.md",
    ),
    Doc(
        object_key="kb:doc:warehouse",
        title="示例标题",
        body="示例文档正文",
        path="kb/warehouse.md",
    ),
]


def test_finds_relevant_doc_first() -> None:
    idx = RepoIndex("test", DOCS)
    hits = idx.search("文档模板", k=3)
    assert hits
    assert hits[0].object_key == "kb:doc:widget"


def test_title_boost_over_body() -> None:
    # "地址" 在 address 的 title、其它仅 body/无 → address 排第一(title 权重 5 主导)。
    idx = RepoIndex("test", DOCS)
    hits = idx.search("寄件地址", k=3)
    assert hits[0].object_key == "kb:doc:address"


def test_object_key_slug_match() -> None:
    # key 走 slug 分词,query 命中 key 段也能召回。
    idx = RepoIndex("test", DOCS)
    keys = [h.object_key for h in idx.search("warehouse", k=3)]
    assert "kb:doc:warehouse" in keys


def test_empty_corpus_rejected() -> None:
    import pytest

    with pytest.raises(ValueError):
        RepoIndex("empty", [])


def test_facet_mask_filters_results() -> None:
    # 同一 query 命中两篇, facet 收窄后只回匹配 kind 的那篇(全量打分后 mask, 不 underfill)。
    from memex.facets import Facets

    docs = [
        Doc(
            object_key="myrepo:decisions/widget.md",
            title="文档模板决策",
            body="文档模板的配置决策",
            path="decisions/widget.md",
            kind="decision",
            domain_prefixes=("decisions",),
            keywords=("widget",),
        ),
        Doc(
            object_key="myrepo:runbook/widget.md",
            title="文档模板排查",
            body="文档模板的排查手册",
            path="runbook/widget.md",
            kind="runbook",
            domain_prefixes=("runbook",),
            keywords=("widget",),
        ),
    ]
    idx = RepoIndex("myrepo", docs)
    assert len(idx.search("文档模板", k=5)) == 2
    hits = idx.search("文档模板", k=5, facets=Facets(kind="decision"))
    assert [h.object_key for h in hits] == ["myrepo:decisions/widget.md"]
    assert idx.search("文档模板", k=5, facets=Facets(domain="docs")) == []
