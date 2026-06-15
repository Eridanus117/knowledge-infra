"""读侧一致性 doctor: 中央 collection 的点 ↔ 盘上 compiled 文件对账。

向量在、compiled 文件不在盘 = compiled_doc_missing 孤儿点 → recall 读路径把
该 hit gate 出 semantic 池、降级 lexical(见 health.stale_drop_reason)。读侧只在
查询时被动发现, 写侧此前零巡检。本 doctor 是写侧 backstop: 扫一遍中央 collection,
有孤儿 exit≠0, 由 Pharos CommandCheck cadence 抓出来 → session-start 露出 → agent
排查修(源在则 sync --apply 重建 / 源已删则 prune 点 / frontmatter 被剥则补回)。

纯只读: 不动 qdrant、不动盘。孤儿如何产生(prune 路径分叉 / 仓脱离 sync 覆盖)是另一
回事; 本 doctor 只负责"把问题暴露出来", 不自动修。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from memex.indexing.compile import safe_filename
from memex.indexing.qdrant import Qdrant, QdrantError
from memex.indexing.sync import INDEX_PROFILE, POINT_KIND

_SCROLL_PAGE = 256

# 修法提示(stderr 摘要 + stdout 全量都用): 三种成因 → 三种处置。
FIX_HINT = (
    "修: 源在→`memex-sync sync --repo <仓>=<路径> --apply` 重建; "
    "源已删→prune 该点; frontmatter 被剥→补回后重 sync"
)


@dataclass(frozen=True)
class OrphanPoint:
    """点在中央 collection、但盘上 compiled 文件缺(= compiled_doc_missing)。"""

    identity: str
    repo: str
    source_path: str | None
    expected_path: str


@dataclass
class ConsistencyReport:
    """中央 collection 点 ↔ compiled 文件对账结果。"""

    collection: str
    total_points: int = 0
    orphans: list[OrphanPoint] = field(default_factory=list)
    error: str | None = None  # qdrant 不可达 / collection 不存在等(无法对账)

    @property
    def healthy(self) -> bool:
        return self.error is None and not self.orphans

    def summary(self) -> str:
        if self.error:
            return f"compiled-consistency: ERROR — {self.error}"
        if not self.orphans:
            return f"compiled-consistency OK: {self.total_points} 个受管点均有盘上 compiled"
        return (
            f"compiled_doc_missing: {len(self.orphans)} 个孤儿点 / 共 {self.total_points} 点"
            " (向量在、compiled 文件缺 → recall semantic 降级 lexical)"
        )

    def render(self) -> str:
        """全量明细(stdout: 人/agent 直接跑时看)。"""
        lines = [self.summary()]
        if self.orphans:
            for o in self.orphans:
                lines.append(
                    f"  - {o.identity} (src={o.source_path}) → 缺 {o.expected_path}"
                )
            lines.append(FIX_HINT)
        return "\n".join(lines)

    def alert_detail(self, examples: int = 5) -> str:
        """Pharos CommandCheck 抓 stderr[:512] 当 detail: 数量+修法前置(防截断),
        孤儿例子在后(可被截断不影响处置)。"""
        if self.healthy:
            return self.summary()
        head = f"{self.summary()}\n{FIX_HINT}"
        if self.error:
            return head
        ex = "; ".join(o.identity for o in self.orphans[:examples])
        more = (
            ""
            if len(self.orphans) <= examples
            else f" …+{len(self.orphans) - examples}"
        )
        return f"{head}\n例: {ex}{more}"


def check_compiled_consistency(
    client: Qdrant, compiled_dir: Path, *, collection: str
) -> ConsistencyReport:
    """scroll 中央 collection 受管点(point_kind+index_profile 收窄, 同 sync 写口径),
    逐点核对 <compiled_dir>/<repo>/<safe(identity)>.json 是否在盘。"""
    report = ConsistencyReport(collection=collection)
    cdir = compiled_dir.expanduser()
    flt = {
        "must": [
            {"key": "point_kind", "match": {"value": POINT_KIND}},
            {"key": "index_profile", "match": {"value": INDEX_PROFILE}},
        ]
    }
    try:
        if not client.collection_exists(collection):
            report.error = f"中央 collection {collection} 不存在(--apply sync 未跑过?)"
            return report
        offset: Any = None
        while True:
            points, offset = client.scroll(
                collection, flt=flt, limit=_SCROLL_PAGE, offset=offset
            )
            for p in points:
                pl = p.get("payload") or {}
                ident = pl.get("identity")
                if not isinstance(ident, str) or ":" not in ident:
                    continue  # 非受管/畸形点不计入对账
                report.total_points += 1
                repo = ident.split(":", 1)[0]
                path = cdir / repo / safe_filename(ident)
                if not path.exists():
                    src = pl.get("source_path")
                    report.orphans.append(
                        OrphanPoint(
                            identity=ident,
                            repo=repo,
                            source_path=src if isinstance(src, str) else None,
                            expected_path=str(path),
                        )
                    )
            if offset is None:
                break
    except QdrantError as exc:
        report.error = f"qdrant 不可达/读失败: {exc}"
    return report
