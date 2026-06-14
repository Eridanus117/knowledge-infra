"""kb check validator: ERROR spine blocks, WARN drift doesn't."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from kb import check, contract


def _errs(findings):
    return [f for f in findings if f.severity == check.ERROR]


def _warns(findings):
    return [f for f in findings if f.severity == check.WARN]


def _fields(findings, severity):
    return {f.field for f in findings if f.severity == severity}


class TestSpineErrors(unittest.TestCase):
    def test_compliant_new_note_clean(self):
        text = (
            contract.render_frontmatter(
                description="d", keywords=["a", "b"], kind="reference"
            )
            + "\nbody\n"
        )
        self.assertEqual(check.check_text(text), [])

    def test_missing_description_errors(self):
        text = "---\nkeywords: [a]\nkind: note\n---\nbody\n"
        f = check.check_text(text)
        self.assertTrue(check.has_errors(f))
        self.assertIn("description", _fields(f, check.ERROR))

    def test_empty_keywords_errors(self):
        text = "---\ndescription: x\nkeywords: []\n---\nbody\n"
        self.assertIn("keywords", _fields(check.check_text(text), check.ERROR))

    def test_missing_keywords_errors(self):
        text = "---\ndescription: x\n---\nbody\n"
        self.assertIn("keywords", _fields(check.check_text(text), check.ERROR))

    def test_bad_kind_errors(self):
        text = "---\ndescription: x\nkeywords: [a]\nkind: essay\n---\nbody\n"
        self.assertIn("kind", _fields(check.check_text(text), check.ERROR))

    def test_unterminated_frontmatter_errors(self):
        text = "---\ndescription: x\nkeywords: [a]\n(no close)\n"
        f = check.check_text(text)
        self.assertTrue(check.has_errors(f))

    def test_no_frontmatter_is_skipped(self):
        self.assertEqual(check.check_text("# README\n\nnot a note\n"), [])

    def test_inline_comment_on_kind_not_blocked(self):
        # was a false positive before comment-stripping
        text = "---\ndescription: x\nkeywords: [a]\nkind: note  # default\n---\nb\n"
        self.assertFalse(check.has_errors(check.check_text(text)))

    def test_comment_only_description_blocked(self):
        # was a false negative: empty description hidden behind a comment
        text = "---\ndescription:  # TODO\nkeywords: [a]\n---\nb\n"
        self.assertIn("description", _fields(check.check_text(text), check.ERROR))

    def test_null_kind_treated_as_absent(self):
        text = "---\ndescription: x\nkeywords: [a]\nkind:\n---\nb\n"
        self.assertFalse(check.has_errors(check.check_text(text)))

    def test_index_kind_valid(self):
        # added `index` as the 7th kind (for INDEX.md domain homepages)
        text = "---\ndescription: domain homepage\nkeywords: [x]\nkind: index\n---\n# dom\n"
        self.assertFalse(check.has_errors(check.check_text(text)))


class TestMermaidGate(unittest.TestCase):
    def _note(self, body: str) -> str:
        return (
            contract.render_frontmatter(
                description="d", keywords=["diagram"], kind="decision"
            )
            + "\n"
            + body
        )

    def test_mermaid_blocks_extracted_with_line_number(self):
        text = self._note("# ADR\n\n```mermaid\nflowchart TD\nA --> B\n```\n")
        self.assertEqual(check.mermaid_blocks(text), [(10, "flowchart TD\nA --> B")])

    def test_valid_mermaid_block_passes(self):
        if not check.mermaid_parser_available():
            self.skipTest("Mermaid parser sidecar dependencies are not installed")
        text = self._note("# ADR\n\n```mermaid\nsequenceDiagram\nA->>B: hello\n```\n")
        self.assertFalse(check.has_errors(check.check_text(text)))

    def test_invalid_mermaid_block_errors(self):
        if not check.mermaid_parser_available():
            self.skipTest("Mermaid parser sidecar dependencies are not installed")
        text = self._note(
            "# ADR\n\n```mermaid\nsequenceDiagram\nA->>B: first; second=0\n```\n"
        )
        findings = check.check_text(text)
        self.assertTrue(check.has_errors(findings))
        self.assertIn("mermaid", _fields(findings, check.ERROR))


class TestLegacyFieldsError(unittest.TestCase):
    """killed/derived legacy fields now ERROR (fix-on-touch via --fix)."""

    def test_old_v5_note_now_errors(self):
        # domain/title/verified — a real v5 shape (e.g. old KB-SKELETON).
        text = (
            "---\n"
            "domain: meta\n"
            "title: 去中心知识库骨架\n"
            "description: 建新域 KB 照这个抄\n"
            "keywords: [知识库, KB, 骨架]\n"
            "verified: 2026-06-06\n"
            "links: [adr-001]\n"
            "---\n\n# body\n"
        )
        f = check.check_text(text)
        self.assertTrue(check.has_errors(f), "old v5 derived fields must block ")
        erred = _fields(f, check.ERROR)
        self.assertIn("domain", erred)
        self.assertIn("title", erred)
        self.assertIn("verified", erred)

    def test_killed_field_errors(self):
        text = "---\ndescription: x\nkeywords: [a]\nobject_id: 123e4567\n---\nbody\n"
        f = check.check_text(text)
        self.assertTrue(check.has_errors(f))
        self.assertIn("object_id", _fields(f, check.ERROR))


class TestCoexistenceWarnings(unittest.TestCase):
    def test_supersedes_on_decision_ok(self):
        text = "---\ndescription: x\nkeywords: [a]\nkind: decision\nsupersedes: old-slug\n---\nb\n"
        f = check.check_text(text)
        self.assertNotIn("supersedes", _fields(f, check.WARN))
        self.assertFalse(check.has_errors(f))

    def test_supersedes_on_nondecision_warns(self):
        text = "---\ndescription: x\nkeywords: [a]\nkind: reference\nsupersedes: old\n---\nb\n"
        self.assertIn("supersedes", _fields(check.check_text(text), check.WARN))

    def test_assets_on_decision_ok(self):
        text = (
            "---\n"
            "description: x\n"
            "keywords: [a]\n"
            "kind: decision\n"
            'assets: ["repo:my-service@release/fix", "data:group/dataset#v3"]\n'
            "---\n"
            "# Decision\n\n"
            "## 触达资产\n\n"
            "| asset_id | relation | change_or_usage | scope |\n"
            "|---|---|---|---|\n"
            "| `repo:my-service@release/fix` | publishes | branch | prod |\n"
            "| `data:group/dataset#v3` | changes | config | v3 |\n"
        )
        f = check.check_text(text)
        self.assertEqual(_warns(f), [])
        self.assertFalse(check.has_errors(f))

    def test_assets_on_nondecision_warns(self):
        text = (
            "---\n"
            "description: x\n"
            "keywords: [a]\n"
            "kind: reference\n"
            'assets: ["repo:my-service@release/fix"]\n'
            "---\n"
            "b\n"
        )
        self.assertIn("assets", _fields(check.check_text(text), check.WARN))

    def test_unknown_asset_prefix_warns(self):
        text = (
            "---\n"
            "description: x\n"
            "keywords: [a]\n"
            "kind: decision\n"
            'assets: ["config:blue"]\n'
            "---\n"
            "b\n"
        )
        self.assertIn("assets", _fields(check.check_text(text), check.WARN))

    def test_assets_must_be_list_warns(self):
        text = (
            "---\n"
            "description: x\n"
            "keywords: [a]\n"
            "kind: decision\n"
            "assets: repo:main\n"
            "---\n"
            "b\n"
        )
        self.assertIn("assets", _fields(check.check_text(text), check.WARN))

    def test_body_asset_section_missing_frontmatter_hint_warns(self):
        text = (
            "---\n"
            "description: x\n"
            "keywords: [a]\n"
            "kind: decision\n"
            "---\n"
            "# Decision\n\n"
            "## 触达资产\n\n"
            "| asset_id | relation | change_or_usage | scope |\n"
            "|---|---|---|---|\n"
            "| `repo:svc@main` | publishes | branch | prod |\n"
        )
        warnings = _warns(check.check_text(text))
        self.assertTrue(
            any("frontmatter assets hint is missing" in f.message for f in warnings)
        )

    def test_asset_section_examples_in_fenced_blocks_are_ignored(self):
        text = (
            "---\n"
            "description: x\n"
            "keywords: [a]\n"
            "kind: reference\n"
            "---\n"
            "# Contract\n\n"
            "```markdown\n"
            "## 触达资产\n\n"
            "| asset_id | relation |\n"
            "|---|---|\n"
            "| `repo:svc@main` | publishes |\n"
            "```\n"
        )
        self.assertEqual(check.check_text(text), [])

    def test_frontmatter_asset_without_body_semantics_warns(self):
        text = (
            "---\n"
            "description: x\n"
            "keywords: [a]\n"
            "kind: decision\n"
            'assets: ["repo:svc@main"]\n'
            "---\n"
            "# Decision\n\nbody\n"
        )
        warnings = _warns(check.check_text(text))
        self.assertTrue(
            any("frontmatter is only a hint" in f.message for f in warnings)
        )

    def test_many_assets_warns_externalize_candidate(self):
        assets = ", ".join(f'"repo:svc-{i}@main"' for i in range(13))
        text = (
            "---\n"
            "description: x\n"
            "keywords: [a]\n"
            "kind: decision\n"
            f"assets: [{assets}]\n"
            "---\n"
            "b\n"
        )
        warnings = _warns(check.check_text(text))
        self.assertTrue(any("externalize candidate" in f.message for f in warnings))

    def test_unknown_field_warns(self):
        text = "---\ndescription: x\nkeywords: [a]\nworkstream: foo\n---\nb\n"
        self.assertIn("workstream", _fields(check.check_text(text), check.WARN))


class TestStripFields(unittest.TestCase):
    """contract.strip_fields — the lossless engine behind `kb check --fix`."""

    REMOVE = contract.KILLED_FIELDS | contract.DERIVED_FIELDS

    def test_strips_killed_and_derived_keeps_rest(self):
        text = (
            "---\n"
            "domain: meta\n"
            "title: T\n"
            "description: d\n"
            "keywords: [a, b]\n"
            "kind: reference\n"
            "verified: 2026-06-06\n"
            "object_id: 123\n"
            "links: [x]\n"
            "---\n\n# body\n\ncontent\n"
        )
        new, removed = contract.strip_fields(text, self.REMOVE)
        self.assertEqual(removed, ["domain", "object_id", "title", "verified"])
        self.assertEqual(
            set(contract.parse_frontmatter(new)),
            {"description", "keywords", "kind", "links"},
        )
        self.assertIn("# body", new)
        self.assertIn("content", new)
        self.assertEqual(check.check_text(new), [], "post-fix note must be fully clean")

    def test_no_frontmatter_unchanged(self):
        text = "# README\n\nnot a note\n"
        self.assertEqual(contract.strip_fields(text, self.REMOVE), (text, []))

    def test_idempotent_on_clean_note(self):
        text = (
            contract.render_frontmatter(description="d", keywords=["a"], kind="note")
            + "\nbody\n"
        )
        new, removed = contract.strip_fields(text, self.REMOVE)
        self.assertEqual(removed, [])
        self.assertEqual(new, text)

    def test_removes_block_list_field_with_continuation(self):
        text = (
            "---\n"
            "description: d\n"
            "keywords: [a]\n"
            "object_id:\n"
            "  - one\n"
            "  - two\n"
            "links: [x]\n"
            "---\nbody\n"
        )
        new, removed = contract.strip_fields(text, self.REMOVE)
        self.assertEqual(removed, ["object_id"])
        self.assertNotIn("one", new)
        self.assertNotIn("two", new)
        self.assertIn("links: [x]", new)

    def test_keeps_comments_and_kept_field_order(self):
        text = (
            "---\n"
            "# header comment\n"
            "description: d\n"
            "verified: 2026-06-06\n"
            "keywords: [a]\n"
            "---\nbody\n"
        )
        new, _ = contract.strip_fields(text, self.REMOVE)
        self.assertIn("# header comment", new)
        self.assertLess(new.index("description"), new.index("keywords"))
        self.assertNotIn("verified", new)


class TestDomainAware(unittest.TestCase):
    """check_path only treats files under an INDEX.md domain as notes ."""

    def _repo(self, tmp: str):
        root = Path(tmp)
        (root / ".git").mkdir()
        dom = root / "decisions"
        dom.mkdir()
        (dom / "INDEX.md").write_text(
            "---\ndescription: d\nkeywords: [x]\nkind: index\n---\n# dom\n",
            encoding="utf-8",
        )
        return root, dom

    def test_note_under_domain_is_checked(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, dom = self._repo(tmp)
            note = dom / "adr.md"
            note.write_text(
                "---\ndomain: x\ndescription: d\nkeywords: [a]\n---\nb\n",
                encoding="utf-8",
            )
            f = check.check_path(note)
            self.assertIn("domain", {x.field for x in f if x.severity == check.ERROR})

    def test_pm_issue_outside_domain_is_skipped(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, _ = self._repo(tmp)
            issues = root / "issues"
            issues.mkdir()
            pm = issues / ".md"  # PM schema, no ancestor INDEX.md → not a note
            pm.write_text(
                "---\ndomain: pm\ntitle: t\nstatus: Done\nid:\n---\nbody\n",
                encoding="utf-8",
            )
            self.assertEqual(check.check_path(pm), [])

    def test_repo_root_file_is_skipped(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, _ = self._repo(tmp)
            scratch = root / "SCRATCH.md"  # root is not a domain
            scratch.write_text(
                "---\ndomain: meta\ntitle: t\n---\nb\n", encoding="utf-8"
            )
            self.assertEqual(check.check_path(scratch), [])


if __name__ == "__main__":
    unittest.main()
