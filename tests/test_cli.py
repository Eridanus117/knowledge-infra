"""CLI: run_new end-to-end against temp repos (domain landing, identity, guards)."""

from __future__ import annotations

import argparse
import contextlib
import io
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from rhizome.cli import (
    CliError,
    _asset_reuse_candidates,
    _read_new_body,
    main,
    run_new,
)
from rhizome.contract import ContractError


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


@contextlib.contextmanager
def _chdir(path: Path):
    prev = Path.cwd()
    os.chdir(path)
    try:
        yield
    finally:
        os.chdir(prev)


class TestNewBodyInput(unittest.TestCase):
    """`rhizome new` body source: --body-file (`-`=stdin) or default stdin (Tier 1)."""

    def _repo_with_domain(self, tmp: str, domain: str = "design") -> tuple[Path, Path]:
        root = Path(tmp)
        (root / ".git").mkdir()
        d = root / domain
        d.mkdir(parents=True)
        (d / "INDEX.md").write_text("# design domain\n")
        return root, d

    def test_body_file_reads_from_file(self):
        args = argparse.Namespace(body_file=None)
        with tempfile.TemporaryDirectory() as tmp:
            bf = Path(tmp) / "body.md"
            bf.write_text("# Title\n\n$X `code` )unbalanced( — CJK\uff0c标点\n")
            args.body_file = str(bf)
            self.assertEqual(_read_new_body(args), bf.read_text())

    def test_body_file_dash_reads_stdin(self):
        args = argparse.Namespace(body_file="-")
        with mock.patch("sys.stdin", io.StringIO("piped body")):
            self.assertEqual(_read_new_body(args), "piped body")

    def test_body_file_missing_returns_none_and_warns(self):
        args = argparse.Namespace(body_file="/no/such/body.md")
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            self.assertIsNone(_read_new_body(args))
        self.assertIn("--body-file", err.getvalue())

    def test_body_file_non_utf8_returns_none(self):
        # non-UTF8 file → UnicodeDecodeError (a ValueError, not OSError); must be
        # caught as a channel error (None → exit 2), never an uncaught traceback.
        with tempfile.TemporaryDirectory() as tmp:
            bf = Path(tmp) / "latin1.md"
            bf.write_bytes(b"# T\n\n\xff\xfe not utf-8 \x80\x81\n")
            args = argparse.Namespace(body_file=str(bf))
            err = io.StringIO()
            with contextlib.redirect_stderr(err):
                self.assertIsNone(_read_new_body(args))
            self.assertIn("--body-file", err.getvalue())

    def test_new_end_to_end_non_utf8_exits_2_no_crash(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, d = self._repo_with_domain(tmp)
            bf = Path(tmp) / "bad.md"
            bf.write_bytes(b"\xff\xfe\x00\x80")
            with _chdir(d), contextlib.redirect_stderr(io.StringIO()):
                rc = main(["new", "x", "-d", "d", "-k", "a", "--body-file", str(bf)])
            self.assertEqual(rc, 2)
            self.assertFalse((d / "x.md").exists())

    def test_empty_content_body_file_exits_1(self):
        # empty file is a valid channel but an empty body → run_new's exit-1
        # contract, not the exit-2 channel path.
        with tempfile.TemporaryDirectory() as tmp:
            _, d = self._repo_with_domain(tmp)
            bf = Path(tmp) / "empty.md"
            bf.write_text("   \n")
            with _chdir(d), contextlib.redirect_stderr(io.StringIO()):
                rc = main(["new", "x", "-d", "d", "-k", "a", "--body-file", str(bf)])
            self.assertEqual(rc, 1)

    def test_default_reads_stdin_backward_compat(self):
        args = argparse.Namespace(body_file=None)
        with mock.patch("sys.stdin", io.StringIO("legacy stdin body")):
            self.assertEqual(_read_new_body(args), "legacy stdin body")

    def test_default_tty_returns_none_and_mentions_body_file(self):
        args = argparse.Namespace(body_file=None)
        fake = io.StringIO("")
        fake.isatty = lambda: True  # type: ignore[method-assign]
        err = io.StringIO()
        with mock.patch("sys.stdin", fake), contextlib.redirect_stderr(err):
            self.assertIsNone(_read_new_body(args))
        self.assertIn("--body-file", err.getvalue())

    def test_new_end_to_end_via_body_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, d = self._repo_with_domain(tmp)
            bf = Path(tmp) / "b.md"
            bf.write_text("# Note\n\nbody via file\n")
            out = io.StringIO()
            with (
                _chdir(d),
                contextlib.redirect_stdout(out),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                rc = main(
                    ["new", "via-file", "-d", "d", "-k", "a", "--body-file", str(bf)]
                )
            self.assertEqual(rc, 0)
            note = d / "via-file.md"
            self.assertTrue(note.exists())
            self.assertIn("body via file", note.read_text())

    def test_empty_body_message_mentions_body_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, d = self._repo_with_domain(tmp)
            with self.assertRaises(CliError) as ctx:
                run_new("x", description="d", keywords=["a"], body="  \n", cwd=d)
            self.assertIn("--body-file", str(ctx.exception))


class TestDomainHints(unittest.TestCase):
    """Did-you-mean hints on domain/cwd errors (Tier 3)."""

    def _repo(self, base: Path, name: str, domains: tuple[str, ...]) -> Path:
        root = base / name
        (root / ".git").mkdir(parents=True)
        for dom in domains:
            d = root / dom
            d.mkdir(parents=True)
            (d / "INDEX.md").write_text("# dom\n")
        return root

    def test_domain_nonexistent_lists_valid_domains(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._repo(Path(tmp), "kb", ("design", "research"))
            with self.assertRaises(CliError) as ctx:
                run_new(
                    "x",
                    description="d",
                    keywords=["a"],
                    domain="ghost",
                    body="x",
                    cwd=root,
                )
            msg = str(ctx.exception)
            self.assertIn("valid domains", msg)
            self.assertIn("design", msg)
            self.assertIn("research", msg)

    def test_domain_doubling_suggests_bare_domain(self):
        # agent prepends the repo name to a repo-relative --domain
        with tempfile.TemporaryDirectory() as tmp:
            root = self._repo(Path(tmp), "example-kb", ("docs",))
            with self.assertRaises(CliError) as ctx:
                run_new(
                    "x",
                    description="d",
                    keywords=["a"],
                    domain="example-kb/docs",
                    body="x",
                    cwd=root,
                )
            self.assertIn("did you mean 'docs'", str(ctx.exception))

    def test_hint_lists_physical_path_not_c2_chain(self):
        # C2-skip topology: b/ has no INDEX, so the C2 domain is `a/c` but the
        # physical --domain value is `a/b/c`. The hint MUST list the physical
        # path (what --domain consumes), never the C2 form that would re-fail.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "kb"
            (root / ".git").mkdir(parents=True)
            (root / "a").mkdir()
            (root / "a" / "INDEX.md").write_text("# a\n")
            (root / "a" / "b" / "c").mkdir(parents=True)
            (root / "a" / "b" / "c" / "INDEX.md").write_text("# c\n")
            with self.assertRaises(CliError) as ctx:
                run_new(
                    "x",
                    description="d",
                    keywords=["a"],
                    domain="a/c",
                    body="x",
                    cwd=root,
                )
            msg = str(ctx.exception)
            self.assertIn("a/b/c", msg)  # physical path, the one that works
            self.assertNotIn("a/c;", msg)  # not the dead-end C2 form
            # and the suggested physical path actually lands a note
            res = run_new(
                "y",
                description="d",
                keywords=["a"],
                domain="a/b/c",
                body="x",
                cwd=root,
            )
            self.assertEqual(Path(res["path"]).parent.name, "c")

    def test_derive_no_domain_lists_valid_domains(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._repo(Path(tmp), "kb", ("design",))
            # cwd = repo root (no INDEX.md at root) → derive fails
            with self.assertRaises(CliError) as ctx:
                run_new("x", description="d", keywords=["a"], body="x", cwd=root)
            msg = str(ctx.exception)
            self.assertIn("valid domains", msg)
            self.assertIn("design", msg)

    def test_not_git_repo_hint(self):
        with tempfile.TemporaryDirectory() as tmp:
            bare = Path(tmp) / "loose"
            bare.mkdir()
            with self.assertRaises(CliError) as ctx:
                run_new("x", description="d", keywords=["a"], body="x", cwd=bare)
            self.assertIn("rhizome domains", str(ctx.exception))


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


class TestDomainsCompact(unittest.TestCase):
    """`domains --compact` (core inline / vertical collapsed) and `domains <repo>` drill.

    Locks the surface-aware rendering the session-start KB hook depends on, so a
    silent feature drop can't recur."""

    def _build(self, base: Path) -> Path:
        for repo, dom in (("core-kb", "alpha"), ("vert-kb", "beta")):
            d = base / repo / dom
            d.mkdir(parents=True)
            (d / "INDEX.md").write_text(f"# {dom}\n")
        reg = base / "kb-sources.toml"
        reg.write_text(
            f'workspace_root = "{base}"\n'
            '[[source]]\nname = "core-kb"\nsurface = "core"\n'
            '[[source]]\nname = "vert-kb"\nsurface = "vertical"\n'
        )
        return reg

    def _run(self, argv, reg, base):
        env = {"KB_SOURCES": str(reg), "KB_WORKSPACE_ROOT": str(base)}
        out = io.StringIO()
        with (
            mock.patch.dict(os.environ, env),
            contextlib.redirect_stdout(out),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            rc = main(argv)
        return rc, out.getvalue()

    def test_compact_core_inline_vertical_collapsed(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            reg = self._build(base)
            rc, out = self._run(["domains", "--compact"], reg, base)
            self.assertEqual(rc, 0)
            self.assertIn("alpha", out)  # core domain shown inline
            self.assertIn("vertical(", out)  # vertical collapsed to a name line
            self.assertIn("vert-kb", out)
            self.assertNotIn("beta", out)  # vertical domain NOT expanded

    def test_drill_one_repo_shows_its_domains(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            reg = self._build(base)
            rc, out = self._run(["domains", "vert-kb"], reg, base)
            self.assertEqual(rc, 0)
            self.assertIn("beta", out)
            self.assertNotIn("alpha", out)

    def test_unknown_repo_returns_2(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            reg = self._build(base)
            rc, _ = self._run(["domains", "nope"], reg, base)
            self.assertEqual(rc, 2)


if __name__ == "__main__":
    unittest.main()
