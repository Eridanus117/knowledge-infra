from memex.planner import (
    ascii_identifier_count,
    cjk_char_count,
    code_token_count,
    is_strongly_anchored,
    is_zh_low_anchor,
)


def test_pure_chinese_is_low_anchor() -> None:
    assert is_zh_low_anchor("文档模板配置")
    assert cjk_char_count("文档模板配置") == 6


def test_ascii_token_makes_it_anchored() -> None:
    # 含 ascii identifier(v1 / parseConfig)→ Mixed, 非低锚。
    assert not is_zh_low_anchor("文档模板 v1 验收")
    assert not is_zh_low_anchor("V1 parseConfig 直连")
    assert ascii_identifier_count("文档模板 v1 验收") == 1


def test_pure_ascii_not_low_anchor() -> None:
    assert not is_zh_low_anchor("document template")


def test_punctuation_is_not_ascii_identifier() -> None:
    # 纯中文带标点仍是低锚(标点不是 identifier start)。
    assert is_zh_low_anchor("文档模板,配置规则。")


def test_code_token_detection() -> None:
    # 数字 / 下划线 / camelCase / ALL_CAPS 算代码符号;纯小写英文词不算。
    assert code_token_count("object_id uuid_v5 CHUNKED_EMBEDDING parseConfig") == 4
    assert code_token_count("document template config") == 0
    assert code_token_count("文档模板配置") == 0


def test_strongly_anchored() -> None:
    # 代码符号密集的术语 query → 强锚定;中文/普通英文 query → 否。
    assert is_strongly_anchored(
        "point ID 算法 whole 裸 object_id chunk uuid_v5 CHUNKED_EMBEDDING"
    )
    assert not is_strongly_anchored("文档模板配置")
    assert not is_strongly_anchored("document template config")
    assert not is_strongly_anchored("文档 v1 验收")  # 仅 1 代码 token,不够阈值
