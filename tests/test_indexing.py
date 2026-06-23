"""写路径切片① 测试: 域树发现 / domain 派生 / identity / 编译 / 报告。

tmp_path 造仓; git commit_time 在临时真 git 仓里测(git 不可用则 skip)。
"""

from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

import pytest

from memex.indexing.compile import (
    KINDS,
    compile_note,
    doc_to_json,
    embed_text,
    safe_filename,
)
from memex.indexing.frontmatter import (
    FrontmatterError,
    parse_frontmatter,
    split_frontmatter,
)
from memex.indexing.pipeline import compile_repo, persist
from memex.indexing.scan import (
    ScanError,
    discover_domains,
    scan_notes,
)

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


def _index(path: Path, desc: str = "domain home") -> None:
    _note(
        path,
        f'---\ndescription: "{desc}"\nkeywords: [idx]\nkind: index\n---\n\n# {path.parent.name}\n',
    )


# ---- frontmatter parser -----------------------------------------------------


def test_split_no_frontmatter_returns_none() -> None:
    assert split_frontmatter("# just a title\n\nbody") is None


def test_split_unterminated_raises() -> None:
    with pytest.raises(FrontmatterError):
        split_frontmatter("---\ndescription: x\n\nno closing fence")


def test_parse_flow_list_and_scalars() -> None:
    fm = parse_frontmatter(FM)
    assert fm is not None
    assert fm["description"] == "一句话召回摘要"
    assert fm["keywords"] == ["foo", "bar"]
    assert fm["kind"] == "reference"


def test_parse_block_list() -> None:
    text = "---\ndescription: x\nkeywords:\n  - a\n  - b\n---\nbody\n"
    fm = parse_frontmatter(text)
    assert fm is not None
    assert fm["keywords"] == ["a", "b"]


# ---- domain tree discovery (C2) --------------------------------------------


def test_root_index_contributes_empty_segment(tmp_path: Path) -> None:
    # repo 根有 INDEX.md → 根域 domain="", 其直接子 INDEX 节点链从自己 basename 开始。
    _index(tmp_path / "INDEX.md", "root")
    _index(tmp_path / "contracts" / "INDEX.md")
    nodes = discover_domains(tmp_path)
    domains = {n.domain for n in nodes}
    assert "" in domains  # 根域
    assert (
        "contracts" in domains
    )  # 与上游写入工具产出 `<repo>:contracts:...` identity 一致


def test_nested_index_node_chain(tmp_path: Path) -> None:
    _index(tmp_path / "docs" / "domain-map" / "INDEX.md")
    _index(tmp_path / "docs" / "domain-map" / "warehouse" / "INDEX.md")
    nodes = {n.domain: n for n in discover_domains(tmp_path)}
    assert "domain-map" in nodes  # docs/ 是非域物理段, 跳过
    assert "domain-map/warehouse" in nodes
    assert nodes["domain-map/warehouse"].prefixes == (
        "domain-map",
        "domain-map/warehouse",
    )


def test_node_chain_skips_non_domain_physical_segments(tmp_path: Path) -> None:
    # domain-map/widget-spec(无 INDEX)/use-cases(有 INDEX) → 域 domain-map/use-cases。
    _index(tmp_path / "docs" / "source-notes" / "domain-map" / "INDEX.md")
    _index(
        tmp_path
        / "docs"
        / "source-notes"
        / "domain-map"
        / "widget-spec"
        / "use-cases"
        / "INDEX.md"
    )
    domains = {n.domain for n in discover_domains(tmp_path)}
    assert "domain-map" in domains
    assert "domain-map/use-cases" in domains  # widget-spec 段被跳过
    assert "domain-map/widget-spec/use-cases" not in domains


def test_subdir_without_index_belongs_to_parent_domain(tmp_path: Path) -> None:
    _index(tmp_path / "contracts" / "INDEX.md")
    _note(tmp_path / "contracts" / "sub" / "deep.md")
    nodes = discover_domains(tmp_path)
    scanned = scan_notes("myrepo", tmp_path, nodes)
    deep = next(s for s in scanned if s.slug.endswith("deep"))
    assert deep.node.domain == "contracts"
    assert deep.slug == "sub/deep"  # identity slug 含子路径(C3)


def test_duplicate_domain_guard(tmp_path: Path) -> None:
    # 两条物理路径派生同一域路径: 仅当根域存在且某子域 basename 与另一处冲突。
    # 构造: a/INDEX(域 a) 与 b/a/INDEX —— b 无 INDEX → b 跳过 → 两者都派生 "a"。
    _index(tmp_path / "a" / "INDEX.md")
    _index(tmp_path / "b" / "a" / "INDEX.md")  # b 非域段, 跳过 → 也是域 "a"
    with pytest.raises(ScanError, match="duplicate domain"):
        discover_domains(tmp_path)


def test_index_md_self_domain(tmp_path: Path) -> None:
    _index(tmp_path / "contracts" / "INDEX.md")
    nodes = discover_domains(tmp_path)
    scanned = scan_notes("myrepo", tmp_path, nodes)
    idx = next(s for s in scanned if s.is_index)
    assert idx.node.domain == "contracts"  # INDEX.md 自身 domain = 它定义的节点
    assert idx.identity == "myrepo:contracts:INDEX"


# ---- identity (C3) ----------------------------------------------------------


def test_identity_position_derived(tmp_path: Path) -> None:
    _index(tmp_path / "contracts" / "INDEX.md")
    _note(tmp_path / "contracts" / "kb-note.md")
    nodes = discover_domains(tmp_path)
    scanned = scan_notes("myrepo", tmp_path, nodes)
    note = next(s for s in scanned if s.slug == "kb-note")
    assert note.identity == "myrepo:contracts:kb-note"


def test_identity_ignores_object_key_in_frontmatter(tmp_path: Path) -> None:
    _index(tmp_path / "contracts" / "INDEX.md")
    fm = '---\ndescription: x\nkeywords: [k]\nobject_key: "stale:legacy:key"\n---\n# T\nbody\n'
    _note(tmp_path / "contracts" / "a.md", fm)
    nodes = discover_domains(tmp_path)
    scanned = scan_notes("repo", tmp_path, nodes)
    note = next(s for s in scanned if s.slug == "a")
    assert note.identity == "repo:contracts:a"  # 忽略 frontmatter object_key


def test_fold_normalizes_case_and_nfc() -> None:
    # _fold = NFC + casefold;大小写折叠 + 组合字符归一化后等价(C3 兜底前置)。
    from memex.indexing.scan import _fold

    assert _fold("Repo:D:Foo") == _fold("repo:d:foo")
    # café 的 NFC(é U+00E9) 与 NFD(e + ́ U+0301) 折叠后相等
    nfc = "repo:d:café"
    nfd = "repo:d:café"
    assert _fold(nfc) == _fold(nfd)


def test_case_collision_loud_error(tmp_path: Path) -> None:
    # 大小写敏感 FS 上才能造出 Foo.md 与 foo.md 共存(撞 casefold identity);
    # 大小写不敏感 FS(macOS APFS 默认)两文件折叠成一个 inode → skip。
    _index(tmp_path / "d" / "INDEX.md")
    p1 = tmp_path / "d" / "Foo.md"
    p2 = tmp_path / "d" / "foo.md"
    p1.write_text(FM, encoding="utf-8")
    p2.write_text(FM, encoding="utf-8")
    if len(list((tmp_path / "d").glob("*.md"))) < 3:  # INDEX + Foo + foo
        pytest.skip("filesystem case-insensitive; Foo.md and foo.md collapse")
    nodes = discover_domains(tmp_path)
    with pytest.raises(ScanError, match="identity collision"):
        scan_notes("repo", tmp_path, nodes)


# ---- frontmatter gate (C1) --------------------------------------------------


def test_no_frontmatter_loud_skip(tmp_path: Path) -> None:
    _index(tmp_path / "d" / "INDEX.md")
    (tmp_path / "d" / "plain.md").write_text(
        "# no frontmatter\n\nbody\n", encoding="utf-8"
    )
    out = compile_repo("repo", tmp_path)
    skipped = {s.source_path for s in out.report.skipped}
    assert "d/plain.md" in skipped


def test_legacy_raw_compile_without_index_or_frontmatter(tmp_path: Path) -> None:
    (tmp_path / "notes" / "plain.md").parent.mkdir(parents=True)
    (tmp_path / "notes" / "plain.md").write_text(
        "# Raw note\n\n旧材料正文。\n", encoding="utf-8"
    )
    _note(
        tmp_path / "with-fm.md",
        "---\ndescription: old desc\nkeywords: [LegacyKW]\n---\n# FM note\nbody\n",
    )
    out = compile_repo("legacy-repo", tmp_path, legacy=True)
    assert out.report.domains == ["legacy"]
    assert out.report.skipped == []
    assert out.report.indexed == 2

    raw = next(d for d in out.docs if d.source_path == "notes/plain.md")
    assert raw.identity == "legacy-repo:legacy:notes/plain"  # ADR-035: registry name, 不取磁盘 basename
    assert raw.domain == "legacy"
    assert raw.domain_prefixes == ["legacy"]
    assert raw.kind == "note"
    assert raw.kind_explicit is True
    assert "LEGACY RAW UNVERIFIED" in raw.description
    assert "LEGACY RAW UNVERIFIED" in raw.body_text

    with_fm = next(d for d in out.docs if d.source_path == "with-fm.md")
    assert "old desc" in with_fm.description
    assert "legacykw" in with_fm.keywords
    assert {"legacy", "raw", "unverified"} <= set(with_fm.keywords)


def test_repo_identity_uses_registry_name_not_basename(tmp_path: Path) -> None:
    """ADR-035 caveat-A 回归防线: 物理目录名(leaf) != registry name 时,

    identity / repo / canonical_repo / 落盘子目录全部用 registry name, 不取磁盘
    basename。生产名常 name==leaf, 物理数据测不出"读 leaf"的回归 → 必须用
    name≠leaf 的合成 fixture 钉死。
    """
    leaf_dir = tmp_path / "bar"  # 物理 leaf = "bar"
    _index(leaf_dir / "d" / "INDEX.md")
    _note(leaf_dir / "d" / "note.md")
    out = compile_repo("foo", leaf_dir)  # registry name = "foo" ≠ leaf "bar"
    assert out.canonical_repo == "foo"
    doc = next(d for d in out.docs if d.source_path == "d/note.md")
    assert doc.repo == "foo"
    assert doc.identity == "foo:d:note"
    assert "bar" not in doc.identity  # 物理 leaf 绝不泄漏进 identity


def test_v3_compat_fields(tmp_path: Path) -> None:
    _index(tmp_path / "d" / "INDEX.md")
    v3 = (
        "---\n"
        'object_key: "repo:d:legacy"\n'
        'description: "v3 desc"\n'
        "keywords: [legacy, kw]\n"
        "kind: reference\n"
        "---\n# Legacy\nbody\n"
    )
    _note(tmp_path / "d" / "legacy.md", v3)
    out = compile_repo("repo", tmp_path)
    doc = next(d for d in out.docs if d.source_path == "d/legacy.md")
    assert doc.description == "v3 desc"
    assert doc.keywords == ["legacy", "kw"]
    assert doc.kind == "reference"


def test_v3_missing_fields_defaults(tmp_path: Path) -> None:
    # 旧 v3 没 description/keywords/kind → 空/默认 note(不要求迁移)。
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "bare.md", "---\nobject_key: x\n---\n# Bare\nbody\n")
    out = compile_repo("repo", tmp_path)
    doc = next(d for d in out.docs if d.source_path == "d/bare.md")
    assert doc.description == ""
    assert doc.keywords == []
    assert doc.kind == "note"


def test_kind_downgrade(tmp_path: Path) -> None:
    _index(tmp_path / "d" / "INDEX.md")
    bad = '---\ndescription: x\nkeywords: [k]\nkind: "design-doc"\n---\n# T\nbody\n'
    _note(tmp_path / "d" / "bad.md", bad)
    out = compile_repo("repo", tmp_path)
    doc = next(d for d in out.docs if d.source_path == "d/bad.md")
    assert doc.kind == "note"
    downs = {k.source_path: k.from_kind for k in out.report.kind_downgrades}
    assert downs.get("d/bad.md") == "design-doc"


def test_all_kinds_accepted(tmp_path: Path) -> None:
    _index(tmp_path / "d" / "INDEX.md")
    for kind in sorted(KINDS - {"index"}):
        fm = f"---\ndescription: x\nkeywords: [k]\nkind: {kind}\n---\n# {kind}\nbody\n"
        _note(tmp_path / "d" / f"{kind}.md", fm)
    out = compile_repo("repo", tmp_path)
    assert not out.report.kind_downgrades


# ---- compiled doc (C4) ------------------------------------------------------


def test_compiled_doc_fields_complete(tmp_path: Path) -> None:
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "note.md")
    nodes = discover_domains(tmp_path)
    scanned = scan_notes("repo", tmp_path, nodes)
    note = next(s for s in scanned if s.slug == "note")
    result = compile_note(note, tmp_path)
    doc = result.doc
    assert doc is not None
    assert doc.identity == "repo:d:note"
    assert doc.repo == "repo"
    assert doc.domain == "d"
    assert doc.domain_prefixes == ["d"]
    assert doc.title == "标题在这里"  # H1 优先
    assert doc.description == "一句话召回摘要"
    assert doc.keywords == ["foo", "bar"]
    assert doc.kind == "reference"
    assert "正文内容" in doc.body_text
    assert "description:" not in doc.body_text  # 去 frontmatter
    assert doc.source_path == "d/note.md"
    assert len(doc.source_hash) == 64
    assert len(doc.compiled_hash) == 64
    assert doc.schema == "kb-note-v1"


def test_title_falls_back_to_filename(tmp_path: Path) -> None:
    _index(tmp_path / "d" / "INDEX.md")
    _note(
        tmp_path / "d" / "no-h1.md",
        "---\ndescription: x\nkeywords: [k]\n---\n\njust body\n",
    )
    nodes = discover_domains(tmp_path)
    scanned = scan_notes("repo", tmp_path, nodes)
    note = next(s for s in scanned if s.slug == "no-h1")
    doc = compile_note(note, tmp_path).doc
    assert doc is not None
    assert doc.title == "no-h1"


def test_compiled_hash_stable(tmp_path: Path) -> None:
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "note.md")
    nodes = discover_domains(tmp_path)
    scanned = scan_notes("repo", tmp_path, nodes)
    note = next(s for s in scanned if s.slug == "note")
    h1 = compile_note(note, tmp_path).doc.compiled_hash
    h2 = compile_note(note, tmp_path).doc.compiled_hash
    assert h1 == h2  # 同输入 → 同 hash(commit_time 在非 git tmp 仓恒为 None)


def test_doc_to_json_roundtrip(tmp_path: Path) -> None:
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "note.md")
    nodes = discover_domains(tmp_path)
    scanned = scan_notes("repo", tmp_path, nodes)
    note = next(s for s in scanned if s.slug == "note")
    doc = compile_note(note, tmp_path).doc
    payload = json.loads(doc_to_json(doc))
    assert payload["identity"] == "repo:d:note"
    assert payload["domain_prefixes"] == ["d"]


def test_safe_filename() -> None:
    fn = safe_filename("repo:domain-map/widget-spec:sub/note")
    assert fn.endswith(".json")
    assert ":" not in fn
    assert "/" not in fn


# ---- embed_text (C4) --------------------------------------------------------


def test_embed_text_order() -> None:
    # M1: 不含 title(title 可回退文件名, 改名不该变 text_hash)。
    txt = embed_text("D", ["k1", "k2"], "B")
    assert txt == "D\n\nk1 k2\n\nB"


def test_embed_text_skips_empty() -> None:
    txt = embed_text("", [], "B")
    assert txt == "B"


# ---- coverage diff + report -------------------------------------------------


def test_coverage_diff_empty_domain(tmp_path: Path) -> None:
    _index(tmp_path / "filled" / "INDEX.md")
    _note(tmp_path / "filled" / "note.md")
    _index(tmp_path / "empty" / "INDEX.md")  # 只有 INDEX 自己, 无普通 note
    out = compile_repo("repo", tmp_path)
    # empty 域有 INDEX.md(它自身 indexed), filled 有 note + INDEX。
    # "空域" = 无任何 note(连 INDEX 自身都没编进? INDEX 也算 note)。
    assert "empty" not in out.report.empty_domains  # INDEX.md 自身使域非空
    assert out.report.indexed >= 3  # 2 INDEX + 1 note


def test_coverage_diff_index_without_frontmatter_is_empty(tmp_path: Path) -> None:
    # INDEX.md 无 frontmatter → loud-skip → 该域无任何 note → 空域。
    _index(tmp_path / "filled" / "INDEX.md")
    _note(tmp_path / "filled" / "note.md")
    (tmp_path / "bare").mkdir()
    (tmp_path / "bare" / "INDEX.md").write_text(
        "# bare index no fm\n", encoding="utf-8"
    )
    out = compile_repo("repo", tmp_path)
    assert "bare" in out.report.empty_domains


def test_report_render_readable(tmp_path: Path) -> None:
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "note.md")
    out = compile_repo("repo", tmp_path)
    text = out.report.render()
    assert "repo" in text
    assert "indexed" in text


def test_missing_repo_reported(tmp_path: Path) -> None:
    out = compile_repo("ghost", tmp_path / "does-not-exist")
    assert out.report.error is not None
    assert not out.docs


# ---- persist ----------------------------------------------------------------


def test_persist_writes_json(tmp_path: Path) -> None:
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "note.md")
    out = compile_repo("repo", tmp_path)
    compiled_dir = tmp_path / "_compiled"
    n = persist(out.docs, compiled_dir, out.canonical_repo)
    assert n == len(out.docs)
    files = list((compiled_dir / out.canonical_repo).glob("*.json"))
    assert files
    data = json.loads(files[0].read_text(encoding="utf-8"))
    assert "identity" in data


# ---- git commit_time --------------------------------------------------------


@pytest.mark.skipif(shutil.which("git") is None, reason="git not available")
def test_commit_time_from_git(tmp_path: Path) -> None:
    repo = tmp_path / "gitrepo"
    repo.mkdir()
    env = {
        "GIT_AUTHOR_NAME": "t",
        "GIT_AUTHOR_EMAIL": "t@t",
        "GIT_COMMITTER_NAME": "t",
        "GIT_COMMITTER_EMAIL": "t@t",
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_SYSTEM": "/dev/null",
        "PATH": __import__("os").environ.get("PATH", ""),
    }

    def _git(*args: str) -> None:
        subprocess.run(
            ["git", "-C", str(repo), *args], check=True, env=env, capture_output=True
        )

    _git("init")
    _index(repo / "d" / "INDEX.md")
    _note(repo / "d" / "note.md")
    _git("add", "-A")
    _git("commit", "-m", "init")

    out = compile_repo("repo", repo)
    doc = next(d for d in out.docs if d.source_path == "d/note.md")
    assert doc.commit_time is not None
    assert "T" in doc.commit_time  # ISO 8601


def test_commit_time_none_outside_git(tmp_path: Path) -> None:
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "note.md")
    out = compile_repo("repo", tmp_path)
    doc = next(d for d in out.docs if d.source_path == "d/note.md")
    assert doc.commit_time is None  # 非 git 仓


# ---- kind_explicit ---------------------------------------------------


def test_kind_missing_recorded(tmp_path: Path) -> None:
    # 无 kind frontmatter → 默认 note 但记录缺失(prior 稀释要 loud)。
    _index(tmp_path / "d" / "INDEX.md")
    _note(
        tmp_path / "d" / "nokind.md",
        '---\ndescription: "x"\nkeywords: [a]\n---\n\n# T\n\n正文。\n',
    )
    out = compile_repo("repo", tmp_path)
    doc = next(d for d in out.docs if d.source_path == "d/nokind.md")
    assert doc.kind == "note" and doc.kind_explicit is False
    assert out.report.kind_missing == ["d/nokind.md"]
    assert "kind-missing 1" in out.report.summary_line()
    assert "kind missing" in out.report.render()


def test_kind_present_is_explicit(tmp_path: Path) -> None:
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")  # FM 模板带 kind: reference
    out = compile_repo("repo", tmp_path)
    doc = next(d for d in out.docs if d.source_path == "d/a.md")
    assert doc.kind_explicit is True
    assert out.report.kind_missing == []


def test_kind_downgrade_counts_explicit_not_missing(tmp_path: Path) -> None:
    # 越界 kind: 给了就算 explicit(越界已有 kind_downgrades 单独 loud), 不计 missing。
    _index(tmp_path / "d" / "INDEX.md")
    _note(
        tmp_path / "d" / "bogus.md",
        '---\ndescription: "x"\nkeywords: [a]\nkind: bogus\n---\n\n# T\n\n正文。\n',
    )
    out = compile_repo("repo", tmp_path)
    doc = next(d for d in out.docs if d.source_path == "d/bogus.md")
    assert doc.kind == "note" and doc.kind_explicit is True
    assert len(out.report.kind_downgrades) == 1
    assert out.report.kind_missing == []


def test_kind_explicit_in_compiled_json(tmp_path: Path) -> None:
    # kind_explicit 进落盘 JSON(读路径据此计数)且进 compiled_hash 输入。
    _index(tmp_path / "d" / "INDEX.md")
    _note(tmp_path / "d" / "a.md")
    out = compile_repo("repo", tmp_path)
    doc = next(d for d in out.docs if d.source_path == "d/a.md")
    assert json.loads(doc_to_json(doc))["kind_explicit"] is True
