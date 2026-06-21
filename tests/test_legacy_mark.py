"""legacy 仓命中在消费时刻标「未核验」(registry flag → recall 标注)。"""

import json

from typer.testing import CliRunner

from memex.cli import app
from memex.lexical import Hit
from memex.registry import SourceRegistry, load_source_registry

runner = CliRunner()


def test_registry_parses_legacy_flag(tmp_path, monkeypatch) -> None:
    toml = tmp_path / "kb-sources.toml"
    toml.write_text(
        f'workspace_root = "{tmp_path}"\n'
        '[[source]]\nname = "fresh"\n'
        '[[source]]\nname = "old"\nlegacy = true\n',
        encoding="utf-8",
    )
    monkeypatch.setenv("KB_SOURCES", str(toml))
    reg = load_source_registry()
    assert reg.legacy == frozenset({"old"})
    assert set(reg.repos) == {"fresh", "old"}


def test_registry_degraded_fallback_keeps_legacy_mark(tmp_path, monkeypatch) -> None:
    monkeypatch.setenv("KB_SOURCES", str(tmp_path / "no-such.toml"))
    reg = load_source_registry()
    assert reg.degraded
    assert isinstance(
        reg.legacy, frozenset
    )  # legacy set is present (may be empty by default)


class _FakeLexical:
    def __init__(self, *_a, **_k) -> None:
        pass

    def search(self, text: str, k: int = 10, repo: str | None = None) -> list[Hit]:
        return [
            Hit(
                object_key="old:d:a",
                score=2.0,
                title="旧知识",
                path="a.md",
                repo="oldrepo",
            ),
            Hit(
                object_key="kb:d:b", score=1.0, title="新知识", path="b.md", repo="ekb"
            ),
        ]


def _patch(monkeypatch) -> None:
    monkeypatch.setattr("memex.engine.Engine", _FakeLexical)
    monkeypatch.setattr("memex.recall._doc_lookup", lambda repo: {})
    monkeypatch.setattr(
        "memex.recall.load_source_registry",
        lambda: SourceRegistry(
            repos={}, degraded=False, reason=None, legacy=frozenset({"oldrepo"})
        ),
    )


def test_recall_json_carries_legacy_raw_unverified_flags_and_note(monkeypatch) -> None:
    _patch(monkeypatch)
    result = runner.invoke(
        app, ["recall", "q", "--lane", "lexical", "--format", "json"]
    )
    assert result.exit_code == 0, result.stdout
    payload = json.loads(result.stdout)
    by_key = {
        h["object_key"]: (h["legacy"], h["raw"], h["unverified"])
        for h in payload["hits"]
    }
    assert by_key == {"old:d:a": (True, True, True), "kb:d:b": (False, False, False)}
    assert any("legacy/raw" in n for n in payload["health"]["notes"])


def test_recall_text_marks_legacy_hit_only(monkeypatch) -> None:
    _patch(monkeypatch)
    result = runner.invoke(app, ["recall", "q", "--lane", "lexical"])
    assert result.exit_code == 0, result.stdout
    lines = result.stdout.splitlines()
    old_line = next(ln for ln in lines if "[oldrepo]" in ln)
    new_line = next(ln for ln in lines if "[ekb]" in ln)
    assert "⚠ legacy/raw 未核验" in old_line
    assert "legacy" not in new_line
    assert any(ln.startswith("!") and "legacy" in ln for ln in lines)
