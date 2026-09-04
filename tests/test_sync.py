"""sync 单测: 两级 reuse / unit-mode 断言 / prune 三守卫 / 单篇失败继续。

qdrant 用 FakeQdrant 内存替身(duck-type 同签名);embed 用 monkeypatch, 不触网。
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest

from memex.config import Settings
from memex.indexing.qdrant import Qdrant, QdrantError
from memex.indexing.sync import (
    EMBEDDING_PROFILE_ID,
    INDEX_PROFILE,
    PAYLOAD_INDEX_FIELDS,
    POINT_KIND,
    UNIT_MODE_WHOLE,
    SyncMode,
    build_payload,
    point_id,
    prune_retired_qdrant_points,
    sync_repo,
)

DIM = 8  # 测试用小维度(fake 不校验维度)

FM = """---
description: "一句话召回摘要"
keywords: [foo, bar]
kind: reference
---

# 标题在这里

正文内容。
"""


def _note(path: Path, fm: str = FM) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(fm, encoding="utf-8")


def _index(path: Path) -> None:
    _note(
        path, '---\ndescription: "home"\nkeywords: [idx]\nkind: index\n---\n\n# home\n'
    )


def _settings(**overrides: Any) -> Settings:
    base: dict[str, Any] = {
        "central_collection": "testcoll",
        "embed_batch_size": 8,
        "embedding_dimensions": DIM,
    }
    base.update(overrides)
    return Settings(**base)


def _fake_embed(texts: list[str], s: Any = None, **_kwargs: Any) -> list[list[float]]:
    return [[float(len(t) % 7) + 0.5] * DIM for t in texts]


def _boom_embed(texts: list[str], s: Any = None, **_kwargs: Any) -> list[list[float]]:
    raise AssertionError("embed 不应被调用")


# ---- FakeQdrant -------------------------------------------------------------


def _match_cond(cond: dict[str, Any], payload: dict[str, Any]) -> bool:
    key, val = cond["key"], cond["match"]["value"]
    pv = payload.get(key)
    if isinstance(pv, list):
        return val in pv
    return pv == val


def _match(flt: dict[str, Any] | None, payload: dict[str, Any]) -> bool:
    if not flt:
        return True
    must_ok = all(_match_cond(c, payload) for c in flt.get("must", []))
    not_ok = not any(_match_cond(c, payload) for c in flt.get("must_not", []))
    return must_ok and not_ok


class FakeQdrant(Qdrant):
    """内存替身: 同签名, 记录写操作。"""

    def __init__(self) -> None:
        super().__init__(_settings())
        self.collections: dict[str, dict[str, Any]] = {}
        self.write_ops: list[str] = []

    def seed(
        self, coll: str, pid: str, vector: list[float], payload: dict[str, Any]
    ) -> None:
        self.collections.setdefault(coll, {"points": {}, "indexes": []})
        self.collections[coll]["points"][pid] = {
            "id": pid,
            "vector": {"object": vector},
            "payload": payload,
        }

    def collection_exists(self, name: str) -> bool:
        return name in self.collections

    def create_collection(self, name: str, vector_name: str, dim: int) -> None:
        self.write_ops.append(f"create_collection:{name}")
        self.collections[name] = {"points": {}, "indexes": [], "dim": dim}

    def create_payload_index(
        self, name: str, field: str, schema: str = "keyword"
    ) -> None:
        self.write_ops.append(f"create_index:{field}")
        if field not in self.collections[name]["indexes"]:
            self.collections[name]["indexes"].append(field)

    def delete_collection(self, name: str) -> None:
        self.write_ops.append(f"delete_collection:{name}")
        self.collections.pop(name, None)

    def retrieve(
        self, collection: str, ids: list[str], with_vector: bool = False
    ) -> list[dict[str, Any]]:
        pts = self.collections.get(collection, {}).get("points", {})
        return [pts[i] for i in ids if i in pts]

    def scroll(
        self,
        collection: str,
        flt: dict[str, Any] | None = None,
        limit: int = 100,
        offset: Any = None,
        with_vector: bool = False,
    ) -> tuple[list[dict[str, Any]], Any]:
        pts = self.collections.get(collection, {}).get("points", {})
        hits = [p for p in pts.values() if _match(flt, p["payload"])]
        return hits[:limit], None

    def upsert(self, collection: str, points: list[dict[str, Any]]) -> None:
        self.write_ops.append(f"upsert:{len(points)}")
        for p in points:
            self.collections[collection]["points"][str(p["id"])] = p

    def overwrite_payload(
        self, collection: str, payload: dict[str, Any], ids: list[str]
    ) -> None:
        self.write_ops.append(f"overwrite_payload:{len(ids)}")
        for i in ids:
            self.collections[collection]["points"][i]["payload"] = payload

    def delete_points(self, collection: str, ids: list[str]) -> None:
        self.write_ops.append(f"delete:{len(ids)}")
        for i in ids:
            self.collections[collection]["points"].pop(i, None)


# ---- point_id / payload ------------------------------------------------------


def test_point_id_deterministic_and_mode_distinct() -> None:
    a = point_id("repo:d:x")
    assert a == point_id("repo:d:x")  # 确定性
    assert a != point_id("repo:d:y")
    assert a != point_id("repo:d:x", "chunk")  # unit-mode 编进派生(C6)


def test_build_payload_complete(tmp_path: Path) -> None:
    from memex.indexing.pipeline import compile_repo

    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    out = compile_repo("repo", tmp_path)
    doc = next(d for d in out.docs if d.source_path == "d/a.md")
    pl = build_payload(doc, "th123")
    assert pl["identity"] == doc.identity
    assert pl["domain_prefixes"] == ["d"]
    assert pl["index_profile"] == INDEX_PROFILE
    assert pl["embedding_profile"] == EMBEDDING_PROFILE_ID
    assert pl["point_kind"] == POINT_KIND
    assert pl["unit_mode"] == UNIT_MODE_WHOLE
    assert pl["text_hash"] == "th123"
    assert "commit_time" not in pl  # None 不写入


# ---- dry-run / apply 基本流 ---------------------------------------------------


def test_dry_run_writes_nothing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _boom_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    fake = FakeQdrant()
    _, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=False)
    )
    assert rep.dry_run
    assert len(rep.embedded) == 2  # INDEX + a 都计划 embed
    assert fake.write_ops == []  # 零写入, 含不建 collection
    assert "testcoll" not in fake.collections
    assert any("不存在" in n for n in rep.notes)


def test_apply_fresh_creates_collection_and_embeds(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    fake = FakeQdrant()
    _, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    assert not rep.failures
    assert len(rep.embedded) == 2
    coll = fake.collections["testcoll"]
    assert sorted(coll["indexes"]) == sorted(PAYLOAD_INDEX_FIELDS)
    assert len(coll["points"]) == 2
    pt = next(iter(coll["points"].values()))
    assert pt["payload"]["unit_mode"] == UNIT_MODE_WHOLE


def test_status_payload_is_written_only_when_explicit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(
        tmp_path / "d" / "unclassified.md",
        '---\ndescription: "待分类"\nkeywords: [unclassified]\n'
        'kind: note\nstatus: unclassified\n---\n\n# T\n\n正文。\n',
    )
    fake = FakeQdrant()
    _, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    assert not rep.failures
    point = next(
        p
        for p in fake.collections["testcoll"]["points"].values()
        if p["payload"].get("status") == "unclassified"
    )
    assert point["payload"]["status"] == "unclassified"


def test_apply_existing_collection_ensures_payload_indexes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    fake = FakeQdrant()
    fake.collections["testcoll"] = {"points": {}, "indexes": ["point_kind"], "dim": DIM}
    _, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    assert not rep.failures
    assert sorted(fake.collections["testcoll"]["indexes"]) == sorted(
        PAYLOAD_INDEX_FIELDS
    )
    assert "create_collection:testcoll" not in fake.write_ops


def test_progress_reports_long_phases_and_embed_timeout_hint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    progress: list[str] = []
    _, rep = sync_repo(
        "repo",
        tmp_path,
        client=FakeQdrant(),
        s=_settings(embed_timeout_secs=12),
        mode=SyncMode(apply=True),
        progress=progress.append,
    )
    assert not rep.failures
    assert any("compile start" in line for line in progress)
    assert any("planning doc actions" in line for line in progress)
    assert any("embedding batch 1/1" in line for line in progress)
    assert any("timeout=12s" in line for line in progress)
    assert any("KB_SEARCH_EMBED_TIMEOUT_SECS" in line for line in progress)


def test_rerun_unchanged_skips(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    fake = FakeQdrant()
    sync_repo("repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True))
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _boom_embed)
    _, rep2 = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    assert len(rep2.unchanged) == 2
    assert not rep2.embedded and not rep2.rekeyed and not rep2.payload_updated


# ---- 两级 reuse ----------------------------------------------------------------


def test_payload_drift_updates_payload_only(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    fake = FakeQdrant()
    out, _ = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    ident = next(d.identity for d in out.docs if d.source_path == "d/a.md")
    pid = point_id(ident)
    # 模拟 payload 漂移(如旧 commit_time);向量不该被动
    fake.collections["testcoll"]["points"][pid]["payload"]["commit_time"] = (
        "2020-01-01T00:00:00+08:00"
    )
    old_vec = fake.collections["testcoll"]["points"][pid]["vector"]
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _boom_embed)
    _, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    assert ident in rep.payload_updated
    new_pt = fake.collections["testcoll"]["points"][pid]
    assert "commit_time" not in new_pt["payload"]  # overwrite 删掉了陈旧 key
    assert new_pt["vector"] == old_vec  # 零 re-embed


def test_content_change_reembeds(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    fake = FakeQdrant()
    out, _ = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    ident = next(d.identity for d in out.docs if d.source_path == "d/a.md")
    _note(tmp_path / "d" / "a.md", FM.replace("正文内容。", "改写后的正文,长度不同。"))
    _, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    assert ident in rep.embedded  # 内容变 → 重 embed


def test_rekey_reuses_vector_on_move(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    fake = FakeQdrant()
    out, _ = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    old_ident = next(d.identity for d in out.docs if d.source_path == "d/a.md")
    old_vec = fake.collections["testcoll"]["points"][point_id(old_ident)]["vector"]
    # 移动文件 = identity 变;内容没变 → 复用向量, 零 embed
    (tmp_path / "d" / "a.md").rename(tmp_path / "d" / "moved.md")
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _boom_embed)
    out2, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    new_ident = next(d.identity for d in out2.docs if d.source_path == "d/moved.md")
    assert new_ident in rep.rekeyed
    assert (
        fake.collections["testcoll"]["points"][point_id(new_ident)]["vector"] == old_vec
    )
    assert old_ident in rep.pruned  # 旧点 1/2 ≤ 50% → 正常 prune


def test_rename_no_h1_note_rekeys_zero_embed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # M1 反例: 无 H1 → title 回退文件名;改名后 title 变, 但 embed_text 不含 title
    # → text_hash 不变 → level-② re-key 命中, 零 embed(boom 守门)。
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    no_h1 = '---\ndescription: "无标题 note"\nkeywords: [k]\n---\n\n正文没有 H1。\n'
    _note(tmp_path / "d" / "old-name.md", no_h1)
    fake = FakeQdrant()
    sync_repo("repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True))
    (tmp_path / "d" / "old-name.md").rename(tmp_path / "d" / "new-name.md")
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _boom_embed)
    _, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    assert len(rep.rekeyed) == 1
    assert not rep.embedded and not rep.failures
    repo = "repo"  # identity = registry name, 不取磁盘 basename
    assert rep.rekeyed == [f"{repo}:d:new-name"]
    assert rep.pruned == [f"{repo}:d:old-name"]


# ---- unit-mode 断言 -------------------------------------------------------------


def test_mode_mismatch_refused(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _boom_embed)
    _index(tmp_path / "d" / "INDEX.md")
    fake = FakeQdrant()
    fake.seed(
        "testcoll",
        "0" * 32,
        [0.1] * DIM,
        {
            "identity": "x:y:z",
            "point_kind": POINT_KIND,
            "index_profile": INDEX_PROFILE,
            "unit_mode": "chunk",
        },
    )
    _, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    assert rep.error is not None and "unit-mode" in rep.error
    assert "整库重建" in rep.error
    assert all(not op.startswith(("upsert", "delete")) for op in fake.write_ops)


# ---- prune 三守卫 ---------------------------------------------------------------


def _seed_ghosts(fake: FakeQdrant, repo: str, n: int) -> None:
    for i in range(n):
        fake.seed(
            "testcoll",
            f"{i:032d}",
            [0.2] * DIM,
            {
                "identity": f"{repo}:d:ghost{i}",
                "point_kind": POINT_KIND,
                "index_profile": INDEX_PROFILE,
                "unit_mode": UNIT_MODE_WHOLE,
                "embedding_profile": EMBEDDING_PROFILE_ID,
                "text_hash": f"gh{i}",
            },
        )


def test_mass_prune_guard_refuses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    fake = FakeQdrant()
    _seed_ghosts(fake, "repo", 4)  # canonical repo = registry name
    _, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    assert len(rep.prune_candidates) == 4  # 4/4 > 50%
    assert rep.prune_refused is not None and "--force" in rep.prune_refused
    # M2: 文案点明双份暂存状态(已写入 N 个新点、旧点未删)
    assert "已写入 2 个新点" in rep.prune_refused
    assert "旧点未删" in rep.prune_refused and "孤儿" in rep.prune_refused
    assert not rep.pruned
    assert len(fake.collections["testcoll"]["points"]) == 4 + 2  # ghost 未删, 新点已加


def test_mass_prune_refusal_dry_run_wording(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # M2 dry-run 措辞: 未写入 → "将写入"。
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _boom_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    fake = FakeQdrant()
    _seed_ghosts(fake, "repo", 4)
    _, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=False)
    )
    assert rep.prune_refused is not None
    assert "将写入 2 个新点" in rep.prune_refused


def test_force_allows_mass_prune(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    fake = FakeQdrant()
    _seed_ghosts(fake, "repo", 4)
    _, rep = sync_repo(
        "repo",
        tmp_path,
        client=fake,
        s=_settings(),
        mode=SyncMode(apply=True, force=True),
    )
    assert len(rep.pruned) == 4
    assert rep.prune_refused is None
    assert len(fake.collections["testcoll"]["points"]) == 2


def test_dry_run_prune_candidates_not_deleted(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    _note(tmp_path / "d" / "b.md")
    fake = FakeQdrant()
    sync_repo("repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True))
    (tmp_path / "d" / "b.md").unlink()
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _boom_embed)
    n_before = len(fake.collections["testcoll"]["points"])
    writes_before = len(fake.write_ops)
    _, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=False)
    )
    assert len(rep.prune_candidates) == 1
    assert not rep.pruned
    assert len(fake.collections["testcoll"]["points"]) == n_before
    assert len(fake.write_ops) == writes_before  # dry-run 零写


def test_prune_other_repo_untouched(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # per-repo 前缀守卫: 别仓的点绝不进 prune 候选。
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    fake = FakeQdrant()
    _seed_ghosts(fake, "other-repo", 3)
    _, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    assert rep.prune_candidates == []
    assert len(fake.collections["testcoll"]["points"]) == 3 + 2


# ---- 鲁棒性 ---------------------------------------------------------------------


class FailingUpsertQdrant(FakeQdrant):
    def __init__(self, fail_identity: str) -> None:
        super().__init__()
        self.fail_identity = fail_identity

    def upsert(self, collection: str, points: list[dict[str, Any]]) -> None:
        if any(p["payload"]["identity"] == self.fail_identity for p in points):
            raise QdrantError("simulated upsert failure")
        super().upsert(collection, points)


def test_single_doc_failure_continues(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    _note(tmp_path / "d" / "b.md")
    repo = "repo"  # identity = registry name, 不取磁盘 basename
    fake = FailingUpsertQdrant(f"{repo}:d:a")
    # batch=1 → 单篇单 upsert;a 失败不拖垮 INDEX/b
    _, rep = sync_repo(
        "repo",
        tmp_path,
        client=fake,
        s=_settings(embed_batch_size=1),
        mode=SyncMode(apply=True),
    )
    assert len(rep.failures) == 1
    assert rep.failures[0][0] == f"{repo}:d:a"
    assert len(rep.embedded) == 2  # INDEX + b 成功
    assert len(fake.collections["testcoll"]["points"]) == 2


def test_embed_failure_recorded_continues(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def _fail_embed(
        texts: list[str], s: Any = None, **_kwargs: Any
    ) -> list[list[float]]:
        raise OSError("embedding 服务挂了")

    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fail_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    fake = FakeQdrant()
    _, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    assert len(rep.failures) == 2
    assert not rep.embedded  # 失败的从 embedded 移走
    assert all("embed" in err for _, err in rep.failures)


def test_embed_batch_failure_falls_back_to_single_docs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[int] = []

    def _batch_fails(
        texts: list[str], s: Any = None, **_kwargs: Any
    ) -> list[list[float]]:
        calls.append(len(texts))
        if len(texts) > 1:
            raise OSError("gateway timeout")
        return _fake_embed(texts, s)

    monkeypatch.setattr("memex.indexing.sync.embed_texts", _batch_fails)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    _note(tmp_path / "d" / "b.md")
    fake = FakeQdrant()
    _, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    assert calls == [3, 1, 1, 1]
    assert not rep.failures
    assert len(rep.embedded) == 3
    assert len(fake.collections["testcoll"]["points"]) == 3
    assert any("逐篇重试" in n for n in rep.notes)


def test_sync_uses_sync_embedding_lane(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[dict[str, Any]] = []

    def _capture_embed(
        texts: list[str], s: Any = None, **kwargs: Any
    ) -> list[list[float]]:
        calls.append(kwargs)
        return _fake_embed(texts, s)

    monkeypatch.setattr("memex.indexing.sync.embed_texts", _capture_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    fake = FakeQdrant()
    _, rep = sync_repo(
        "repo",
        tmp_path,
        client=fake,
        s=_settings(embedding_url="https://gateway.test/embedding-query/v1/embeddings"),
        mode=SyncMode(apply=True),
    )

    assert not rep.failures
    assert {call["lane"] for call in calls} == {"sync"}
    assert {call["endpoint"] for call in calls} == {
        "https://gateway.test/embedding-sync/v1/embeddings"
    }


class DownQdrant(FakeQdrant):
    def collection_exists(self, name: str) -> bool:
        raise QdrantError("connection refused")


def test_qdrant_unreachable_reported(tmp_path: Path) -> None:
    _index(tmp_path / "d" / "INDEX.md")
    _, rep = sync_repo(
        "repo", tmp_path, client=DownQdrant(), s=_settings(), mode=SyncMode(apply=False)
    )
    assert rep.error is not None and "不可达" in rep.error


# ---- kind_explicit payload -------------------------------------------


def test_payload_includes_kind_explicit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    fake = FakeQdrant()
    out, _ = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    ident = next(d.identity for d in out.docs if d.source_path == "d/a.md")
    pt = fake.collections["testcoll"]["points"][point_id(ident)]
    assert pt["payload"]["kind_explicit"] is True


def test_kind_explicit_rollout_payload_update_not_embed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # 模拟 rollout: 存量点 = 旧 payload(无 kind_explicit, 旧 compiled_hash), text_hash
    # 不变 → 两级 reuse 第①级命中: 只 set_payload, 零 re-embed。
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _fake_embed)
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    fake = FakeQdrant()
    out, _ = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    ident = next(d.identity for d in out.docs if d.source_path == "d/a.md")
    pid = point_id(ident)
    pt = fake.collections["testcoll"]["points"][pid]
    del pt["payload"]["kind_explicit"]
    pt["payload"]["compiled_hash"] = "pre-rollout-hash"
    old_vec = pt["vector"]
    monkeypatch.setattr("memex.indexing.sync.embed_texts", _boom_embed)
    _, rep = sync_repo(
        "repo", tmp_path, client=fake, s=_settings(), mode=SyncMode(apply=True)
    )
    assert ident in rep.payload_updated
    assert not rep.embedded and not rep.rekeyed
    new_pt = fake.collections["testcoll"]["points"][pid]
    assert new_pt["payload"]["kind_explicit"] is True
    assert new_pt["vector"] == old_vec  # 零 re-embed


# ---- prune_retired_qdrant_points -------------------------


def _seed_idy(fake: FakeQdrant, coll: str, specs: list[tuple[str, str]]) -> None:
    """预置点: (point_id, identity)。"""
    for pid, idy in specs:
        fake.seed(coll, pid, [0.0] * DIM, {"identity": idy})


def test_retire_qdrant_deletes_inactive_repo_points() -> None:
    fake = FakeQdrant()
    _seed_idy(
        fake,
        "testcoll",
        [
            ("1", "project-kb:d:a"),
            ("2", "project-kb:d:b"),
            ("3", "project-a:d:a"),  # 退役: 旧 leaf
            ("4", "tracker:d:x"),  # 退役: 旧 leaf
            ("5", "rhizome:d:y"),
        ],
    )
    r = prune_retired_qdrant_points(
        fake, "testcoll", {"project-kb", "rhizome", "tracker-kb"}, apply=True
    )
    assert r.retired_repos == ["project-a", "tracker"]
    assert r.point_count == 2
    assert r.deleted
    assert set(fake.collections["testcoll"]["points"]) == {"1", "2", "5"}


def test_retire_qdrant_dry_run_no_delete() -> None:
    fake = FakeQdrant()
    _seed_idy(fake, "testcoll", [("1", "project-kb:d:a"), ("2", "project-a:d:a")])
    r = prune_retired_qdrant_points(fake, "testcoll", {"project-kb"}, apply=False)
    assert r.point_count == 1
    assert not r.deleted
    assert set(fake.collections["testcoll"]["points"]) == {"1", "2"}


def test_retire_qdrant_mass_guard_refuses_then_force() -> None:
    fake = FakeQdrant()
    _seed_idy(
        fake,
        "testcoll",
        [
            ("1", "old:d:a"),
            ("2", "old:d:b"),
            ("3", "old:d:c"),
            ("4", "project-kb:d:x"),
        ],
    )  # 3 退役 / 4 总 = 75% > 50%
    r = prune_retired_qdrant_points(fake, "testcoll", {"project-kb"}, apply=True)
    assert r.refused is not None
    assert not r.deleted
    assert set(fake.collections["testcoll"]["points"]) == {"1", "2", "3", "4"}
    r2 = prune_retired_qdrant_points(
        fake, "testcoll", {"project-kb"}, apply=True, force=True
    )
    assert r2.deleted
    assert r2.point_count == 3
    assert set(fake.collections["testcoll"]["points"]) == {"4"}


def test_retire_qdrant_missing_collection_noop() -> None:
    fake = FakeQdrant()
    r = prune_retired_qdrant_points(fake, "nope", {"project-kb"}, apply=True)
    assert r.point_count == 0
    assert not r.deleted
    assert not r.retired_repos
