"""Repo-level duplicate-domain guard (domain-guard spec).

Two physical INDEX.md paths whose C2 node-chain domains collide must FAIL `kb
check`, naming both physical paths and the contended domain. The C2 node-chain
口径 is deliberately distinct from `kb new`'s physical-path derive_domain; the
divergence is pinned here so neither口径 can drift silently.
"""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from rhizome import check, contract


def _repo(base: Path) -> Path:
    r = base / "kb"
    (r / ".git").mkdir(parents=True)
    return r


def _index(repo: Path, rel: str):
    """Place an INDEX.md (a domain node) at repo/<rel>."""
    d = repo / rel
    d.mkdir(parents=True, exist_ok=True)
    (d / "INDEX.md").write_text(
        "---\ndescription: d\nkeywords: [x]\nkind: index\n---\n# dom\n",
        encoding="utf-8",
    )


def _plain_dir(repo: Path, rel: str):
    """A physical directory WITHOUT an INDEX.md (a skipped non-domain segment)."""
    (repo / rel).mkdir(parents=True, exist_ok=True)


class TestNodeChainDerivation(unittest.TestCase):
    def test_skips_nondomain_segments(self):
        # The contract example: docs/source-notes are non-domain physical dirs;
        # domain-map/blue/use-cases all carry INDEX.md.
        with tempfile.TemporaryDirectory() as tmp:
            repo = _repo(Path(tmp))
            _plain_dir(repo, "docs/source-notes")
            _index(repo, "docs/source-notes/domain-map")
            _index(repo, "docs/source-notes/domain-map/blue")
            _index(repo, "docs/source-notes/domain-map/blue/use-cases")
            leaf = repo / "docs/source-notes/domain-map/blue/use-cases"
            self.assertEqual(
                contract.derive_node_chain_domain(leaf, repo),
                "domain-map/blue/use-cases",
            )

    def test_diverges_from_physical_derive_domain(self):
        # Pin the drift: the two口径 disagree exactly when a non-domain segment
        # sits above a domain dir. If kb new is ever flipped to C2 this still
        # documents the contract; if either口径 changes shape this test breaks.
        with tempfile.TemporaryDirectory() as tmp:
            repo = _repo(Path(tmp))
            _plain_dir(repo, "docs/source-notes")
            _index(repo, "docs/source-notes/domain-map")
            leaf = repo / "docs/source-notes/domain-map"
            self.assertEqual(
                contract.derive_node_chain_domain(leaf, repo), "domain-map"
            )
            self.assertEqual(
                contract.derive_domain(leaf, repo), "docs/source-notes/domain-map"
            )

    def test_flat_repo_two_口径_agree(self):
        # When every physical segment is itself a domain, C2 == physical.
        with tempfile.TemporaryDirectory() as tmp:
            repo = _repo(Path(tmp))
            _index(repo, "widgets")
            _index(repo, "widgets/blue")
            leaf = repo / "widgets/blue"
            self.assertEqual(
                contract.derive_node_chain_domain(leaf, repo),
                contract.derive_domain(leaf, repo),
            )

    def test_repo_root_is_empty_domain(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = _repo(Path(tmp))
            self.assertEqual(contract.derive_node_chain_domain(repo, repo), "")


class TestDuplicateDomainGuard(unittest.TestCase):
    def test_nested_skip_no_collision_passes(self):
        # Positive: skipped non-domain segments, deeply nested, no two chains
        # collide → no findings.
        with tempfile.TemporaryDirectory() as tmp:
            repo = _repo(Path(tmp))
            (repo / "INDEX.md").write_text(
                "---\ndescription: root\nkeywords: [x]\n---\n", encoding="utf-8"
            )  # root INDEX.md → empty domain, never collides
            _plain_dir(repo, "docs/source-notes")
            _index(repo, "docs/source-notes/domain-map")
            _index(repo, "docs/source-notes/domain-map/blue")
            _index(repo, "docs/source-notes/domain-map/blue/use-cases")
            _index(repo, "docs/source-notes/domain-map/blue-template")
            _index(repo, "docs/source-notes/domain-map/blue-template/use-cases")
            self.assertEqual(check.duplicate_domain_findings(repo), [])

    def test_skip_collision_fails_and_names_both_paths(self):
        # The real bug: blue & blue-template intermediates lack
        # INDEX.md, so both .../use-cases derive to domain-map/use-cases.
        with tempfile.TemporaryDirectory() as tmp:
            repo = _repo(Path(tmp))
            _index(repo, "docs/source-notes/domain-map")
            _plain_dir(repo, "docs/source-notes/domain-map/blue")
            _index(repo, "docs/source-notes/domain-map/blue/use-cases")
            _plain_dir(repo, "docs/source-notes/domain-map/blue-template")
            _index(repo, "docs/source-notes/domain-map/blue-template/use-cases")

            findings = check.duplicate_domain_findings(repo)
            self.assertTrue(check.has_errors(findings))
            self.assertEqual(len(findings), 1)
            msg = findings[0].message
            self.assertIn("domain-map/use-cases", msg)
            self.assertIn("docs/source-notes/domain-map/blue/use-cases", msg)
            self.assertIn("docs/source-notes/domain-map/blue-template/use-cases", msg)
            self.assertIn("INDEX.md", msg)  # the fix hint

    def test_root_index_only_no_collision(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = _repo(Path(tmp))
            (repo / "INDEX.md").write_text(
                "---\ndescription: root\nkeywords: [x]\n---\n", encoding="utf-8"
            )
            self.assertEqual(check.duplicate_domain_findings(repo), [])

    def test_walk_skips_hidden_and_vendor_dirs(self):
        # An INDEX.md buried in .git/.venv/node_modules must not be walked, so it
        # cannot manufacture a phantom collision.
        with tempfile.TemporaryDirectory() as tmp:
            repo = _repo(Path(tmp))
            _index(repo, "widgets")
            for vendor in (".venv/widgets", "node_modules/widgets", ".git/widgets"):
                d = repo / vendor
                d.mkdir(parents=True, exist_ok=True)
                (d / "INDEX.md").write_text(
                    "---\ndescription: d\nkeywords: [x]\n---\n", encoding="utf-8"
                )
            self.assertEqual(check.duplicate_domain_findings(repo), [])


if __name__ == "__main__":
    unittest.main()
