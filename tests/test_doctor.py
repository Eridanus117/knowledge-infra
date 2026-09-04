"""doctor 单测: 中央 collection 点 ↔ 盘上 compiled 对账。

qdrant 用最小内存替身(只需 collection_exists + scroll);compiled 文件用真
safe_filename 落到 tmp, 留一部分缺失模拟孤儿。
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from memex.indexing.compile import safe_filename
from memex.indexing.doctor import check_compiled_consistency
from memex.indexing.qdrant import Qdrant, QdrantError
from memex.indexing.sync import INDEX_PROFILE, POINT_KIND

COLL = "testcoll"


def _match(flt: dict[str, Any] | None, payload: dict[str, Any]) -> bool:
    if not flt:
        return True
    for c in flt.get("must", []):
        if payload.get(c["key"]) != c["match"]["value"]:
            return False
    return True


class FakeQdrant(Qdrant):
    """最小替身: 只实现 doctor 用到的 collection_exists + scroll。"""

    def __init__(self, points: list[dict[str, Any]], *, exists: bool = True) -> None:
        self._points = points
        self._exists = exists

    def collection_exists(self, name: str) -> bool:  # type: ignore[override]
        return self._exists

    def scroll(  # type: ignore[override]
        self,
        collection: str,
        flt: dict[str, Any] | None = None,
        limit: int = 100,
        offset: Any = None,
        with_vector: bool = False,
    ) -> tuple[list[dict[str, Any]], Any]:
        hits = [p for p in self._points if _match(flt, p.get("payload") or {})]
        return hits, None  # 一页返回, next=None


class BoomQdrant(Qdrant):
    """collection_exists 即抛 QdrantError(模拟 qdrant 不可达)。"""

    def __init__(self) -> None:
        pass

    def collection_exists(self, name: str) -> bool:  # type: ignore[override]
        raise QdrantError("connection refused")


def _pt(
    identity: str, *, managed: bool = True, source_path: str = "d/x.md"
) -> dict[str, Any]:
    pl: dict[str, Any] = {"identity": identity, "source_path": source_path}
    if managed:
        pl["point_kind"] = POINT_KIND
        pl["index_profile"] = INDEX_PROFILE
    return {"id": "pid", "payload": pl}


def _write_compiled(cdir: Path, identity: str) -> None:
    repo = identity.split(":", 1)[0]
    sub = cdir / repo
    sub.mkdir(parents=True, exist_ok=True)
    (sub / safe_filename(identity)).write_text("{}", encoding="utf-8")


def test_all_present_is_healthy(tmp_path: Path) -> None:
    idents = ["repo:d:a", "repo:d:b", "other:x:c"]
    for i in idents:
        _write_compiled(tmp_path, i)
    fake = FakeQdrant([_pt(i) for i in idents])
    rep = check_compiled_consistency(fake, tmp_path, collection=COLL)
    assert rep.healthy
    assert rep.total_points == 3
    assert rep.orphans == []
    assert "OK" in rep.summary()


def test_missing_compiled_flagged_as_orphan(tmp_path: Path) -> None:
    _write_compiled(tmp_path, "repo:d:a")  # a 在盘
    # b、c 的点在、文件缺 → 孤儿
    fake = FakeQdrant(
        [_pt("repo:d:a"), _pt("repo:d:b", source_path="d/b.md"), _pt("other:x:c")]
    )
    rep = check_compiled_consistency(fake, tmp_path, collection=COLL)
    assert not rep.healthy
    assert rep.total_points == 3
    got = sorted(o.identity for o in rep.orphans)
    assert got == ["other:x:c", "repo:d:b"]
    b = next(o for o in rep.orphans if o.identity == "repo:d:b")
    assert b.repo == "repo"
    assert b.source_path == "d/b.md"
    assert b.expected_path.endswith(safe_filename("repo:d:b"))
    assert "compiled_doc_missing" in rep.summary()


def test_unmanaged_points_ignored(tmp_path: Path) -> None:
    # 缺 point_kind/index_profile 的点不进对账(即便文件缺也不算孤儿)。
    fake = FakeQdrant([_pt("repo:d:legacy", managed=False)])
    rep = check_compiled_consistency(fake, tmp_path, collection=COLL)
    assert rep.healthy
    assert rep.total_points == 0


def test_malformed_identity_skipped(tmp_path: Path) -> None:
    fake = FakeQdrant(
        [
            _pt("no-colon-here"),
            {
                "id": "x",
                "payload": {"point_kind": POINT_KIND, "index_profile": INDEX_PROFILE},
            },
        ]
    )
    rep = check_compiled_consistency(fake, tmp_path, collection=COLL)
    assert rep.healthy
    assert rep.total_points == 0


def test_collection_absent_is_error(tmp_path: Path) -> None:
    fake = FakeQdrant([], exists=False)
    rep = check_compiled_consistency(fake, tmp_path, collection=COLL)
    assert not rep.healthy
    assert rep.error is not None and "不存在" in rep.error


def test_qdrant_unreachable_is_error(tmp_path: Path) -> None:
    rep = check_compiled_consistency(BoomQdrant(), tmp_path, collection=COLL)
    assert not rep.healthy
    assert rep.error is not None and "qdrant" in rep.error


def test_alert_detail_front_loads_fix(tmp_path: Path) -> None:
    fake = FakeQdrant([_pt(f"repo:d:n{i}") for i in range(20)])
    rep = check_compiled_consistency(fake, tmp_path, collection=COLL)
    detail = rep.alert_detail()
    # 数量 + 修法在前 512 字内(Pharos 截断点), 例子在后可截断。
    head = detail[:512]
    assert "20 个孤儿点" in head
    assert "修:" in head
    assert "…+15" in detail  # 默认只列 5 个例子 + 余量计数
