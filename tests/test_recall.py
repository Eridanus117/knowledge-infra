import json

from typer.testing import CliRunner

from memex.artifacts import Doc
from memex.cli import app
from memex.lexical import Hit

runner = CliRunner()


class _FakeLexical:
    def __init__(self, *_a, **_k) -> None:
        pass

    def search(self, text: str, k: int = 10, repo: str | None = None) -> list[Hit]:
        return [
            Hit(
                object_key="kb:doc:a",
                score=2.5,
                title="文档模板",
                path="kb/a.md",
                repo="myrepo",
            ),
            Hit(
                object_key="kb:doc:b",
                score=1.0,
                title="示例文档",
                path="kb/b.md",
                repo="myrepo",
            ),
        ]


def test_recall_json_contract(monkeypatch) -> None:
    monkeypatch.setattr("memex.engine.Engine", _FakeLexical)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})
    result = runner.invoke(
        app, ["recall", "文档", "--lane", "lexical", "--format", "json"]
    )
    assert result.exit_code == 0, result.stdout
    payload = json.loads(result.stdout)
    assert [h["object_key"] for h in payload["hits"]] == ["kb:doc:a", "kb:doc:b"]
    # title/path 富化(_doc_lookup 空 → 回落 hit 自带字段)。
    assert payload["hits"][0]["title"] == "文档模板"
    assert payload["hits"][0]["path"] == "kb/a.md"


def test_recall_respects_limit(monkeypatch) -> None:
    captured: dict[str, int] = {}

    class _Cap(_FakeLexical):
        def search(self, text: str, k: int = 10, repo: str | None = None) -> list[Hit]:
            captured["k"] = k
            return super().search(text, k, repo)

    monkeypatch.setattr("memex.engine.Engine", _Cap)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})
    result = runner.invoke(app, ["recall", "文档", "--lane", "lexical", "--limit", "3"])
    assert result.exit_code == 0, result.stdout
    assert captured["k"] == 3


def test_recall_text_output(monkeypatch) -> None:
    monkeypatch.setattr("memex.engine.Engine", _FakeLexical)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})
    result = runner.invoke(app, ["recall", "文档", "--lane", "lexical"])
    assert result.exit_code == 0, result.stdout
    assert "文档模板" in result.stdout
    assert "kb:doc:a" in result.stdout


def test_recall_threads_facets_to_engine(monkeypatch) -> None:
    from memex.facets import Facets

    captured: dict = {}

    class _Cap(_FakeLexical):
        def search(self, text, k=10, repo=None, facets=None):
            captured["facets"] = facets
            return super().search(text, k, repo)

    monkeypatch.setattr("memex.engine.Engine", _Cap)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})
    result = runner.invoke(
        app,
        [
            "recall",
            "文档",
            "--lane",
            "lexical",
            "--domain",
            "decisions/",
            "--kind",
            "decision",
        ],
    )
    assert result.exit_code == 0, result.stdout
    assert captured["facets"] == Facets(domain="decisions", kind="decision")


def test_recall_no_facets_does_not_pass_kwarg(monkeypatch) -> None:
    # 不收窄时不传 facets kwarg → 旧引擎签名/默认路径零变化。
    captured: dict = {}

    class _Strict(_FakeLexical):
        def search(self, text, k=10, repo=None):  # 无 facets 参数
            captured["called"] = True
            return super().search(text, k, repo)

    monkeypatch.setattr("memex.engine.Engine", _Strict)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})
    result = runner.invoke(app, ["recall", "文档", "--lane", "lexical"])
    assert result.exit_code == 0, result.stdout
    assert captured["called"]


def test_recall_facets_require_central(monkeypatch) -> None:
    from memex.config import Settings

    monkeypatch.setattr("memex.recall.settings", Settings(read_from_central=False))
    result = runner.invoke(app, ["recall", "文档", "--kind", "decision"])
    assert result.exit_code == 2


def test_recall_export_includes_candidate_text(monkeypatch) -> None:
    monkeypatch.setattr("memex.engine.Engine", _FakeLexical)
    docs = {
        ("myrepo", "kb:doc:a"): Doc(
            object_key="kb:doc:a",
            title="富化标题",
            body="正文第一段\n\n正文第二段",
            path="docs/a.md",
            kind="runbook",
            domain_prefixes=("tools", "tools/rerank"),
            keywords=("reranker", "kb"),
        )
    }
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: docs)

    result = runner.invoke(
        app,
        [
            "recall-export",
            "文档",
            "--lane",
            "lexical",
            "--limit",
            "1",
            "--max-text-chars",
            "20",
        ],
    )
    assert result.exit_code == 0, result.stdout
    payload = json.loads(result.stdout)
    assert payload["schema"] == "memex-recall-export-v1"
    assert payload["query"] == "文档"
    hit = payload["hits"][0]
    assert hit["object_key"] == "kb:doc:a"
    assert hit["title"] == "富化标题"
    assert hit["path"] == "docs/a.md"
    assert hit["kind"] == "runbook"
    assert hit["domain_prefixes"] == ["tools", "tools/rerank"]
    assert hit["keywords"] == ["reranker", "kb"]
    assert hit["candidate_text"].startswith("富化标题\n\ndocs/a.md")
    assert hit["candidate_text_truncated"] is True
    assert hit["candidate_text_chars"] > len(hit["candidate_text"])


def test_recall_export_handles_missing_doc(monkeypatch) -> None:
    monkeypatch.setattr("memex.engine.Engine", _FakeLexical)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})

    result = runner.invoke(
        app, ["recall-export", "文档", "--lane", "lexical", "--limit", "1"]
    )
    assert result.exit_code == 0, result.stdout
    payload = json.loads(result.stdout)
    hit = payload["hits"][0]
    assert hit["candidate_text"] == "文档模板\n\nkb/a.md"
    assert hit["kind"] == ""
    assert hit["domain_prefixes"] == []


def test_recall_abs_path_from_registry(monkeypatch) -> None:
    # 读路径: recall 输出磁盘绝对路径(registry repo 根 + source_path),
    # agent 召回后可直接 Read。
    from pathlib import Path

    from memex.registry import SourceRegistry

    monkeypatch.setattr("memex.engine.Engine", _FakeLexical)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})
    monkeypatch.setattr(
        "memex.recall.load_source_registry",
        lambda: SourceRegistry(
            repos={"myrepo": Path("/ws/myrepo")},
            degraded=False,
            reason=None,
            legacy=frozenset(),
        ),
    )
    result = runner.invoke(
        app, ["recall", "文档", "--lane", "lexical", "--format", "json"]
    )
    assert result.exit_code == 0, result.stdout
    payload = json.loads(result.stdout)
    assert payload["hits"][0]["abs_path"] == "/ws/myrepo/kb/a.md"
    text = runner.invoke(app, ["recall", "文档", "--lane", "lexical"])
    assert "/ws/myrepo/kb/a.md" in text.stdout  # 文本输出含可直接 Read 的绝对路径行


def test_recall_preview_flag(monkeypatch) -> None:
    # --preview 才填正文摘要片段; 默认不填(保持默认输出紧凑)。
    from types import SimpleNamespace

    monkeypatch.setattr("memex.engine.Engine", _FakeLexical)
    monkeypatch.setattr(
        "memex.recall._doc_lookup",
        lambda repo: {
            ("myrepo", "kb:doc:a"): SimpleNamespace(
                title="文档模板",
                path="kb/a.md",
                body="这是正文摘要内容。" * 5,
                kind_explicit=True,
            )
        },
    )
    with_p = runner.invoke(
        app, ["recall", "文档", "--lane", "lexical", "--preview", "--format", "json"]
    )
    assert with_p.exit_code == 0, with_p.stdout
    assert json.loads(with_p.stdout)["hits"][0]["preview"].startswith(
        "这是正文摘要内容"
    )
    without_p = runner.invoke(
        app, ["recall", "文档", "--lane", "lexical", "--format", "json"]
    )
    assert json.loads(without_p.stdout)["hits"][0]["preview"] == ""
