"""CLI: run_new end-to-end against temp repos (domain landing, identity, guards)."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import contextlib
import io
import os

from kb.cli import CliError, _asset_reuse_candidates, main, run_new
from kb.contract import ContractError


class TestRunNew(unittest.TestCase):
    def _repo_with_domain(self, tmp: str, domain: str = "design") -> tuple[Path, Path]:
        root = Path(tmp)
        (root / ".git").mkdir()
        d = root / domain
        d.mkdir(parents=True)
        (d / "INDEX.md").write_text("# design domain\n")
        return root, d

    def test_creates_compliant_note_in_domain(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, d = self._repo_with_domain(tmp)
            res = run_new(
                "kb-frontmatter-contract",
                description="the new 5-field §存 contract",
                keywords=["frontmatter", "契约", "kb"],
                kind="reference",
                links=["adr-003"],
                code=["my-kb/tools/kb"],
                body="# Contract\n\nbody text\n",
                cwd=d,
            )
            dest = Path(res["path"])
            self.assertEqual(dest, (d / "kb-frontmatter-contract.md").resolve())
            self.assertTrue(dest.exists())
            self.assertEqual(res["domain"], "design")
            self.assertEqual(
                res["identity"], f"{root.name}:design:kb-frontmatter-contract"
            )
            self.assertEqual(res["kind"], "reference")
            text = dest.read_text()
            self.assertIn('description: "the new 5-field §存 contract"', text)
            self.assertIn("kind: reference", text)
            self.assertIn("links: [adr-003]", text)
            self.assertIn("body text", text)

    def test_nested_repo_identity_is_c2_chain(self):
        # identity skips non-domain physical segments, matching the
        # central index; the file still lands at the physical path.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / ".git").mkdir()
            leaf = root / "docs" / "source-notes" / "dm"  # only dm has an INDEX
            leaf.mkdir(parents=True)
            (leaf / "INDEX.md").write_text("# dm\n")
            res = run_new("n", description="d", keywords=["a"], body="x", cwd=leaf)
            self.assertEqual(res["domain"], "dm")
            self.assertEqual(res["identity"], f"{root.name}:dm:n")
            self.assertEqual(Path(res["path"]), (leaf / "n.md").resolve())

    def test_domain_flag_lands_in_named_domain(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, _ = self._repo_with_domain(tmp, "research")
            res = run_new(
                "agent-memory",
                description="d",
                keywords=["a"],
                domain="research",
                body="x",
                cwd=root,  # cwd is repo root, --domain selects research
            )
            self.assertEqual(res["domain"], "research")
            self.assertEqual(Path(res["path"]).parent.name, "research")

    def test_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, d = self._repo_with_domain(tmp)
            run_new("dup", description="d", keywords=["a"], body="x", cwd=d)
            with self.assertRaises(CliError):
                run_new("dup", description="d", keywords=["a"], body="x", cwd=d)

    def test_no_index_errors(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / ".git").mkdir()
            bare = root / "nope"
            bare.mkdir()
            with self.assertRaises(CliError):
                run_new("x", description="d", keywords=["a"], body="x", cwd=bare)

    def test_not_in_repo_errors(self):
        with tempfile.TemporaryDirectory() as tmp:
            bare = Path(tmp) / "loose"
            bare.mkdir()
            with self.assertRaises(CliError):
                run_new("x", description="d", keywords=["a"], body="x", cwd=bare)

    def test_empty_body_errors(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, d = self._repo_with_domain(tmp)
            with self.assertRaises(CliError):
                run_new("x", description="d", keywords=["a"], body="   \n", cwd=d)

    def test_bad_kind_errors(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, d = self._repo_with_domain(tmp)
            with self.assertRaises(ContractError):
                run_new(
                    "x", description="d", keywords=["a"], kind="essay", body="x", cwd=d
                )

    def test_assets_requires_decision_kind(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, d = self._repo_with_domain(tmp)
            with self.assertRaises(CliError):
                run_new(
                    "x",
                    description="d",
                    keywords=["a"],
                    assets=["repo:service@main"],
                    body="x",
                    cwd=d,
                )

    def test_assets_written_for_decision_note(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, d = self._repo_with_domain(tmp)
            res = run_new(
                "asset-decision",
                description="d",
                keywords=["a"],
                kind="decision",
                assets=["repo:service@main", "svc:com.example.Service#run(Long)"],
                body="# Asset decision\n\nbody\n",
                cwd=d,
            )
            text = Path(res["path"]).read_text()
            self.assertIn("kind: decision", text)
            self.assertIn(
                'assets: ["repo:service@main", "svc:com.example.Service#run(Long)"]',
                text,
            )

    def test_asset_reuse_candidates_after_three_decisions(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, d = self._repo_with_domain(tmp)
            paths = []
            for i in range(3):
                res = run_new(
                    f"asset-decision-{i}",
                    description="d",
                    keywords=["a"],
                    kind="decision",
                    assets=["data:group/dataset#v1"],
                    body=f"# Asset decision {i}\n\nbody\n",
                    cwd=d,
                )
                paths.append(Path(res["path"]))
            self.assertEqual(
                list(_asset_reuse_candidates(paths)),
                ["data:group/dataset#v1"],
            )

    def test_domain_flag_nonexistent_errors(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, _ = self._repo_with_domain(tmp)
            with self.assertRaises(CliError):
                run_new(
                    "x",
                    description="d",
                    keywords=["a"],
                    domain="ghost",
                    body="x",
                    cwd=root,
                )

    def test_domain_flag_without_own_index_errors_no_silent_fallback(self):
        # design/ has INDEX.md; design/blue/ does not. Explicit --domain must
        # NOT silently fall back to the design domain — it must error loudly.
        with tempfile.TemporaryDirectory() as tmp:
            root, _ = self._repo_with_domain(tmp, "design")
            (root / "design" / "blue").mkdir()
            with self.assertRaises(CliError):
                run_new(
                    "x",
                    description="d",
                    keywords=["a"],
                    domain="design/blue",
                    body="x",
                    cwd=root,
                )

    def test_nearest_index_wins_for_identity(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / ".git").mkdir()
            top = root / "widgets"
            deep = top / "blue"
            deep.mkdir(parents=True)
            (top / "INDEX.md").write_text("x")
            (deep / "INDEX.md").write_text("x")
            res = run_new("ap-gap", description="d", keywords=["a"], body="x", cwd=deep)
            self.assertEqual(res["domain"], "widgets/blue")
            self.assertEqual(res["identity"], f"{root.name}:widgets/blue:ap-gap")


class TestCheckDuplicateDomainsCmd(unittest.TestCase):
    """`kb check --duplicate-domains` exit code (repo-level commit-hook guard)."""

    @contextlib.contextmanager
    def _chdir(self, path: Path):
        prev = Path.cwd()
        os.chdir(path)
        try:
            yield
        finally:
            os.chdir(prev)

    def _index(self, repo: Path, rel: str):
        d = repo / rel
        d.mkdir(parents=True, exist_ok=True)
        (d / "INDEX.md").write_text(
            "---\ndescription: d\nkeywords: [x]\nkind: index\n---\n# dom\n",
            encoding="utf-8",
        )

    def test_clean_repo_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp) / "kb"
            (repo / ".git").mkdir(parents=True)
            self._index(repo, "widgets")
            self._index(repo, "widgets/blue")
            with self._chdir(repo), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(main(["check", "--duplicate-domains"]), 0)

    def test_collision_repo_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp) / "kb"
            (repo / ".git").mkdir(parents=True)
            self._index(repo, "docs/domain-map")
            (repo / "docs/domain-map/blue").mkdir(parents=True)
            self._index(repo, "docs/domain-map/blue/use-cases")
            (repo / "docs/domain-map/blue-template").mkdir(parents=True)
            self._index(repo, "docs/domain-map/blue-template/use-cases")
            err = io.StringIO()
            with self._chdir(repo), contextlib.redirect_stderr(err):
                self.assertEqual(main(["check", "--duplicate-domains"]), 1)
            self.assertIn("duplicate-domain", err.getvalue())


if __name__ == "__main__":
    unittest.main()
