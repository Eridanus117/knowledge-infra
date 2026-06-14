"""Frozen gate (frozen-gate spec): status single-value,
HEAD-frozen edit blocking, staged delete/rename guard, --fix value-awareness."""

from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path

from kb import check, cli, contract


def _fields(findings, severity):
    return {f.field for f in findings if f.severity == severity}


FROZEN_ADR = (
    "---\n"
    "description: an accepted decision\n"
    "keywords: [adr]\n"
    "kind: decision\n"
    "---\n# ADR\n\ndecided\n"
)
FROZEN_REF = (
    "---\n"
    "description: a frozen reference snapshot\n"
    "keywords: [ref]\n"
    "kind: reference\n"
    "status: frozen\n"
    "---\n# Ref\n\nsnapshot\n"
)
LIVING_REF = (
    "---\n"
    "description: a living reference\n"
    "keywords: [ref]\n"
    "kind: reference\n"
    "---\n# Ref\n\ncurrent truth\n"
)


class TestStatusSingleValue(unittest.TestCase):
    def test_status_frozen_is_clean(self):
        f = check.check_text(FROZEN_REF)
        self.assertEqual(f, [], "status: frozen must pass ")

    def test_status_other_value_errors(self):
        text = FROZEN_REF.replace("status: frozen", "status: draft")
        self.assertIn("status", _fields(check.check_text(text), check.ERROR))

    def test_status_null_errors(self):
        text = FROZEN_REF.replace("status: frozen", "status:")
        self.assertIn("status", _fields(check.check_text(text), check.ERROR))

    def test_status_not_unknown_field(self):
        self.assertNotIn("status", _fields(check.check_text(FROZEN_REF), check.WARN))

    def test_status_no_longer_killed(self):
        self.assertNotIn("status", contract.KILLED_FIELDS)

    def test_is_frozen_fm(self):
        self.assertTrue(contract.is_frozen_fm({"kind": "decision"}))
        self.assertTrue(contract.is_frozen_fm({"status": "frozen"}))
        self.assertFalse(contract.is_frozen_fm({"kind": "reference"}))
        self.assertFalse(contract.is_frozen_fm({"status": "draft"}))
        self.assertFalse(contract.is_frozen_fm(None))


def _git(root: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-C", str(root), *args],
        check=True,
        capture_output=True,
        env={
            "GIT_AUTHOR_NAME": "t",
            "GIT_AUTHOR_EMAIL": "t@t",
            "GIT_COMMITTER_NAME": "t",
            "GIT_COMMITTER_EMAIL": "t@t",
            "HOME": str(root),
            "PATH": "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin",
        },
    )


class _GitRepoCase(unittest.TestCase):
    """A real git repo with a decisions/ domain and committed docs."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name).resolve()
        _git(self.root, "init", "-q")
        dom = self.root / "decisions"
        dom.mkdir()
        (dom / "INDEX.md").write_text(
            "---\ndescription: d\nkeywords: [x]\nkind: index\n---\n# dom\n",
            encoding="utf-8",
        )
        self.adr = dom / "adr-001-x.md"
        self.adr.write_text(FROZEN_ADR, encoding="utf-8")
        self.frozen_ref = dom / "frozen-ref.md"
        self.frozen_ref.write_text(FROZEN_REF, encoding="utf-8")
        self.living = dom / "living-ref.md"
        self.living.write_text(LIVING_REF, encoding="utf-8")
        _git(self.root, "add", "-A")
        _git(self.root, "commit", "-q", "-m", "seed")

    def tearDown(self):
        self._tmp.cleanup()


class TestFrozenGate(_GitRepoCase):
    def test_unmodified_frozen_doc_passes(self):
        self.assertEqual(check.check_path(self.adr), [])

    def test_modified_decision_blocks(self):
        self.adr.write_text(FROZEN_ADR + "\nedited\n", encoding="utf-8")
        f = check.check_path(self.adr)
        self.assertTrue(check.has_errors(f))
        self.assertTrue(any("frozen document modified" in x.message for x in f))

    def test_modified_status_frozen_blocks(self):
        self.frozen_ref.write_text(FROZEN_REF + "\nedited\n", encoding="utf-8")
        self.assertTrue(check.has_errors(check.check_path(self.frozen_ref)))

    def test_downgrade_status_blocks(self):
        # flipping frozen→(absent) and editing must still be caught: HEAD rules
        self.frozen_ref.write_text(LIVING_REF, encoding="utf-8")
        self.assertTrue(check.has_errors(check.check_path(self.frozen_ref)))

    def test_living_doc_edit_passes(self):
        self.living.write_text(LIVING_REF + "\nupdated\n", encoding="utf-8")
        self.assertEqual(check.check_path(self.living), [])

    def test_new_born_frozen_file_passes(self):
        new = self.adr.parent / "adr-002-y.md"
        new.write_text(FROZEN_ADR, encoding="utf-8")
        self.assertEqual(check.check_path(new), [])

    def test_no_git_repo_skips_gate(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / ".git").mkdir()  # fake .git: git commands fail → gate skips
            dom = root / "decisions"
            dom.mkdir()
            (dom / "INDEX.md").write_text(
                "---\ndescription: d\nkeywords: [x]\nkind: index\n---\n# d\n",
                encoding="utf-8",
            )
            doc = dom / "adr.md"
            doc.write_text(FROZEN_ADR, encoding="utf-8")
            self.assertEqual(check.check_path(doc), [])


class TestStagedFrozen(_GitRepoCase):
    def test_staged_delete_of_frozen_blocks(self):
        _git(self.root, "rm", "-q", "decisions/adr-001-x.md")
        f = check.staged_frozen_findings(self.root)
        self.assertTrue(check.has_errors(f))
        self.assertTrue(any("deletion" in x.message for x in f))

    def test_staged_rename_of_frozen_blocks(self):
        _git(self.root, "mv", "decisions/adr-001-x.md", "decisions/adr-001-z.md")
        f = check.staged_frozen_findings(self.root)
        self.assertTrue(check.has_errors(f))
        self.assertTrue(any("rename" in x.message for x in f))

    def test_staged_delete_of_living_passes(self):
        _git(self.root, "rm", "-q", "decisions/living-ref.md")
        self.assertEqual(check.staged_frozen_findings(self.root), [])

    def test_clean_index_passes(self):
        self.assertEqual(check.staged_frozen_findings(self.root), [])

    def test_delete_outside_domain_passes(self):
        # a frozen-shaped doc outside any INDEX.md domain is not a KB note
        stray = self.root / "stray.md"
        stray.write_text(FROZEN_ADR, encoding="utf-8")
        _git(self.root, "add", "stray.md")
        _git(self.root, "commit", "-q", "-m", "stray")
        _git(self.root, "rm", "-q", "stray.md")
        self.assertEqual(check.staged_frozen_findings(self.root), [])


class TestFixValueAware(_GitRepoCase):
    def test_fix_strips_nonfrozen_status(self):
        doc = self.living
        doc.write_text(
            LIVING_REF.replace("---\n# Ref", "status: draft\n---\n# Ref"),
            encoding="utf-8",
        )
        fixed, skipped = cli._fix_paths([doc])
        self.assertEqual(skipped, [])
        self.assertIn("status", fixed[str(doc)])
        self.assertNotIn("status", doc.read_text(encoding="utf-8"))

    def test_fix_keeps_legal_frozen_on_new_file(self):
        new = self.adr.parent / "new-frozen.md"
        new.write_text(FROZEN_REF + "\nobject_id: 1\n", encoding="utf-8")
        # malformed placement aside: put object_id in frontmatter properly
        new.write_text(
            FROZEN_REF.replace("---\n# Ref", "object_id: 1\n---\n# Ref"),
            encoding="utf-8",
        )
        fixed, skipped = cli._fix_paths([new])
        self.assertEqual(skipped, [])
        self.assertEqual(fixed[str(new)], ["object_id"])
        self.assertIn("status: frozen", new.read_text(encoding="utf-8"))

    def test_fix_skips_head_frozen_doc(self):
        self.adr.write_text(
            FROZEN_ADR.replace("---\n# ADR", "object_id: 1\n---\n# ADR"),
            encoding="utf-8",
        )
        before = self.adr.read_text(encoding="utf-8")
        fixed, skipped = cli._fix_paths([self.adr])
        self.assertEqual(fixed, {})
        self.assertEqual(skipped, [str(self.adr)])
        self.assertEqual(self.adr.read_text(encoding="utf-8"), before)


if __name__ == "__main__":
    unittest.main()
