"""compile 内容完整性 loud-report 测试。

覆盖两类发现:
  ① ZERO_DOC —— 有域(INDEX.md)却 compile 出 0 doc。
  ② DOMAIN_SKIP —— 域内文件无 frontmatter 被静默 skip。
+ 非发现旁路(无 INDEX.md 的仓、硬错误仓、健康仓)+ CLI 退出码 / 显眼 section。
"""

from __future__ import annotations

from pathlib import Path

from typer.testing import CliRunner

from memex.indexing import cli as sync_cli
from memex.indexing.integrity import (
    DOMAIN_SKIP,
    ZERO_DOC,
    IntegrityReport,
    findings_for_report,
)
from memex.indexing.pipeline import compile_repo
from memex.indexing.report import RepoReport, SkipEntry

FM = """---
description: "一句话召回摘要"
keywords: [foo, bar]
kind: reference
---

# 标题

正文。
"""


def _note(path: Path, fm: str = FM) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(fm, encoding="utf-8")


def _index(path: Path) -> None:
    _note(
        path,
        f'---\ndescription: "home"\nkeywords: [idx]\nkind: index\n---\n\n# {path.parent.name}\n',
    )


# ---- findings_for_report 单元(直接喂 RepoReport)---------------------------


def test_zero_doc_with_domains_is_finding() -> None:
    # 发现了域但 indexed==0 → ZERO_DOC。
    rep = RepoReport(repo="r", repo_path="/r", indexed=0, domains=["contracts"])
    finds = findings_for_report(rep)
    assert [f.kind for f in finds] == [ZERO_DOC]
    assert "0 doc" in finds[0].detail


def test_zero_doc_without_domains_is_not_finding() -> None:
    # 仓里没 INDEX.md(无域)→ 域外散落 md 本就不该索引, 不算完整性问题。
    rep = RepoReport(repo="r", repo_path="/r", indexed=0, domains=[])
    assert findings_for_report(rep) == []


def test_domain_skip_is_finding() -> None:
    rep = RepoReport(
        repo="r",
        repo_path="/r",
        indexed=2,
        domains=["d"],
        skipped=[SkipEntry(source_path="d/plain.md", reason="no frontmatter")],
    )
    finds = findings_for_report(rep)
    assert [f.kind for f in finds] == [DOMAIN_SKIP]
    assert finds[0].count == 1
    assert finds[0].paths == ("d/plain.md",)


def test_zero_doc_and_skip_both_reported() -> None:
    # 仓有域、indexed==0、且有被 skip 的文件 → 两条发现都出。
    rep = RepoReport(
        repo="r",
        repo_path="/r",
        indexed=0,
        domains=["d"],
        skipped=[SkipEntry(source_path="d/a.md", reason="no frontmatter")],
    )
    kinds = {f.kind for f in findings_for_report(rep)}
    assert kinds == {ZERO_DOC, DOMAIN_SKIP}


def test_hard_error_repo_is_not_integrity_finding() -> None:
    # error / duplicate_error 走 compile 既有 fail-stop, 不重复 loud。
    err = RepoReport(repo="r", repo_path="/r", error="repo path not found")
    dup = RepoReport(repo="r", repo_path="/r", duplicate_error="duplicate domain")
    assert findings_for_report(err) == []
    assert findings_for_report(dup) == []


def test_healthy_repo_no_findings() -> None:
    rep = RepoReport(repo="r", repo_path="/r", indexed=5, domains=["d"])
    assert findings_for_report(rep) == []


# ---- IntegrityReport 聚合 + 显眼 section -------------------------------------


def test_integrity_report_flag_and_render() -> None:
    integ = IntegrityReport()
    integ.add_repo(RepoReport(repo="empty", repo_path="/e", indexed=0, domains=["d"]))
    integ.add_repo(
        RepoReport(
            repo="skips",
            repo_path="/s",
            indexed=1,
            domains=["d"],
            skipped=[SkipEntry(source_path="d/x.md", reason="no frontmatter")],
        )
    )
    integ.add_repo(RepoReport(repo="ok", repo_path="/o", indexed=3, domains=["d"]))
    assert integ.has_findings is True
    assert len(integ.zero_doc_repos()) == 1
    assert len(integ.domain_skip_repos()) == 1
    section = integ.render()
    assert "INTEGRITY" in section
    assert "ZERO_DOC" in section and "empty" in section
    assert "DOMAIN_SKIP" in section and "d/x.md" in section


def test_integrity_report_empty_renders_blank() -> None:
    integ = IntegrityReport()
    integ.add_repo(RepoReport(repo="ok", repo_path="/o", indexed=3, domains=["d"]))
    assert integ.has_findings is False
    assert integ.render() == ""


# ---- 端到端: 真仓 → compile_repo → findings ---------------------------------


def test_compile_repo_zero_doc_real_tree(tmp_path: Path) -> None:
    # INDEX.md 无 frontmatter → 它自己被 skip → 该域 0 doc, 全仓 indexed==0。
    (tmp_path / "d").mkdir()
    (tmp_path / "d" / "INDEX.md").write_text("# bare index\n", encoding="utf-8")
    out = compile_repo("r", tmp_path)
    assert out.report.indexed == 0
    assert out.report.domains  # 域被发现了
    finds = findings_for_report(out.report)
    kinds = {f.kind for f in finds}
    assert ZERO_DOC in kinds  # 有域却 0 doc
    assert DOMAIN_SKIP in kinds  # INDEX.md 本身就是域内被 skip 的文件


def test_compile_repo_domain_skip_real_tree(tmp_path: Path) -> None:
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "good.md")
    (tmp_path / "d" / "plain.md").write_text("# no fm\n\nbody\n", encoding="utf-8")
    out = compile_repo("r", tmp_path)
    assert out.report.indexed >= 1  # INDEX + good
    finds = findings_for_report(out.report)
    assert [f.kind for f in finds] == [DOMAIN_SKIP]
    assert "d/plain.md" in finds[0].paths


# ---- CLI: distinct 退出码 3 + 显眼 section -----------------------------------


def test_compile_cli_integrity_exit_code(tmp_path: Path) -> None:
    # 健康仓 + 域内 skip 仓 → 退出码 3, 输出含 INTEGRITY section。
    good = tmp_path / "good"
    _index(good / "d" / "INDEX.md")
    _note(good / "d" / "n.md")
    skipper = tmp_path / "skipper"
    _index(skipper / "d" / "INDEX.md")
    (skipper / "d" / "plain.md").write_text("# no fm\n\nbody\n", encoding="utf-8")

    result = CliRunner().invoke(
        sync_cli.app,
        [
            "compile",
            "--dry-run",
            "--repo",
            f"good={good}",
            "--repo",
            f"skipper={skipper}",
        ],
    )
    assert result.exit_code == sync_cli.EXIT_INTEGRITY, result.stdout
    assert "INTEGRITY" in result.stdout
    assert "DOMAIN_SKIP" in result.stdout
    assert "d/plain.md" in result.stdout


def test_compile_cli_clean_exits_zero(tmp_path: Path) -> None:
    # 健康仓 → 退出码 0, 无 INTEGRITY section(向后兼容)。
    good = tmp_path / "good"
    _index(good / "d" / "INDEX.md")
    _note(good / "d" / "n.md")
    result = CliRunner().invoke(
        sync_cli.app, ["compile", "--dry-run", "--repo", f"good={good}"]
    )
    assert result.exit_code == 0, result.stdout
    assert "INTEGRITY" not in result.stdout


def test_compile_cli_zero_doc_exit_code(tmp_path: Path) -> None:
    bare = tmp_path / "bare"
    (bare / "d").mkdir(parents=True)
    (bare / "d" / "INDEX.md").write_text("# bare index\n", encoding="utf-8")
    result = CliRunner().invoke(
        sync_cli.app, ["compile", "--dry-run", "--repo", f"bare={bare}"]
    )
    assert result.exit_code == sync_cli.EXIT_INTEGRITY, result.stdout
    assert "ZERO_DOC" in result.stdout
