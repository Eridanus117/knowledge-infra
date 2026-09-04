"""测试: registry 收敛 / compiled loader / 读路径双源 flag / orchestrator。

flag=False 的零行为变化由现有测试守卫(本文件不动它们的断言)。
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
from typer.testing import CliRunner

from memex import semantic
from memex.compiled import load_compiled_corpus, load_compiled_docs
from memex.config import Settings
from memex.engine import Engine
from memex.indexing.pipeline import compile_repo, persist
from memex.indexing.report import RepoReport
from memex.registry import DEFAULT_SOURCE_REPOS, load_source_repos

FM = """---
description: "文档模板的配置规则"
keywords: [文档模板, 配置]
kind: reference
---

# 文档模板

文档模板的配置规则与配置正文。
"""


def _note(path: Path, fm: str = FM) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(fm, encoding="utf-8")


def _index(path: Path) -> None:
    _note(
        path, '---\ndescription: "home"\nkeywords: [idx]\nkind: index\n---\n\n# home\n'
    )


def _build_compiled(tmp_path: Path) -> tuple[Path, str]:
    """tmp 源仓 → compile → persist 到 tmp compiled 目录;返回 (compiled_dir, repo)。"""
    src = tmp_path / "src"
    _index(src / "d" / "INDEX.md")
    _note(src / "d" / "widget.md")
    out = compile_repo("repo", src)
    compiled_dir = tmp_path / "compiled"
    persist(out.docs, compiled_dir, out.canonical_repo)
    return compiled_dir, out.canonical_repo


# ---- registry 收敛 -------------------------------------------------------------


def test_load_source_repos_from_toml(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    toml = tmp_path / "kb-sources.toml"
    toml.write_text(
        'source_root = "~/projects"\n'
        '[[source]]\nname = "alpha"\n'
        '[[source]]\nname = "beta"\npath = "~/elsewhere/beta"\n',
        encoding="utf-8",
    )
    monkeypatch.setenv("KB_SOURCES", str(toml))
    monkeypatch.delenv("KB_SOURCE_ROOT", raising=False)
    monkeypatch.delenv("KB_WORKSPACE_ROOT", raising=False)
    repos = load_source_repos()
    assert repos["alpha"] == Path("~/projects").expanduser() / "alpha"
    assert repos["beta"] == Path("~/elsewhere/beta").expanduser()


def test_load_source_repos_source_root_env_override(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    toml = tmp_path / "kb-sources.toml"
    toml.write_text(
        'source_root = "~/projects"\n[[source]]\nname = "alpha"\n',
        encoding="utf-8",
    )
    monkeypatch.setenv("KB_SOURCES", str(toml))
    monkeypatch.setenv("KB_SOURCE_ROOT", str(tmp_path / "override"))
    assert load_source_repos()["alpha"] == tmp_path / "override" / "alpha"


def test_load_source_repos_shared_workspace_root_contract(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    toml = tmp_path / "sources.toml"
    toml.write_text(
        f'workspace_root = "{tmp_path / "from-registry"}"\n'
        '[[source]]\nname = "alpha"\n',
        encoding="utf-8",
    )
    monkeypatch.setenv("KB_SOURCES", str(toml))
    monkeypatch.delenv("KB_SOURCE_ROOT", raising=False)
    monkeypatch.delenv("KB_WORKSPACE_ROOT", raising=False)
    assert load_source_repos()["alpha"] == tmp_path / "from-registry" / "alpha"

    monkeypatch.setenv("KB_WORKSPACE_ROOT", str(tmp_path / "from-env"))
    assert load_source_repos()["alpha"] == tmp_path / "from-env" / "alpha"


def test_load_source_repos_applies_sibling_local_overlay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    toml = tmp_path / "sources.toml"
    toml.write_text(
        f'workspace_root = "{tmp_path}"\n'
        '[[source]]\nname = "alpha"\nlegacy = true\n'
        '[[source]]\nname = "beta"\n',
        encoding="utf-8",
    )
    (tmp_path / "sources.local.toml").write_text(
        f'[[source]]\nname = "alpha"\npath = "{tmp_path / "local-alpha"}"\nlegacy = false\n'
        f'[[source]]\nname = "unknown"\npath = "{tmp_path / "ignored"}"\n',
        encoding="utf-8",
    )
    monkeypatch.setenv("KB_SOURCES", str(toml))
    monkeypatch.delenv("KB_SOURCE_ROOT", raising=False)
    monkeypatch.delenv("KB_WORKSPACE_ROOT", raising=False)

    from memex.registry import load_source_registry

    registry = load_source_registry()
    assert registry.repos == {
        "alpha": tmp_path / "local-alpha",
        "beta": tmp_path / "beta",
    }
    assert registry.legacy == frozenset()


def test_load_source_repos_fallback_on_missing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("KB_SOURCES", str(tmp_path / "no-such.toml"))
    assert load_source_repos() == DEFAULT_SOURCE_REPOS


def test_load_source_repos_default_env_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source_root = tmp_path / "sources"
    toml = source_root / "kb-sources.toml"
    toml.parent.mkdir(parents=True)
    toml.write_text(
        f'source_root = "{source_root}"\n[[source]]\nname = "alpha"\n',
        encoding="utf-8",
    )
    monkeypatch.setenv("KB_SOURCE_ROOT", str(source_root))
    monkeypatch.delenv("KB_SOURCES", raising=False)
    assert load_source_repos()["alpha"] == source_root / "alpha"


def test_load_source_repos_fallback_on_garbage(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    bad = tmp_path / "bad.toml"
    bad.write_text("not [ valid toml ===", encoding="utf-8")
    monkeypatch.setenv("KB_SOURCES", str(bad))
    assert load_source_repos() == DEFAULT_SOURCE_REPOS


def test_load_source_registry_degraded_flag(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from memex.registry import load_source_registry

    monkeypatch.setenv("KB_SOURCES", str(tmp_path / "no-such.toml"))
    reg = load_source_registry()
    assert reg.degraded and reg.reason is not None
    monkeypatch.setenv("KB_SOURCES", str(tmp_path / "ok.toml"))
    (tmp_path / "ok.toml").write_text('[[source]]\nname = "a"\n', encoding="utf-8")
    reg2 = load_source_registry()
    assert not reg2.degraded and reg2.reason is None


def test_load_source_repos_skips_illegal_and_duplicate_names(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # 含 / \ .. 或前导 ~ 的 name = 路径穿越风险, 跳过;重复 name 取首个。
    toml = tmp_path / "kb-sources.toml"
    toml.write_text(
        'source_root = "~/projects"\n'
        '[[source]]\nname = "good"\n'
        '[[source]]\nname = "../evil"\n'
        '[[source]]\nname = "a/b"\n'
        '[[source]]\nname = "c\\\\d"\n'
        '[[source]]\nname = "~tilde"\n'
        '[[source]]\nname = "good"\npath = "~/elsewhere/sneaky"\n',
        encoding="utf-8",
    )
    monkeypatch.setenv("KB_SOURCES", str(toml))
    monkeypatch.delenv("KB_SOURCE_ROOT", raising=False)
    monkeypatch.delenv("KB_WORKSPACE_ROOT", raising=False)
    repos = load_source_repos()
    assert set(repos) == {"good"}
    assert (
        repos["good"] == Path("~/projects").expanduser() / "good"
    )  # 首个 wins, sneaky 路径没生效


# ---- compiled loader -----------------------------------------------------------


def test_load_compiled_docs_fields(tmp_path: Path) -> None:
    compiled_dir, repo = _build_compiled(tmp_path)
    docs = load_compiled_docs(compiled_dir / repo)
    note = next(d for d in docs if d.object_key == f"{repo}:d:widget")
    assert note.title == "文档模板"
    assert note.path == "d/widget.md"
    # body = description + keywords + body_text(召回信号全进 lexical 索引)
    assert "配置规则" in note.body and "文档模板" in note.body


def test_load_compiled_corpus_by_repo(tmp_path: Path) -> None:
    compiled_dir, repo = _build_compiled(tmp_path)
    corpus = load_compiled_corpus(Settings(compiled_dir=compiled_dir))
    assert set(corpus) == {repo}
    assert len(corpus[repo]) == 2  # INDEX + widget


def test_load_compiled_corpus_missing_dir(tmp_path: Path) -> None:
    assert load_compiled_corpus(Settings(compiled_dir=tmp_path / "nope")) == {}


def test_load_compiled_skips_broken_json(tmp_path: Path) -> None:
    compiled_dir, repo = _build_compiled(tmp_path)
    (compiled_dir / repo / "zz-broken.json").write_text("{not json", encoding="utf-8")
    docs = load_compiled_docs(compiled_dir / repo)
    assert len(docs) == 2  # 损坏文件跳过, 不拖垮整仓


# ---- lexical flag=True ----------------------------------------------------------


def test_lexical_engine_reads_compiled_when_flag_on(tmp_path: Path) -> None:
    compiled_dir, repo = _build_compiled(tmp_path)
    eng = Engine(s=Settings(read_from_central=True, compiled_dir=compiled_dir))
    hits = eng.search("文档模板", k=3)
    assert hits
    assert hits[0].object_key == f"{repo}:d:widget"
    assert hits[0].repo == repo


def test_lexical_engine_flag_on_empty_compiled(tmp_path: Path) -> None:
    eng = Engine(s=Settings(read_from_central=True, compiled_dir=tmp_path / "empty"))
    assert eng.repos == {}


# ---- semantic central -----------------------------------------------------------


def _central_response() -> dict[str, Any]:
    return {
        "result": [
            {
                "score": 0.9,
                "payload": {"identity": "repoA:d:x", "source_path": "d/x.md"},
            },
            {
                "score": 0.8,
                "payload": {"identity": "repoA:d:x", "source_path": "d/x.md"},
            },
            {
                "score": 0.7,
                "payload": {"identity": "repoB:docs:y", "source_path": "docs/y.md"},
            },
        ]
    }


def test_search_central_filter_and_identity(monkeypatch: pytest.MonkeyPatch) -> None:
    captured: dict[str, Any] = {}

    def _fake_post(url: str, body: dict[str, Any], timeout: float) -> dict[str, Any]:
        captured["url"] = url
        captured["body"] = body
        return _central_response()

    monkeypatch.setattr(semantic, "_post_json", _fake_post)
    s = Settings(central_collection="kb_central_test")
    hits = semantic.search_central([0.0] * 3, k=10, s=s)
    assert "/collections/kb_central_test/points/search" in captured["url"]
    musts = {c["key"]: c["match"]["value"] for c in captured["body"]["filter"]["must"]}
    assert musts == {
        "point_kind": "note",
        "index_profile": "kb-central-v1",
        "embedding_profile": semantic.EMBEDDING_PROFILE_ID,
    }
    # identity 去重 + repo 取 identity 前缀
    assert [h.object_key for h in hits] == ["repoA:d:x", "repoB:docs:y"]
    assert hits[0].repo == "repoA" and hits[1].repo == "repoB"


def test_search_central_repo_prefix_narrowing(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(semantic, "_post_json", lambda *a, **k: _central_response())
    hits = semantic.search_central([0.0] * 3, k=10, repo="repoB", s=Settings())
    assert [h.object_key for h in hits] == ["repoB:docs:y"]


def test_semantic_engine_routes_to_central_when_flag_on(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(semantic, "_post_json", lambda *a, **k: _central_response())
    eng = semantic.SemanticEngine(s=Settings(read_from_central=True))
    assert eng.central and eng.collections == {}
    hits = eng.search_vec([0.0] * 3, k=5)
    assert hits[0].object_key == "repoA:d:x"


# ---- recall 富化 lookup 双源 ------------------------------------------------------


def test_doc_lookup_central_branch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    compiled_dir, repo = _build_compiled(tmp_path)
    from memex.recall import _doc_lookup
    from memex.recall import settings as recall_settings

    monkeypatch.setattr(recall_settings, "read_from_central", True)
    monkeypatch.setattr(recall_settings, "compiled_dir", compiled_dir)
    lookup = _doc_lookup(None)
    doc = lookup[(repo, f"{repo}:d:widget")]
    assert doc.title == "文档模板"
    assert _doc_lookup("不存在的仓") == {}


# ---- orchestrator (sync-all) ------------------------------------------------------


def _fake_sync_result(
    name: str,
    *,
    error: str | None = None,
    failures: int = 0,
    prune_refused: str | None = None,
):
    from memex.indexing.pipeline import CompileOutput
    from memex.indexing.sync import SyncReport

    rep = SyncReport(repo=name, collection="c", dry_run=True)
    rep.error = error
    rep.failures = [(f"{name}:d:f{i}", "boom") for i in range(failures)]
    rep.prune_refused = prune_refused
    c_out = CompileOutput(
        report=RepoReport(repo=name, repo_path="/x", indexed=3),
        docs=[],
        canonical_repo=name,
    )
    return c_out, rep


def _registry(
    repos: dict[str, Path],
    degraded: bool = False,
    reason: str | None = None,
    legacy: frozenset[str] = frozenset(),
):
    from memex.registry import SourceRegistry

    return SourceRegistry(repos=repos, degraded=degraded, reason=reason, legacy=legacy)


@pytest.fixture(autouse=True)
def _isolate_qdrant_retire_prune(monkeypatch: pytest.MonkeyPatch) -> None:
    """sync-all 编排测试不触碰真生产 qdrant 的退役清理 —— 这些测试验证编排/退出码;
    qdrant retire-prune 的逻辑由 test_sync.py 的 test_retire_qdrant_* 用 FakeQdrant 专测。
    不隔离则 prune_retired_qdrant_points 会连真 collection、按 test 小 registry 误判退役。"""
    from memex.indexing.sync import RetiredQdrantPrune

    monkeypatch.setattr(
        "memex.indexing.sync.prune_retired_qdrant_points",
        lambda *a, **k: RetiredQdrantPrune(retired_repos=[], point_count=0),
    )


def test_sync_all_continues_and_exits_nonzero(monkeypatch: pytest.MonkeyPatch) -> None:
    from memex.indexing import cli as sync_cli

    calls: list[str] = []

    def _fake_sync_repo(name: str, path: Path, **kw: Any):
        calls.append(name)
        if name == "bad":
            return _fake_sync_result(name, error="qdrant 不可达: boom")
        if name == "flaky":
            return _fake_sync_result(name, failures=2)
        return _fake_sync_result(name)

    monkeypatch.setattr(
        sync_cli,
        "load_source_registry",
        lambda: _registry({"good": Path("/g"), "bad": Path("/b"), "flaky": Path("/f")}),
    )
    monkeypatch.setattr("memex.indexing.sync.sync_repo", _fake_sync_repo)
    result = CliRunner().invoke(sync_cli.app, ["sync-all"])
    assert calls == ["good", "bad", "flaky"]  # 单仓失败不中断
    assert result.exit_code == 1  # 任一仓硬失败 = 1
    assert "sync-all 汇总" in result.stdout
    assert "总失败清单" in result.stdout
    assert "qdrant 不可达" in result.stdout


def test_sync_all_green_exits_zero(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from memex.indexing import cli as sync_cli

    monkeypatch.setattr(
        sync_cli, "load_source_registry", lambda: _registry({"good": Path("/g")})
    )
    monkeypatch.setattr(
        "memex.indexing.sync.sync_repo",
        lambda name, path, **kw: _fake_sync_result(name),
    )
    # --out 隔离: 退役清理扫 out_dir, 不碰生产 compiled_dir。
    result = CliRunner().invoke(sync_cli.app, ["sync-all", "--out", str(tmp_path)])
    assert result.exit_code == 0, result.stdout
    assert ">>> sync-all good  (/g)" in result.stdout
    assert "sync-all 汇总" in result.stdout


def test_sync_all_qdrant_retire_error_reports_failure(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from memex.indexing import cli as sync_cli
    from memex.indexing.qdrant import QdrantError

    monkeypatch.setattr(
        sync_cli, "load_source_registry", lambda: _registry({"good": Path("/g")})
    )
    monkeypatch.setattr(
        "memex.indexing.sync.sync_repo",
        lambda name, path, **kw: _fake_sync_result(name),
    )

    def _raise_qdrant(*_a: Any, **_kw: Any):
        raise QdrantError("GET /collections/c: dns")

    monkeypatch.setattr(
        "memex.indexing.sync.prune_retired_qdrant_points", _raise_qdrant
    )
    result = CliRunner().invoke(sync_cli.app, ["sync-all", "--out", str(tmp_path)])
    assert result.exit_code == 1, result.stdout
    assert "retired-qdrant-prune ERROR" in result.stdout
    assert "总失败清单" in result.stdout
    assert "<retired-qdrant>" in result.stdout
    assert "sync-all 汇总" in result.stdout


def test_sync_all_passes_legacy_flag(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from memex.indexing import cli as sync_cli

    calls: list[tuple[str, bool]] = []

    def _fake_sync_repo(name: str, path: Path, **kw: Any):
        calls.append((name, bool(kw.get("legacy"))))
        return _fake_sync_result(name)

    monkeypatch.setattr(
        sync_cli,
        "load_source_registry",
        lambda: _registry(
            {"good": Path("/g"), "old": Path("/o")}, legacy=frozenset({"old"})
        ),
    )
    monkeypatch.setattr("memex.indexing.sync.sync_repo", _fake_sync_repo)
    # --out 隔离: 退役清理扫 out_dir, 不碰生产 compiled_dir。
    result = CliRunner().invoke(sync_cli.app, ["sync-all", "--out", str(tmp_path)])
    assert result.exit_code == 0, result.stdout
    assert calls == [("good", False), ("old", True)]


def test_sync_all_prune_refused_exits_two(monkeypatch: pytest.MonkeyPatch) -> None:
    # M-3: 无硬失败但有 prune 拒绝 → exit 2 + 汇总单列"需人工介入"。
    from memex.indexing import cli as sync_cli

    def _fake_sync_repo(name: str, path: Path, **kw: Any):
        if name == "stuck":
            return _fake_sync_result(name, prune_refused="待删 4 > 50%, 拒绝")
        return _fake_sync_result(name)

    monkeypatch.setattr(
        sync_cli,
        "load_source_registry",
        lambda: _registry({"good": Path("/g"), "stuck": Path("/s")}),
    )
    monkeypatch.setattr("memex.indexing.sync.sync_repo", _fake_sync_repo)
    result = CliRunner().invoke(sync_cli.app, ["sync-all"])
    assert result.exit_code == 2, result.stdout
    assert "需人工介入(--force)" in result.stdout
    assert "stuck" in result.stdout


def test_sync_all_hard_failure_beats_needs_force(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # M-3: 硬失败(1)优先于 prune 拒绝(2)。
    from memex.indexing import cli as sync_cli

    def _fake_sync_repo(name: str, path: Path, **kw: Any):
        if name == "bad":
            return _fake_sync_result(name, error="boom")
        return _fake_sync_result(name, prune_refused="拒绝")

    monkeypatch.setattr(
        sync_cli,
        "load_source_registry",
        lambda: _registry({"bad": Path("/b"), "stuck": Path("/s")}),
    )
    monkeypatch.setattr("memex.indexing.sync.sync_repo", _fake_sync_repo)
    result = CliRunner().invoke(sync_cli.app, ["sync-all"])
    assert result.exit_code == 1, result.stdout
    assert "需人工介入(--force)" in result.stdout  # 清单仍单列


def test_sync_cmd_prune_refused_exits_two(monkeypatch: pytest.MonkeyPatch) -> None:
    # M-3: 单仓 sync 同语义。
    from memex.indexing import cli as sync_cli

    monkeypatch.setattr(
        "memex.indexing.sync.sync_repo",
        lambda name, path, **kw: _fake_sync_result(name, prune_refused="拒绝"),
    )
    result = CliRunner().invoke(sync_cli.app, ["sync", "--repo", "x=/tmp/x"])
    assert result.exit_code == 2, result.stdout


def test_sync_all_degraded_registry_warns(monkeypatch: pytest.MonkeyPatch) -> None:
    # M-2: 降级要在运行输出可见, 不只埋日志。
    from memex.indexing import cli as sync_cli

    monkeypatch.setattr(
        sync_cli,
        "load_source_registry",
        lambda: _registry({"good": Path("/g")}, degraded=True, reason="no such file"),
    )
    monkeypatch.setattr(
        "memex.indexing.sync.sync_repo",
        lambda name, path, **kw: _fake_sync_result(name),
    )
    result = CliRunner().invoke(sync_cli.app, ["sync-all"])
    assert result.exit_code == 0, result.stdout
    assert "WARN: 源仓清单降级到内置默认" in result.stdout


def test_sync_cmd_degraded_registry_warns(monkeypatch: pytest.MonkeyPatch) -> None:
    from memex.indexing import cli as sync_cli

    monkeypatch.setattr(
        sync_cli,
        "load_source_registry",
        lambda: _registry({"good": Path("/g")}, degraded=True, reason="no such file"),
    )
    monkeypatch.setattr(
        "memex.indexing.sync.sync_repo",
        lambda name, path, **kw: _fake_sync_result(name),
    )
    result = CliRunner().invoke(sync_cli.app, ["sync"])
    assert result.exit_code == 0, result.stdout
    assert "WARN: 源仓清单降级到内置默认" in result.stdout


# ---- loader 携带 hash/kind_explicit ------------------------------------


def test_loader_carries_hashes_and_kind_explicit(tmp_path: Path) -> None:
    compiled_dir, repo = _build_compiled(tmp_path)
    docs = load_compiled_docs(compiled_dir / repo)
    d = next(x for x in docs if x.path == "d/widget.md")
    assert d.source_hash and d.compiled_hash  # stale gate 的对比基准
    assert d.kind_explicit is True


def test_loader_old_compiled_defaults_kind_explicit_true(tmp_path: Path) -> None:
    # 老产物(无 kind_explicit 字段)重编译前不产生假 loud。
    import json as _json

    compiled_dir, repo = _build_compiled(tmp_path)
    f = next((compiled_dir / repo).glob("*.json"))
    data = _json.loads(f.read_text(encoding="utf-8"))
    data.pop("kind_explicit", None)
    f.write_text(_json.dumps(data, ensure_ascii=False), encoding="utf-8")
    docs = load_compiled_docs(compiled_dir / repo)
    d = next(x for x in docs if x.object_key == data["identity"])
    assert d.kind_explicit is True


def test_search_central_extracts_text_hash_and_unit_mode(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # stale gate 的判定输入来自 payload, 抽取断链 gate 会静默 no-op。
    resp = {
        "result": [
            {
                "score": 0.9,
                "payload": {
                    "identity": "repoA:d:x",
                    "source_path": "d/x.md",
                    "source_hash": "s1",
                    "compiled_hash": "c1",
                    "text_hash": "t1",
                    "unit_mode": "whole",
                },
            }
        ]
    }
    monkeypatch.setattr(semantic, "_post_json", lambda *a, **k: resp)
    hits = semantic.search_central([0.0] * 3, k=10, s=Settings())
    assert hits[0].text_hash == "t1"
    assert hits[0].unit_mode == "whole"
    assert hits[0].source_hash == "s1" and hits[0].compiled_hash == "c1"


def test_network_error_raises_semantic_unavailable(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import urllib.error

    def _down(*a: Any, **k: Any) -> dict[str, Any]:
        raise urllib.error.URLError("connection refused")

    monkeypatch.setattr(semantic, "_post_json", _down)
    with pytest.raises(semantic.SemanticUnavailable):
        semantic.search_central([0.0] * 3, k=10, s=Settings())
    with pytest.raises(semantic.SemanticUnavailable):
        semantic.embed_texts(["a"], Settings())
