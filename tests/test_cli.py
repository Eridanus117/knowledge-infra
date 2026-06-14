import json

from typer.testing import CliRunner

from memex.cli import app
from memex.lexical import Hit

runner = CliRunner()


class _FakeEngine:
    def __init__(self, *_a, **_k) -> None:
        pass

    def search(self, text: str, k: int = 10, repo: str | None = None) -> list[Hit]:
        return [
            Hit(
                object_key="kb:doc:x", score=1.25, title="t", path="p.md", repo="myrepo"
            )
        ]


def test_query_json_contract(monkeypatch) -> None:
    # eval 适配契约: {hits:[{object_key,...}]}。
    monkeypatch.setattr("memex.engine.Engine", _FakeEngine)
    result = runner.invoke(app, ["query", "文档", "--format", "json"])
    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["hits"][0]["object_key"] == "kb:doc:x"
    assert payload["hits"][0]["score"] == 1.25


def test_query_text_output(monkeypatch) -> None:
    monkeypatch.setattr("memex.engine.Engine", _FakeEngine)
    result = runner.invoke(app, ["query", "文档"])
    assert result.exit_code == 0
    assert "kb:doc:x" in result.stdout
