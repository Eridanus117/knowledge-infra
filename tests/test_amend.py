"""rhizome amend — audited frozen-doc edit channel.

Covers: amend happy path (commit + trailer + ledger audit visible); the frozen
guard STILL blocks an unapproved frozen edit; amend does NOT bypass frontmatter
content errors; and the approval marker is narrow — it frees only the one named
file and leaves every other frozen doc still gated.

The integration cases use a real git repo whose pre-commit hook runs the
WORKTREE's `rhizome.cli check` (via PYTHONPATH), so the env-inheritance path through
`git commit` → hook → `rhizome check` is exercised against the code under test,
not whatever `rhizome` happens to be installed on PATH.
"""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

from rhizome import amend, check

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
    "---\n# Ref\n\ncurrent truth\n"
)

# src dir of the code under test → goes on the hook's PYTHONPATH.
_SRC = str(Path(__file__).resolve().parents[1] / "src")


def _git(
    root: Path, *args: str, env: dict | None = None
) -> subprocess.CompletedProcess:
    base = {
        "GIT_AUTHOR_NAME": "t",
        "GIT_AUTHOR_EMAIL": "t@t",
        "GIT_COMMITTER_NAME": "t",
        "GIT_COMMITTER_EMAIL": "t@t",
        "HOME": str(root),
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
    }
    if env:
        base.update(env)
    return subprocess.run(
        ["git", "-C", str(root), *args],
        capture_output=True,
        text=True,
        env=base,
    )


class _GateRepoCase(unittest.TestCase):
    """A real git repo with a decisions/ domain, committed docs, and a live
    pre-commit gate that runs the worktree's `rhizome.cli check` on staged files."""

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
        self.adr2 = dom / "adr-002-y.md"
        self.adr2.write_text(
            FROZEN_ADR.replace("decided", "also decided"), encoding="utf-8"
        )
        self.living = dom / "living-ref.md"
        self.living.write_text(LIVING_REF, encoding="utf-8")
        _git(self.root, "add", "-A")
        _git(self.root, "commit", "-q", "-m", "seed")

        # A real pre-commit gate: run the code-under-test's check on staged .md.
        hook = self.root / ".git" / "hooks" / "pre-commit"
        hook.write_text(
            textwrap.dedent(f"""\
            #!/bin/sh
            files=$(git diff --cached --name-only --diff-filter=ACMR -- '*.md')
            [ -z "$files" ] && exit 0
            PYTHONPATH={_SRC} {sys.executable} -m rhizome.cli check $files
        """),
            encoding="utf-8",
        )
        hook.chmod(0o755)

    def tearDown(self):
        self._tmp.cleanup()

    def _run_amend(self, file: Path, reason: str) -> dict:
        # run_amend spawns `git commit`; inject deterministic identity + the
        # worktree on PYTHONPATH so the hook resolves the code under test.
        prev = dict(os.environ)
        os.environ.update(
            {
                "GIT_AUTHOR_NAME": "t",
                "GIT_AUTHOR_EMAIL": "t@t",
                "GIT_COMMITTER_NAME": "t",
                "GIT_COMMITTER_EMAIL": "t@t",
                "HOME": str(self.root),
                "PYTHONPATH": _SRC,
            }
        )
        try:
            return amend.run_amend(str(file), reason=reason, cwd=self.root)
        finally:
            os.environ.clear()
            os.environ.update(prev)


class TestAmendHappyPath(_GateRepoCase):
    def test_amend_commits_with_trailer_and_ledger(self):
        self.adr.write_text(FROZEN_ADR + "\namended detail\n", encoding="utf-8")
        result = self._run_amend(self.adr, "fix typo in decided clause")

        # committed
        self.assertNotEqual(result["commit"], "?")
        log = _git(self.root, "log", "-1", "--format=%B").stdout
        self.assertIn("Frozen-Amend-Approved: fix typo in decided clause", log)
        self.assertIn("amend(frozen): decisions/adr-001-x.md", log)

        # working tree is clean for that file (the edit landed in the commit)
        st = _git(
            self.root, "status", "--porcelain", "--", "decisions/adr-001-x.md"
        ).stdout
        self.assertEqual(st.strip(), "")
        committed = _git(self.root, "show", "HEAD:decisions/adr-001-x.md").stdout
        self.assertIn("amended detail", committed)

        # ledger audit line is present and committed
        ledger = Path(result["ledger"])
        self.assertTrue(ledger.is_file())
        ledger_text = ledger.read_text(encoding="utf-8")
        self.assertIn("decisions/adr-001-x.md", ledger_text)
        self.assertIn("fix typo in decided clause", ledger_text)
        tracked = _git(
            self.root,
            "ls-files",
            "--",
            ".frozen-amend-ledger",
            "decisions/.frozen-amend-ledger",
        ).stdout
        self.assertIn(".frozen-amend-ledger", tracked)

    def test_amend_works_on_status_frozen_ref(self):
        ref = self.root / "decisions" / "frozen-ref.md"
        ref.write_text(FROZEN_REF, encoding="utf-8")
        _git(self.root, "add", "-A")
        _git(self.root, "commit", "-q", "-m", "add frozen ref")
        ref.write_text(FROZEN_REF + "\nmore\n", encoding="utf-8")
        result = self._run_amend(ref, "correct broken link")
        self.assertNotEqual(result["commit"], "?")


class TestAmendRejections(_GateRepoCase):
    def test_amend_requires_changes(self):
        with self.assertRaises(amend.AmendError):
            self._run_amend(self.adr, "no edits made")  # unchanged → nothing to amend

    def test_amend_refuses_non_frozen_doc(self):
        self.living.write_text(LIVING_REF + "\nupdated\n", encoding="utf-8")
        with self.assertRaises(amend.AmendError):
            self._run_amend(self.living, "this should not need amend")

    def test_amend_requires_reason(self):
        self.adr.write_text(FROZEN_ADR + "\nx\n", encoding="utf-8")
        with self.assertRaises(amend.AmendUsageError):
            self._run_amend(self.adr, "   ")

    def test_amend_does_not_bypass_frontmatter_error(self):
        # Break the frontmatter (drop required `description`) while editing a
        # frozen doc. The frozen block is approved, but the CONTENT check must
        # still fire, so the commit must FAIL — amend is not --no-verify.
        broken = (
            "---\n"
            "keywords: [adr]\n"
            "kind: decision\n"
            "---\n# ADR\n\ndecided then broke frontmatter\n"
        )
        self.adr.write_text(broken, encoding="utf-8")
        with self.assertRaises(amend.AmendError) as ctx:
            self._run_amend(self.adr, "amend but with bad frontmatter")
        # the failure is the content gate, not the frozen gate
        self.assertIn("refused", str(ctx.exception).lower())
        # nothing got committed: HEAD still has the original frozen ADR
        head = _git(self.root, "show", "HEAD:decisions/adr-001-x.md").stdout
        self.assertNotIn("broke frontmatter", head)


class TestUnapprovedStillBlocked(_GateRepoCase):
    def test_unapproved_frozen_edit_still_blocks_at_gate(self):
        # Edit + stage + plain `git commit` (no amend, no env) must be rejected.
        self.adr.write_text(FROZEN_ADR + "\nsneaky edit\n", encoding="utf-8")
        _git(self.root, "add", "decisions/adr-001-x.md")
        proc = _git(self.root, "commit", "-m", "sneaky")
        self.assertNotEqual(
            proc.returncode, 0, "frozen edit must be blocked without approval"
        )
        self.assertIn("frozen document modified", (proc.stdout + proc.stderr))

    def test_check_findings_block_without_env(self):
        self.adr.write_text(FROZEN_ADR + "\nedit\n", encoding="utf-8")
        os.environ.pop(check._AMEND_APPROVED_ENV, None)
        self.assertTrue(check.has_errors(check.check_path(self.adr)))


class TestApprovalIsNarrow(unittest.TestCase):
    """The RHIZOME_AMEND_APPROVED marker frees ONLY the one named file."""

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
        self.a = dom / "adr-a.md"
        self.a.write_text(FROZEN_ADR, encoding="utf-8")
        self.b = dom / "adr-b.md"
        self.b.write_text(
            FROZEN_ADR.replace("decided", "other decision"), encoding="utf-8"
        )
        _git(self.root, "add", "-A")
        _git(self.root, "commit", "-q", "-m", "seed")
        self.a.write_text(FROZEN_ADR + "\nedit a\n", encoding="utf-8")
        self.b.write_text(
            FROZEN_ADR.replace("decided", "other decision") + "\nedit b\n",
            encoding="utf-8",
        )
        self._prev = dict(os.environ)

    def tearDown(self):
        os.environ.clear()
        os.environ.update(self._prev)
        self._tmp.cleanup()

    def test_approved_file_passes_other_frozen_file_still_blocks(self):
        os.environ[check._AMEND_APPROVED_ENV] = str(self.a)
        # the named file is freed
        self.assertEqual(
            check.frozen_gate_findings(self.a, self.a.read_text(encoding="utf-8")), []
        )
        # a DIFFERENT frozen file in the same commit is still blocked
        self.assertTrue(
            check.has_errors(
                check.frozen_gate_findings(self.b, self.b.read_text(encoding="utf-8"))
            )
        )

    def test_unset_env_blocks_everything(self):
        os.environ.pop(check._AMEND_APPROVED_ENV, None)
        self.assertTrue(
            check.has_errors(
                check.frozen_gate_findings(self.a, self.a.read_text(encoding="utf-8"))
            )
        )

    def test_empty_env_is_not_a_wildcard(self):
        os.environ[check._AMEND_APPROVED_ENV] = ""
        self.assertTrue(
            check.has_errors(
                check.frozen_gate_findings(self.a, self.a.read_text(encoding="utf-8"))
            )
        )

    def test_match_is_resolved_path_not_string(self):
        # a dotted/relative spelling of the same file still matches by resolve()
        os.environ[check._AMEND_APPROVED_ENV] = str(self.a.parent / "." / self.a.name)
        self.assertEqual(
            check.frozen_gate_findings(self.a, self.a.read_text(encoding="utf-8")), []
        )


if __name__ == "__main__":
    unittest.main()
