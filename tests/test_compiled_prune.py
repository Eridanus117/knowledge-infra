"""compiled 目录 stale 产物清理(域退出 INDEX 链后产物随之消失)。

tmp_path 造仓 + 独立 compiled 目录;不碰 qdrant(纯文件级)。
"""

from __future__ import annotations

from pathlib import Path

from memex.indexing.pipeline import compile_repo, persist, prune_stale_compiled

FM = """---
description: "一句话召回摘要"
keywords: [foo, bar]
kind: reference
---

# 标题
"""

INDEX_FM = """---
description: "domain home"
keywords: [home]
kind: index
---

# home
"""


def _mk_repo(root: Path) -> None:
    """两域仓: a(2 note) + b(1 note), 各带 INDEX。"""
    for domain, notes in {"a": ["x", "y"], "b": ["z"]}.items():
        d = root / domain
        d.mkdir(parents=True)
        (d / "INDEX.md").write_text(INDEX_FM, encoding="utf-8")
        for n in notes:
            (d / f"{n}.md").write_text(FM, encoding="utf-8")


def _compile_and_persist(root: Path, compiled: Path) -> str:
    out = compile_repo("repo", root)
    assert not out.report.error and not out.report.duplicate_error
    persist(out.docs, compiled, out.canonical_repo)
    return out.canonical_repo


def test_domain_exit_prunes_compiled(tmp_path: Path) -> None:
    """域退出 INDEX 链后, compiled 产物随之消失(<50%, 守卫不触发)。"""
    root, compiled = tmp_path / "repo", tmp_path / "compiled"
    _mk_repo(root)
    repo = _compile_and_persist(root, compiled)
    assert len(list((compiled / repo).glob("*.json"))) == 5  # 2 INDEX + 3 note

    (root / "b" / "INDEX.md").unlink()  # 域 b 退出索引
    out = compile_repo("repo", root)
    pr = prune_stale_compiled(out.docs, compiled, repo, apply=True)
    assert pr.deleted and pr.refused is None
    assert all("__b__" in name or name.endswith("__b.json") for name in pr.stale)
    remaining = {p.name for p in (compiled / repo).glob("*.json")}
    assert not any("__b__" in n or n.endswith("__b.json") for n in remaining)
    assert any("__a__" in n for n in remaining)  # 域 a 不受影响


def test_mass_delete_guard_refuses_then_force(tmp_path: Path) -> None:
    """全仓退出(stale >50%)→ 守卫拒绝零删除;--force 放行。"""
    root, compiled = tmp_path / "repo", tmp_path / "compiled"
    _mk_repo(root)
    repo = _compile_and_persist(root, compiled)
    before = len(list((compiled / repo).glob("*.json")))

    for index in root.rglob("INDEX.md"):
        index.unlink()  # 全部域退出 → 0 docs
    out = compile_repo("repo", root)
    assert out.docs == []

    pr = prune_stale_compiled(out.docs, compiled, repo, apply=True)
    assert pr.refused is not None and not pr.deleted
    assert len(list((compiled / repo).glob("*.json"))) == before  # 零删除

    pr = prune_stale_compiled(out.docs, compiled, repo, apply=True, force=True)
    assert pr.deleted and len(pr.stale) == before
    assert list((compiled / repo).glob("*.json")) == []


def test_dry_run_reports_without_deleting(tmp_path: Path) -> None:
    root, compiled = tmp_path / "repo", tmp_path / "compiled"
    _mk_repo(root)
    repo = _compile_and_persist(root, compiled)
    (root / "b" / "INDEX.md").unlink()
    out = compile_repo("repo", root)

    before = len(list((compiled / repo).glob("*.json")))
    pr = prune_stale_compiled(out.docs, compiled, repo, apply=False)
    assert pr.stale and not pr.deleted and pr.refused is None
    assert len(list((compiled / repo).glob("*.json"))) == before


def test_other_repo_dir_untouched(tmp_path: Path) -> None:
    """按仓子目录收窄, 他仓产物不在 stale 判定范围。"""
    root, compiled = tmp_path / "repo", tmp_path / "compiled"
    _mk_repo(root)
    repo = _compile_and_persist(root, compiled)
    other = compiled / "other-repo"
    other.mkdir()
    (other / "other-repo__d__n.json").write_text("{}", encoding="utf-8")

    out = compile_repo("repo", root)
    pr = prune_stale_compiled(out.docs, compiled, repo, apply=True, force=True)
    assert pr.stale == []
    assert (other / "other-repo__d__n.json").exists()


def test_no_compiled_dir_noop(tmp_path: Path) -> None:
    root = tmp_path / "repo"
    _mk_repo(root)
    out = compile_repo("repo", root)
    pr = prune_stale_compiled(
        out.docs, tmp_path / "absent", out.canonical_repo, apply=True
    )
    assert pr.stale == [] and not pr.deleted and pr.refused is None
