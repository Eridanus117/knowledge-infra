"""Per-repo 4 字段加权 BM25。

字段 boost title5/body1/object_key2/path2 + 每字段独立 BM25 的加权求和
(不是 cross-field BM25F)。object_key/path 走 slug 分词,其余走 jieba。
"""

from __future__ import annotations

from dataclasses import dataclass

import bm25s
import numpy as np

from memex.artifacts import Doc
from memex.facets import Facets
from memex.tokenize import slugify, tokenize

# 4 字段加权求和 boost(PoC 实测形态)。
FIELD_BOOST: dict[str, float] = {
    "title": 5.0,
    "body": 1.0,
    "object_key": 2.0,
    "path": 2.0,
}
# raw 字段:不分自然语言,走 slug 切分。
_RAW_FIELDS = ("object_key", "path")


@dataclass(frozen=True)
class Hit:
    object_key: str
    score: float
    title: str
    path: str
    repo: str


def _field_tokens(field: str, doc: Doc) -> list[str]:
    val = getattr(doc, field)
    return tokenize(slugify(val)) if field in _RAW_FIELDS else tokenize(val)


class RepoIndex:
    """单仓 4 字段 BM25,加权求和打分。"""

    def __init__(self, name: str, docs: list[Doc]) -> None:
        if not docs:
            raise ValueError(f"RepoIndex({name}): 空语料")
        self.name = name
        self.docs = docs
        self.n = len(docs)
        self._field_idx: dict[str, bm25s.BM25] = {}
        for field in FIELD_BOOST:
            corpus = [_field_tokens(field, d) for d in docs]
            r = bm25s.BM25(method="lucene")
            r.index(corpus, show_progress=False)
            self._field_idx[field] = r

    def _field_scores(self, field: str, q_tokens: list[str]) -> np.ndarray:
        vec = np.zeros(self.n, dtype=np.float64)
        if not q_tokens:
            return vec
        res, sc = self._field_idx[field].retrieve(
            [q_tokens], k=self.n, show_progress=False
        )
        for doc_i, s in zip(res[0], sc[0], strict=False):
            vec[doc_i] = s
        return vec

    def search(
        self, query: str, k: int = 10, facets: Facets | None = None
    ) -> list[Hit]:
        q_nat = tokenize(query)
        q_raw = tokenize(slugify(query))
        total = np.zeros(self.n, dtype=np.float64)
        for field, weight in FIELD_BOOST.items():
            q = q_raw if field in _RAW_FIELDS else q_nat
            total += weight * self._field_scores(field, q)
        if facets:
            # 全量打分后做 facet mask → top-k 精确, 不会 underfill。
            mask = np.array([facets.matches_doc(d) for d in self.docs], dtype=bool)
            total = np.where(mask, total, 0.0)
        order = np.argsort(-total)[:k]
        return [
            Hit(
                object_key=self.docs[i].object_key,
                score=float(total[i]),
                title=self.docs[i].title,
                path=self.docs[i].path,
                repo=self.name,
            )
            for i in order
            if total[i] > 0
        ]
