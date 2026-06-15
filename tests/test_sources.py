"""Source registry + domain-tree discovery + completeness diff ."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from rhizome import sources


def _repo(base: Path, name: str) -> Path:
    r = base / name
    (r / ".git").mkdir(parents=True)
    return r


def _domain(repo: Path, rel: str, desc: str = "d", notes: list[str] | None = None):
    d = repo / rel
    d.mkdir(parents=True, exist_ok=True)
    (d / "INDEX.md").write_text(
        f"---\ndescription: {desc}\nkeywords: [x]\nkind: index\n---\n# {rel}\n"
    )
    for nm in notes or []:
        (d / nm).write_text("---\ndescription: n\nkeywords: [x]\n---\n# n\n")


def _pages(*pages):
    """Fake qdrant scroll fetch: yields the given pages, chaining offsets."""

    def fetch(offset):
        i = 0 if offset is None else offset
        page = dict(pages[i])
        page["next_page_offset"] = i + 1 if i + 1 < len(pages) else None
        return page

    return fetch


def _pt(identity: str, source_path: str | None):
    payload = {"identity": identity}
    if source_path is not None:
        payload["source_path"] = source_path
    return {"payload": payload}


def _registry(base: Path, names: list[str]) -> Path:
    lines = [f'workspace_root = "{base}"', ""]
    for nm in names:
        lines += ["[[source]]", f'name = "{nm}"', ""]
    reg = base / "kb-sources.toml"
    reg.write_text("\n".join(lines))
    return reg


class TestRegistry(unittest.TestCase):
    def test_load_sources(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            reg = _registry(base, ["my-kb", "legacy-repo"])
            got = sources.load_sources(reg)
            self.assertEqual([n for n, _ in got], ["my-kb", "legacy-repo"])
            self.assertEqual(got[0][1], base / "my-kb")

    def test_missing_registry_raises(self):
        with (
            tempfile.TemporaryDirectory() as tmp,
            self.assertRaises(sources.SourcesError),
        ):
            sources.load_sources(Path(tmp) / "nope.toml")


class TestDiscovery(unittest.TestCase):
    def test_discover_nested_domains_skips_root_index(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = _repo(Path(tmp), "kb")
            (repo / "INDEX.md").write_text(
                "---\ndescription: root\nkeywords: [x]\n---\n"
            )  # not a domain
            _domain(repo, "widgets", desc="示例域")
            _domain(repo, "widgets/blue", desc="示例")
            doms = sources.discover_domains(repo)
            self.assertEqual([d["domain"] for d in doms], ["widgets", "widgets/blue"])
            self.assertEqual(doms[0]["description"], "示例域")

    def test_note_domain_nearest(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = _repo(Path(tmp), "kb")
            _domain(repo, "widgets")
            _domain(repo, "widgets/blue")
            self.assertEqual(
                sources.note_domain(repo / "widgets/blue/n.md", repo), "widgets/blue"
            )
            self.assertEqual(
                sources.note_domain(repo / "widgets/n.md", repo), "widgets"
            )
            self.assertIsNone(sources.note_domain(repo / "loose/n.md", repo))

    def test_central_index_groups_by_repo_distinct_identity(self):
        fetch = _pages(
            {
                "points": [
                    _pt("kb:design:a", "design/a.md"),
                    _pt("kb:design:b", "design/b.md"),
                    _pt("kb:loose:c", None),  # missing source_path → ""
                    _pt("other:docs:x", "docs/x.md"),
                ]
            }
        )
        central = sources.central_note_index(fetch=fetch)
        self.assertEqual(
            set(central["kb"]), {"kb:design:a", "kb:design:b", "kb:loose:c"}
        )
        self.assertEqual(central["kb"]["kb:loose:c"], "")
        self.assertEqual(central["other"], {"other:docs:x": "docs/x.md"})

    def test_central_index_paginates_and_dedupes_chunks(self):
        # chunked notes share an identity across points → one entry;
        # pagination follows next_page_offset across pages.
        fetch = _pages(
            {
                "points": [
                    _pt("kb:design:a", "design/a.md"),
                    _pt("kb:design:a", "design/a.md"),
                ]
            },
            {
                "points": [
                    _pt("kb:design:a", "design/a.md"),
                    _pt("kb:design:b", "design/b.md"),
                ]
            },
        )
        central = sources.central_note_index(fetch=fetch)
        self.assertEqual(len(central["kb"]), 2)

    def test_central_index_tolerates_malformed_points(self):
        fetch = _pages(
            {
                "points": [
                    _pt("kb:design:a", "design/a.md"),
                    {"payload": {"identity": 42}},  # non-str identity
                    {
                        "payload": {"identity": "no-colon"}
                    },  # not repo:domain:slug shaped
                    {"payload": None},
                    "not-a-dict",
                ]
            }
        )
        central = sources.central_note_index(fetch=fetch)
        self.assertEqual(central, {"kb": {"kb:design:a": "design/a.md"}})

    def test_central_unreachable_raises_loud(self):
        def fetch(offset):
            raise sources.SourcesError("central index unreachable (test)")

        with self.assertRaises(sources.SourcesError):
            sources.central_note_index(fetch=fetch)

    def test_note_domain_outside_repo_returns_none(self):
        # a `..`-escaping source_path must not crash via derive_domain ValueError
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            repo = _repo(base, "kb")
            (base / "INDEX.md").write_text(
                "---\ndescription: stray\nkeywords: [x]\n---\n"
            )  # outside repo
            self.assertIsNone(sources.note_domain(repo / ".." / "note.md", repo))

    def test_old_frontmatter_note_resolves_by_path(self):
        # 新旧并存 at this layer: domain is path-based, ignores frontmatter shape
        with tempfile.TemporaryDirectory() as tmp:
            repo = _repo(Path(tmp), "kb")
            _domain(repo, "design")
            old = repo / "design" / "legacy.md"
            old.write_text(
                "---\nobject_key: kb:doc:legacy\ntopic: x\ntitle: Legacy\n---\n# L\n"
            )
            self.assertEqual(sources.note_domain(old, repo), "design")


class TestC2Codification(unittest.TestCase):
    # discovery and note_domain report the C2 node chain — non-domain
    # physical segments are dropped, matching the central index identity.
    def test_discover_domains_reports_c2_chain(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = _repo(Path(tmp), "kb")
            leaf = repo / "docs" / "source-notes" / "dm"
            leaf.mkdir(parents=True)
            (leaf / "INDEX.md").write_text(
                "---\ndescription: d\nkeywords: [x]\nkind: index\n---\n# dm\n"
            )
            doms = sources.discover_domains(repo)
            self.assertEqual([d["domain"] for d in doms], ["dm"])

    def test_note_domain_reports_c2_chain(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = _repo(Path(tmp), "kb")
            leaf = repo / "docs" / "source-notes" / "dm"
            leaf.mkdir(parents=True)
            (leaf / "INDEX.md").write_text("# dm\n")
            self.assertEqual(sources.note_domain(leaf / "n.md", repo), "dm")

    def test_root_index_note_domain_is_none(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = _repo(Path(tmp), "kb")
            (repo / "INDEX.md").write_text("# root\n")
            self.assertIsNone(sources.note_domain(repo / "n.md", repo))


class TestTreeAndDiff(unittest.TestCase):
    def test_build_tree(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            repo = _repo(base, "kb")
            _domain(repo, "design", desc="设计")
            reg = _registry(base, ["kb", "ghost"])
            tree = sources.build_tree(reg)
            self.assertEqual(tree[0]["name"], "kb")
            self.assertEqual(tree[0]["domains"][0]["domain"], "design")
            self.assertFalse(tree[1]["exists"])  # ghost repo missing

    def test_diff_parent_covered_by_child_not_empty(self):
        # prefix coverage: parent domain with notes only in a child is NOT empty
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            repo = _repo(base, "kb")
            _domain(repo, "widgets")
            _domain(repo, "widgets/blue")
            central = {"kb": {"kb:x:n": "widgets/blue/n.md"}}  # only child has a note
            reg = _registry(base, ["kb"])
            row = {r["name"]: r for r in sources.diff(reg, central=central)}["kb"]
            self.assertEqual(row["status"], "ok")
            self.assertEqual(row["empty_domains"], [])  # widgets covered via child
            self.assertEqual(row["domains_indexed"], 2)

    def test_diff_segment_aware_no_false_prefix(self):
        # `widgets` coverage must NOT count a sibling `legacy-repo`-style domain
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            repo = _repo(base, "kb")
            _domain(repo, "legacy-repo")
            _domain(repo, "widgets")
            central = {
                "kb": {"kb:x:n": "legacy-repo/n.md"}
            }  # only the -private domain has a note
            reg = _registry(base, ["kb"])
            row = {r["name"]: r for r in sources.diff(reg, central=central)}["kb"]
            self.assertEqual(
                row["empty_domains"], ["widgets"]
            )  # NOT covered by legacy-repo

    def test_duplicate_source_name_raises(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            reg = _registry(base, ["kb", "kb"])
            with self.assertRaises(sources.SourcesError):
                sources.load_sources(reg)

    def test_diff_statuses(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            # kb: 2 domains, one indexed one empty
            kb = _repo(base, "kb")
            _domain(kb, "design")
            _domain(kb, "research")
            central = {
                "kb": {"kb:design:a": "design/a.md"}
            }  # design indexed, research empty; pmrepo absent
            # ops: no INDEX.md
            ops = _repo(base, "ops")
            (ops / "notes.md").write_text("x")
            # pmrepo: domain but not indexed
            pm = _repo(base, "pmrepo")
            _domain(pm, "decisions")
            reg = _registry(base, ["kb", "ops", "pmrepo", "ghost"])
            rep = {r["name"]: r for r in sources.diff(reg, central=central)}
            self.assertEqual(rep["kb"]["status"], "ok")
            self.assertEqual(rep["kb"]["empty_domains"], ["research"])
            self.assertEqual(rep["kb"]["orphan_notes"], 0)
            self.assertEqual(rep["ops"]["status"], "no-domains")
            self.assertEqual(rep["pmrepo"]["status"], "not-indexed")
            self.assertEqual(rep["ghost"]["status"], "missing-repo")


class TestCaseInsensitiveDiscovery(unittest.TestCase):
    def test_lowercase_index_md_is_not_a_domain(self):
        # rglob's literal pattern matched index.md on APFS; the os.walk
        # rewrite compares real entry names, so only exact INDEX.md counts.
        with tempfile.TemporaryDirectory() as tmp:
            repo = _repo(Path(tmp), "kb")
            docs = repo / "docs"
            docs.mkdir()
            (docs / "index.md").write_text("# lowercase\n")
            self.assertEqual(sources.discover_domains(repo), [])
            _domain(repo, "real")
            self.assertEqual(
                [d["domain"] for d in sources.discover_domains(repo)], ["real"]
            )


if __name__ == "__main__":
    unittest.main()
