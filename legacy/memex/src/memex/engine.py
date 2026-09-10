"""多仓 lexical 引擎:从 registry 加载各源仓 → per-repo 索引 → 检索。

默认**不按 object_type 过滤**(类型降 prior;首切先做到「类型不再是硬闸」,
prior 加权留后续切片)。检索默认跨所有 active 源仓合并;`repo=` 收窄到单仓。
"""

from __future__ import annotations

from memex.artifacts import load_artifacts
from memex.config import Settings, settings
from memex.facets import Facets
from memex.lexical import Hit, RepoIndex
from memex.registry import Source, active_sources


class Engine:
    def __init__(
        self, sources: list[Source] | None = None, s: Settings = settings
    ) -> None:
        self.repos: dict[str, RepoIndex] = {}
        if sources is None and s.read_from_central:
            # 双源 flag: 读 compiled 目录(kb-note-v1), 不碰 .legacy-index artifact。
            from memex.compiled import load_compiled_corpus

            for name, docs in load_compiled_corpus(s).items():
                self.repos[name] = RepoIndex(name, docs)
            return
        for src in sources if sources is not None else active_sources():
            docs = load_artifacts(src.artifacts_dir)
            if docs:
                self.repos[src.name] = RepoIndex(src.name, docs)

    def search(
        self,
        query: str,
        k: int = 10,
        repo: str | None = None,
        facets: Facets | None = None,
    ) -> list[Hit]:
        if repo is not None:
            if repo not in self.repos:
                raise KeyError(
                    f"未知/未索引的源仓: {repo}(active: {sorted(self.repos)})"
                )
            return self.repos[repo].search(query, k, facets=facets)
        # 跨仓:各仓取 top-k → 按原始 BM25 分合并(v0 简单合并,非跨仓校准)。
        merged: list[Hit] = []
        for idx in self.repos.values():
            merged.extend(idx.search(query, k, facets=facets))
        merged.sort(key=lambda h: h.score, reverse=True)
        return merged[:k]
