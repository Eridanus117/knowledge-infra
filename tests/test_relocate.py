"""rhizome relocate — cross-repo/cross-domain note migration, frozen-aware.

Covers: dry-run touches nothing; non-frozen within-repo move (identity recompute
+ ledger); frozen content-preserving move; cross-repo move (both ledgers, repo
flips in identity); slug-rename link rewrite (frontmatter `links:` + body
`[[slug]]`, frozen referrers left alone); and the frozen-gate relocate exception
— an audited content-preserving relocate's staged delete/rename passes, while an
unrecorded delete, a tampered (edited) relocate, and an in-place edit STILL block.

All fixtures are throwaway temp git repos + temp registries; nothing touches real
rhizome KB content.
"""

from __future__ import annotations

import os
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path

from rhizome import check, contract, relocate

FROZEN_ADR = (
    "---\n"
    "description: an accepted decision\n"
    "keywords: [adr]\n"
    "kind: decision\n"
    "---\n# ADR\n\ndecided\n"
)
NOTE = (
    "---\ndescription: a living note\nkeywords: [n]\nkind: note\n---\n# Note\n\nbody\n"
)
INDEX = "---\ndescription: d\nkeywords: [x]\nkind: index\n---\n# dom\n"


def _git(root: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", "-C", str(root), *args],
        check=True,
        capture_output=True,
        text=True,
        env={
            "GIT_AUTHOR_NAME": "t",
            "GIT_AUTHOR_EMAIL": "t@t",
            "GIT_COMMITTER_NAME": "t",
            "GIT_COMMITTER_EMAIL": "t@t",
            "HOME": str(root),
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        },
    )


def _mkdomain(repo: Path, rel: str) -> Path:
    d = repo / rel
    d.mkdir(parents=True, exist_ok=True)
    (d / "INDEX.md").write_text(INDEX, encoding="utf-8")
    return d


def _write_registry(tmp: Path, entries: list[tuple[str, Path]]) -> Path:
    lines = [f'workspace_root = "{tmp}"\n']
    for name, path in entries:
        lines.append(f'\n[[source]]\nname = "{name}"\npath = "{path}"\n')
    reg = tmp / "kb-sources.toml"
    reg.write_text("".join(lines), encoding="utf-8")
    return reg


class _RegistryCase(unittest.TestCase):
    """Two registered repos (srcrepo/dstrepo) each with a docs/ domain."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name).resolve()
        self.src = self.tmp / "srcrepo"
        self.dst = self.tmp / "dstrepo"
        self.src.mkdir()
        self.dst.mkdir()
        _git(self.src, "init", "-q")
        _git(self.dst, "init", "-q")
        _mkdomain(self.src, "docs")
        _mkdomain(self.src, "archive")
        _mkdomain(self.dst, "docs")
        self.registry = _write_registry(
            self.tmp, [("srcrepo", self.src), ("dstrepo", self.dst)]
        )
        self._prev_env = os.environ.get("KB_SOURCES")
        os.environ["KB_SOURCES"] = str(self.registry)

    def tearDown(self):
        if self._prev_env is None:
            os.environ.pop("KB_SOURCES", None)
        else:
            os.environ["KB_SOURCES"] = self._prev_env
        self._tmp.cleanup()

    def _commit_all(self, repo: Path, msg: str = "seed") -> None:
        _git(repo, "add", "-A")
        _git(repo, "commit", "-q", "-m", msg)


# --------------------------------------------------------------------------- #
# planning + apply (filesystem, no git gate)
# --------------------------------------------------------------------------- #


class TestRelocateNonFrozen(_RegistryCase):
    def test_dry_run_touches_nothing(self):
        note = self.src / "docs" / "thing.md"
        note.write_text(NOTE, encoding="utf-8")
        plan = relocate.plan_relocate(
            str(note), "srcrepo:archive", cwd=self.src, registry=self.registry
        )
        self.assertEqual(plan.old_identity, "srcrepo:docs:thing")
        self.assertEqual(plan.new_identity, "srcrepo:archive:thing")
        self.assertFalse(plan.frozen)
        # plan is pure: source still there, dest absent
        self.assertTrue(note.is_file())
        self.assertFalse((self.src / "archive" / "thing.md").exists())
        self.assertFalse((self.src / relocate.RELOCATE_LEDGER_NAME).exists())

    def test_apply_moves_and_records(self):
        note = self.src / "docs" / "thing.md"
        note.write_text(NOTE, encoding="utf-8")
        plan = relocate.plan_relocate(
            str(note), "srcrepo:archive", cwd=self.src, registry=self.registry
        )
        relocate.apply_relocate(plan)
        dest = self.src / "archive" / "thing.md"
        self.assertFalse(note.exists())
        self.assertTrue(dest.is_file())
        self.assertEqual(dest.read_text(encoding="utf-8"), NOTE)
        records = relocate.read_ledger(self.src)
        self.assertEqual(len(records), 1)
        self.assertEqual(records[0]["old_identity"], "srcrepo:docs:thing")
        self.assertEqual(records[0]["new_identity"], "srcrepo:archive:thing")

    def test_target_must_be_registered_source(self):
        note = self.src / "docs" / "thing.md"
        note.write_text(NOTE, encoding="utf-8")
        with self.assertRaises(relocate.RelocateError):
            relocate.plan_relocate(
                str(note), "ghostrepo:docs", cwd=self.src, registry=self.registry
            )

    def test_target_domain_needs_index(self):
        note = self.src / "docs" / "thing.md"
        note.write_text(NOTE, encoding="utf-8")
        (self.src / "no-index-dir").mkdir()
        with self.assertRaises(relocate.RelocateError):
            relocate.plan_relocate(
                str(note), "srcrepo:no-index-dir", cwd=self.src, registry=self.registry
            )

    def test_no_overwrite(self):
        note = self.src / "docs" / "thing.md"
        note.write_text(NOTE, encoding="utf-8")
        (self.src / "archive" / "thing.md").write_text(NOTE, encoding="utf-8")
        with self.assertRaises(relocate.RelocateError):
            relocate.plan_relocate(
                str(note), "srcrepo:archive", cwd=self.src, registry=self.registry
            )


class TestRelocateFrozen(_RegistryCase):
    def test_frozen_content_preserved(self):
        adr = self.src / "docs" / "adr-1.md"
        adr.write_text(FROZEN_ADR, encoding="utf-8")
        plan = relocate.plan_relocate(
            str(adr), "srcrepo:archive", cwd=self.src, registry=self.registry
        )
        self.assertTrue(plan.frozen)
        before = relocate.content_hash(FROZEN_ADR)
        relocate.apply_relocate(plan)
        dest = self.src / "archive" / "adr-1.md"
        self.assertEqual(
            relocate.content_hash(dest.read_text(encoding="utf-8")), before
        )
        self.assertEqual(relocate.read_ledger(self.src)[0]["content_hash"], before)

    def test_frozen_with_uncommitted_edit_refused(self):
        adr = self.src / "docs" / "adr-1.md"
        adr.write_text(FROZEN_ADR, encoding="utf-8")
        self._commit_all(self.src)
        adr.write_text(FROZEN_ADR + "\nsmuggled\n", encoding="utf-8")
        with self.assertRaises(relocate.RelocateError):
            relocate.plan_relocate(
                str(adr), "srcrepo:archive", cwd=self.src, registry=self.registry
            )


class TestRelocateCrossRepo(_RegistryCase):
    def test_cross_repo_move(self):
        note = self.src / "docs" / "thing.md"
        note.write_text(NOTE, encoding="utf-8")
        plan = relocate.plan_relocate(
            str(note), "dstrepo:docs", cwd=self.src, registry=self.registry
        )
        self.assertTrue(plan.cross_repo)
        self.assertEqual(plan.new_identity, "dstrepo:docs:thing")
        relocate.apply_relocate(plan)
        self.assertFalse(note.exists())
        self.assertTrue((self.dst / "docs" / "thing.md").is_file())
        # provenance landed in BOTH repos
        self.assertEqual(len(relocate.read_ledger(self.src)), 1)
        self.assertEqual(len(relocate.read_ledger(self.dst)), 1)

    def test_same_repo_ref_flagged_cross_repo(self):
        target = self.src / "docs" / "target.md"
        target.write_text(NOTE, encoding="utf-8")
        referrer = self.src / "docs" / "ref.md"
        referrer.write_text(
            NOTE.replace("kind: note\n", "kind: note\nlinks: [target]\n"),
            encoding="utf-8",
        )
        plan = relocate.plan_relocate(
            str(target), "dstrepo:docs", cwd=self.src, registry=self.registry
        )
        paths = {r["path"] for r in plan.cross_repo_refs}
        self.assertIn(referrer, paths)


# --------------------------------------------------------------------------- #
# link rewrite (slug rename)
# --------------------------------------------------------------------------- #


class TestRewriteUnit(unittest.TestCase):
    def test_rewrite_frontmatter_and_body(self):
        text = (
            "---\n"
            "description: d\n"
            "keywords: [k]\n"
            "kind: note\n"
            "links: [old, other]\n"
            "---\n# X\n\nsee [[old]] and [[old#sec]] and [[old|alias]] and [[other]]\n"
        )
        new, n = relocate.rewrite_references(text, "old", "new")
        self.assertEqual(n, 4)  # 1 frontmatter + 3 body
        fm = contract.parse_frontmatter(new)
        self.assertEqual(fm["links"], ["new", "other"])
        self.assertIn("[[new]]", new)
        self.assertIn("[[new#sec]]", new)
        self.assertIn("[[new|alias]]", new)
        self.assertNotIn("[[old", new)

    def test_rewrite_noop_when_absent(self):
        text = NOTE
        new, n = relocate.rewrite_references(text, "missing", "x")
        self.assertEqual(n, 0)
        self.assertEqual(new, text)


class TestSlugRenameRewrite(_RegistryCase):
    def test_referrers_rewritten_frozen_left_alone(self):
        note = self.src / "docs" / "old-slug.md"
        note.write_text(NOTE, encoding="utf-8")
        live_ref = self.src / "docs" / "live.md"
        live_ref.write_text(
            NOTE.replace("kind: note\n", "kind: note\nlinks: [old-slug]\n").replace(
                "body", "see [[old-slug]]"
            ),
            encoding="utf-8",
        )
        frozen_ref = self.src / "docs" / "frozen.md"
        frozen_ref.write_text(
            FROZEN_ADR.replace(
                "kind: decision\n", "kind: decision\nlinks: [old-slug]\n"
            ),
            encoding="utf-8",
        )
        plan = relocate.plan_relocate(
            str(note), "srcrepo:archive:new-slug", cwd=self.src, registry=self.registry
        )
        self.assertTrue(plan.slug_changed)
        self.assertEqual(len(plan.rewrites), 1)  # only the living referrer
        self.assertEqual(len(plan.frozen_blocked_refs), 1)
        relocate.apply_relocate(plan)
        live_text = live_ref.read_text(encoding="utf-8")
        self.assertIn("[[new-slug]]", live_text)
        self.assertEqual(contract.parse_frontmatter(live_text)["links"], ["new-slug"])
        # frozen referrer untouched (would have been a content edit)
        self.assertIn("old-slug", frozen_ref.read_text(encoding="utf-8"))


# --------------------------------------------------------------------------- #
# frozen gate: relocate exception (real git, staged diff)
# --------------------------------------------------------------------------- #


class TestFrozenGateRelocateException(_RegistryCase):
    def _frozen_committed(self) -> Path:
        adr = self.src / "docs" / "adr-1.md"
        adr.write_text(FROZEN_ADR, encoding="utf-8")
        self._commit_all(self.src)
        return adr

    def test_within_repo_relocate_passes_gate(self):
        adr = self._frozen_committed()
        plan = relocate.plan_relocate(
            str(adr), "srcrepo:archive", cwd=self.src, registry=self.registry
        )
        relocate.apply_relocate(plan)
        _git(self.src, "add", "-A")  # stages the rename + the ledger
        self.assertEqual(check.staged_frozen_findings(self.src), [])
        # the relocated copy is a born-frozen NEW file → per-file gate passes too
        dest = self.src / "archive" / "adr-1.md"
        self.assertEqual(check.check_path(dest), [])

    def test_cross_repo_relocate_passes_source_gate(self):
        adr = self._frozen_committed()
        plan = relocate.plan_relocate(
            str(adr), "dstrepo:docs", cwd=self.src, registry=self.registry
        )
        relocate.apply_relocate(plan)
        _git(self.src, "add", "-A")  # stages the deletion + the ledger
        self.assertEqual(check.staged_frozen_findings(self.src), [])

    def test_unrecorded_delete_still_blocks(self):
        self._frozen_committed()
        _git(self.src, "rm", "-q", "docs/adr-1.md")  # plain delete, no ledger
        findings = check.staged_frozen_findings(self.src)
        self.assertTrue(check.has_errors(findings))
        self.assertTrue(any("deletion" in f.message for f in findings))

    def test_tampered_relocate_still_blocks(self):
        # legit relocate, then EDIT the destination before staging: the staged
        # copy's hash no longer matches the ledger → not content-preserving → block.
        adr = self._frozen_committed()
        plan = relocate.plan_relocate(
            str(adr), "srcrepo:archive", cwd=self.src, registry=self.registry
        )
        relocate.apply_relocate(plan)
        dest = self.src / "archive" / "adr-1.md"
        dest.write_text(FROZEN_ADR + "\nsmuggled edit\n", encoding="utf-8")
        _git(self.src, "add", "-A")
        self.assertTrue(check.has_errors(check.staged_frozen_findings(self.src)))

    def test_inplace_edit_still_blocks(self):
        # an in-place edit is an `M`, handled by the per-file gate — relocate
        # exception must not touch it.
        adr = self._frozen_committed()
        adr.write_text(FROZEN_ADR + "\nedited in place\n", encoding="utf-8")
        self.assertTrue(check.has_errors(check.check_path(adr)))

    def test_wrong_hash_ledger_does_not_unlock(self):
        # a ledger row whose content hash does NOT match the HEAD content must
        # not exempt the delete (forged/stale provenance).
        self._frozen_committed()
        relocate.append_ledger(
            self.src,
            chash="deadbeef" * 8,
            old_identity="srcrepo:docs:adr-1",
            new_identity="srcrepo:archive:adr-1",
            old_rel="docs/adr-1.md",
            new_rel="archive/adr-1.md",
        )
        _git(self.src, "rm", "-q", "docs/adr-1.md")
        self.assertTrue(check.has_errors(check.staged_frozen_findings(self.src)))

    def test_cross_repo_forged_ledger_without_copy_blocks(self):
        # THE cross-repo bypass: the HEAD hash is public, so anyone can forge a
        # ledger row claiming a cross-repo relocate WITHOUT ever putting a real
        # copy in the target. Trusting the ledger alone would green-light deleting
        # any frozen doc; the gate must read dstrepo's tree and refuse.
        adr = self._frozen_committed()
        head = adr.read_text(encoding="utf-8")
        relocate.append_ledger(
            self.src,
            chash=relocate.content_hash(head),
            old_identity="srcrepo:docs:adr-1",
            new_identity="dstrepo:docs:adr-1",  # claims cross-repo
            old_rel="docs/adr-1.md",
            new_rel="docs/adr-1.md",
        )
        # NO copy placed in dstrepo
        _git(self.src, "rm", "-q", "docs/adr-1.md")
        self.assertTrue(check.has_errors(check.staged_frozen_findings(self.src)))

    def test_cross_repo_tampered_target_copy_blocks(self):
        # forged ledger + a TAMPERED copy in the target (content differs from
        # HEAD): the target-tree hash won't match HEAD, so the delete stays blocked.
        adr = self._frozen_committed()
        head = adr.read_text(encoding="utf-8")
        (self.dst / "docs" / "adr-1.md").write_text(
            head + "\ntampered in target\n", encoding="utf-8"
        )
        relocate.append_ledger(
            self.src,
            chash=relocate.content_hash(head),
            old_identity="srcrepo:docs:adr-1",
            new_identity="dstrepo:docs:adr-1",
            old_rel="docs/adr-1.md",
            new_rel="docs/adr-1.md",
        )
        _git(self.src, "rm", "-q", "docs/adr-1.md")
        self.assertTrue(check.has_errors(check.staged_frozen_findings(self.src)))


# --------------------------------------------------------------------------- #
# end-to-end via the live pre-commit gate (env-inheritance path, like amend)
# --------------------------------------------------------------------------- #

_SRC = str(Path(__file__).resolve().parents[1] / "src")


class TestRelocateThroughLiveGate(_RegistryCase):
    """A real pre-commit hook running the worktree's `rhizome.cli check
    --staged-frozen` accepts the relocate commit end-to-end."""

    def _install_gate(self, repo: Path) -> None:
        hook = repo / ".git" / "hooks" / "pre-commit"
        import sys

        hook.write_text(
            textwrap.dedent(f"""\
            #!/bin/sh
            PYTHONPATH={_SRC} {sys.executable} -m rhizome.cli check --staged-frozen || exit 1
            files=$(git diff --cached --name-only --diff-filter=ACMR -- '*.md')
            [ -z "$files" ] && exit 0
            PYTHONPATH={_SRC} {sys.executable} -m rhizome.cli check $files
        """),
            encoding="utf-8",
        )
        hook.chmod(0o755)

    def test_within_repo_relocate_commits(self):
        adr = self.src / "docs" / "adr-1.md"
        adr.write_text(FROZEN_ADR, encoding="utf-8")
        self._commit_all(self.src)
        self._install_gate(self.src)
        plan = relocate.plan_relocate(
            str(adr), "srcrepo:archive", cwd=self.src, registry=self.registry
        )
        relocate.apply_relocate(plan)
        _git(self.src, "add", "-A")
        # commit must SUCCEED — the gate recognises the audited relocate
        proc = subprocess.run(
            ["git", "-C", str(self.src), "commit", "-m", "relocate adr"],
            capture_output=True,
            text=True,
            env={
                "GIT_AUTHOR_NAME": "t",
                "GIT_AUTHOR_EMAIL": "t@t",
                "GIT_COMMITTER_NAME": "t",
                "GIT_COMMITTER_EMAIL": "t@t",
                "HOME": str(self.src),
                "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
                "KB_SOURCES": str(self.registry),
            },
        )
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)


class TestBatchAtomicity(_RegistryCase):
    def test_bad_move_aborts_before_touching_anything(self):
        # two-phase apply: a bad move (missing target domain) must abort the whole
        # batch during planning, before ANY file is moved or ledger written.
        n1 = self.src / "docs" / "a.md"
        n1.write_text(NOTE, encoding="utf-8")
        n2 = self.src / "docs" / "b.md"
        n2.write_text(NOTE, encoding="utf-8")
        plan_path = self.tmp / "batch.toml"
        plan_path.write_text(
            f'[[move]]\nsource = "{n1}"\nto = "srcrepo:archive"\n'
            f'[[move]]\nsource = "{n2}"\nto = "srcrepo:ghost-domain"\n',
            encoding="utf-8",
        )
        with self.assertRaises(relocate.RelocateError):
            relocate.run_relocate(
                batch=str(plan_path), apply=True, cwd=self.src, registry=self.registry
            )
        # first move NOT applied — nothing touched
        self.assertTrue(n1.is_file())
        self.assertFalse((self.src / "archive" / "a.md").exists())
        self.assertFalse((self.src / relocate.RELOCATE_LEDGER_NAME).exists())


if __name__ == "__main__":
    unittest.main()
