from memex.tokenize import slugify, tokenize


def test_chinese_cut_for_search_overlapping() -> None:
    # cut_for_search 出细粒度重叠子词,中文不空、已 lowercase。
    toks = tokenize("文档模板配置")
    assert toks
    assert all(t == t.lower() for t in toks)
    assert "文档" in toks


def test_drops_over_40_and_blank() -> None:
    long = "a" * 41
    assert long not in tokenize(long + " kb")
    assert tokenize("   ") == []
    assert tokenize("") == []


def test_slugify_splits_key_delimiters() -> None:
    assert slugify("kb:doc:user-guide/use_cases.md").split() == [
        "kb",
        "doc",
        "user",
        "guide",
        "use",
        "cases",
        "md",
    ]
