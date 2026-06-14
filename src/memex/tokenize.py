"""中文分词(lexical lane)。

jieba `cut_for_search`(细粒度重叠子词,HMM off)
+ 后置丢长 token(>40)+ LowerCaser。
"""

from __future__ import annotations

import re

import jieba

jieba.setLogLevel(60)

# 丢长度 >40 的 token(对应 Tantivy RemoveLongFilter 语义)。
_MAX_TOKEN_LEN = 40
# raw 字段(object_key / path)的 slug 切分:在 :/_-. 处断词。
_SLUG_SPLIT = re.compile(r"[:/_\-.]+")


def tokenize(text: str) -> list[str]:
    """jieba cut_for_search(HMM off) + lowercase + 丢 >40。"""
    if not text:
        return []
    out: list[str] = []
    for raw in jieba.cut_for_search(text, HMM=False):
        t = raw.strip().lower()
        if t and len(t) <= _MAX_TOKEN_LEN and not t.isspace():
            out.append(t)
    return out


def slugify(key: str) -> str:
    """object_key / path → 以空格分隔的 slug(再喂 tokenize)。"""
    return _SLUG_SPLIT.sub(" ", key)
