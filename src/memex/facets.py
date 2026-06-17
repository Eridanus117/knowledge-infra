"""Facet 收窄:domain 前缀 / kind / tag 的统一口径。

归一只在 __post_init__ 一处,qdrant filter 与 lexical mask 消费同一值——
防「尾斜杠两 lane 分叉」。repo 不是 facet(检索维度 = domain/kind/内容,仓是物理细节);
敏感源不单独 carve(统一进中央 collection)。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any


def _norm(v: str | None) -> str | None:
    v = (v or "").strip().strip("/")
    return v or None


@dataclass(frozen=True)
class Facets:
    domain: str | None = (
        None  # 域路径前缀(INDEX.md 域链), match payload.domain_prefixes
    )
    kind: str | None = None
    tag: str | None = None  # match payload.keywords

    def __post_init__(self) -> None:
        object.__setattr__(self, "domain", _norm(self.domain))
        object.__setattr__(self, "kind", _norm(self.kind))
        # tag 与写侧 keywords 同口径 case-fold,消除 'PM'/'pm' 漂移;
        # domain/kind 共用 _norm 不动,只收 tag 这一维。
        tag = _norm(self.tag)
        object.__setattr__(self, "tag", tag.casefold() if tag else None)

    def __bool__(self) -> bool:
        return any((self.domain, self.kind, self.tag))

    def qdrant_must(self) -> list[dict[str, Any]]:
        """中央 collection 的 server-side 条件(domain_prefixes/kind 有 payload index)。

        domain 前缀语义靠写路径的累进数组(`a/b` → ["a","a/b"]):对数组做精确
        match 即「按前缀收窄」,且 `foo` 不误伤 `foo-private`。
        """
        out: list[dict[str, Any]] = []
        if self.domain:
            out.append({"key": "domain_prefixes", "match": {"value": self.domain}})
        if self.kind:
            out.append({"key": "kind", "match": {"value": self.kind}})
        if self.tag:
            out.append({"key": "keywords", "match": {"value": self.tag}})
        return out

    def matches_doc(self, doc: Any) -> bool:
        """lexical lane 的同口径过滤(doc 带 domain_prefixes/kind/keywords)。"""
        if self.domain and self.domain not in (
            getattr(doc, "domain_prefixes", ()) or ()
        ):
            return False
        if self.kind and self.kind != getattr(doc, "kind", ""):
            return False
        return not (self.tag and self.tag not in (getattr(doc, "keywords", ()) or ()))
