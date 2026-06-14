"""kb adopt: idempotent convergence over registry + INDEX skeleton + lefthook ."""

from __future__ import annotations

import contextlib
import io
import json
import os
import subprocess
import tempfile
import tomllib
import unittest
from pathlib import Path
from types import SimpleNamespace

from kb.adopt import AdoptError, AdoptUsageError, run_adopt
from kb.cli import main


class _Runner:
    """Fake subprocess.run: answers `git config core.hooksPath` and mimics
    `lefthook install` (writes the hook file) without ever shelling out."""

    def __init__(self, hooks_path: str | None = None, install_rc: int = 0):
        self.calls: list[tuple[list[str], Path]] = []
        self.hooks_path = hooks_path
        self.install_rc = install_rc

    def __call__(self, cmd, cwd=None, capture_output=True, text=True):
        self.calls.append((list(cmd), Path(cwd)))
        if list(cmd) == ["git", "config", "core.hooksPath"]:
            if self.hooks_path:
                return SimpleNamespace(
                    returncode=0, stdout=self.hooks_path + "\n", stderr=""
                )
            return SimpleNamespace(returncode=1, stdout="", stderr="")
        if list(cmd) == ["lefthook", "install"]:
            if self.install_rc == 0:
                hooks = Path(cwd) / ".git" / "hooks"
                hooks.mkdir(parents=True, exist_ok=True)
                (hooks / "pre-commit").write_text("# managed by lefthook\n")
            return SimpleNamespace(returncode=self.install_rc, stdout="", stderr="boom")
        return SimpleNamespace(returncode=0, stdout="", stderr="")

    @property
    def install_calls(self):
        return [c for c in self.calls if c[0] == ["lefthook", "install"]]


def _which(name):  # both lefthook and kb "found"
    return f"/fake/bin/{name}"


class TestAdopt(unittest.TestCase):
    def setUp(self):
        self._env = {
            k: os.environ.pop(k, None) for k in ("KB_SOURCES", "KB_WORKSPACE_ROOT")
        }

    def tearDown(self):
        for k, v in self._env.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v

    # ---- fixtures -----------------------------------------------------------

    def _ws(self, tmp: str) -> tuple[Path, Path]:
        ws = Path(tmp) / "ws"
        ws.mkdir()
        reg = Path(tmp) / "kb-sources.toml"
        reg.write_text(
            f'workspace_root = "{ws}"\n\n[[source]]\nname = "seed"\n',
            encoding="utf-8",
        )
        return ws, reg

    def _repo(
        self, parent: Path, name: str = "proj", domain: str | None = None
    ) -> Path:
        repo = parent / name
        repo.mkdir(parents=True)
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        if domain:
            d = repo / domain
            d.mkdir(parents=True)
            (d / "INDEX.md").write_text(
                '---\ndescription: "seed domain"\nkeywords: [seed]\nkind: index\n---\n\n# seed\n',
                encoding="utf-8",
            )
        return repo

    def _adopt(self, arg, reg, runner=None, **kw):
        runner = runner or _Runner()
        return run_adopt(
            arg, registry=reg, runner=runner, which=_which, cwd=Path("/"), **kw
        ), runner

    # ---- the happy path -----------------------------------------------------

    def test_fresh_adopt_three_changed(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            repo = self._repo(ws)
            res, runner = self._adopt(
                "proj", reg, description="proj docs domain", keywords=["proj", "docs"]
            )
            self.assertEqual(
                {s["step"]: s["status"] for s in res["steps"]},
                {"registry": "changed", "index": "changed", "lefthook": "changed"},
            )
            text = reg.read_text()
            self.assertIn('name = "proj"', text)
            self.assertNotIn(
                "path =", text.split('name = "proj"')[1]
            )  # under workspace_root → bare row
            data = tomllib.loads(text)  # still parses, seed entry intact
            self.assertEqual([e["name"] for e in data["source"]], ["seed", "proj"])
            index = repo / "docs" / "INDEX.md"
            self.assertTrue(index.is_file())
            self.assertIn('description: "proj docs domain"', index.read_text())
            self.assertIn("kind: index", index.read_text())
            self.assertIn(
                "rhizome check {staged_files}", (repo / "lefthook.yml").read_text()
            )
            self.assertIn(
                "--duplicate-domains --staged-frozen",
                (repo / "lefthook.yml").read_text(),
            )
            self.assertEqual(
                runner.install_calls, [(["lefthook", "install"], repo.resolve())]
            )

    def test_rerun_idempotent(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(ws)
            self._adopt("proj", reg, description="d", keywords=["k"])
            before = reg.read_text()
            res, runner = self._adopt("proj", reg)
            self.assertEqual({s["status"] for s in res["steps"]}, {"ok"})
            self.assertEqual(reg.read_text(), before)
            self.assertEqual(runner.install_calls, [])

    # ---- registry append edge cases ------------------------------------------

    def test_append_handles_missing_trailing_newline(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(ws, domain="docs")
            reg.write_text(reg.read_text().rstrip("\n"), encoding="utf-8")
            res, _ = self._adopt("proj", reg)
            data = tomllib.loads(reg.read_text())
            self.assertIn("proj", [e["name"] for e in data["source"]])

    def test_repo_outside_workspace_root_gets_path_line(self):
        with tempfile.TemporaryDirectory() as tmp:
            _, reg = self._ws(tmp)
            repo = self._repo(Path(tmp) / "elsewhere", "proj2", domain="docs")
            self._adopt(str(repo), reg)
            data = tomllib.loads(reg.read_text())
            entry = next(e for e in data["source"] if e["name"] == "proj2")
            self.assertEqual(Path(entry["path"]).expanduser().resolve(), repo.resolve())

    def test_symlinked_path_entry_is_recognized(self):
        # macOS /var → /private/var: an unresolved seed path must still match.
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            repo = self._repo(ws, domain="docs")
            reg.write_text(
                f'workspace_root = "{Path(tmp) / "nowhere"}"\n\n[[source]]\nname = "proj"\npath = "{repo}"\n',
                encoding="utf-8",
            )
            before = reg.read_text()
            res, _ = self._adopt(str(repo.resolve()), reg)
            self.assertEqual(
                res["steps"][0],
                {
                    "step": "registry",
                    "status": "ok",
                    "detail": f"already listed in {reg.name}",
                },
            )
            self.assertEqual(reg.read_text(), before)

    def test_same_name_different_path_conflicts(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(ws, domain="docs")
            reg.write_text(
                f'workspace_root = "{ws}"\n\n[[source]]\nname = "proj"\npath = "/somewhere/else"\n',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(AdoptError, "different path"):
                self._adopt("proj", reg)

    def test_same_path_different_name_conflicts(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            repo = self._repo(ws, domain="docs")
            reg.write_text(
                f'workspace_root = "{ws}"\n\n[[source]]\nname = "alias"\npath = "{repo}"\n',
                encoding="utf-8",
            )
            with self.assertRaisesRegex(
                AdoptError, "already registered as source 'alias'"
            ):
                self._adopt("proj", reg)

    # ---- preconditions (plan-then-apply: nothing written on failure) ----------

    def test_worktree_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            wt = ws / "proj-wt"
            wt.mkdir()
            (wt / ".git").write_text("gitdir: /x/.git/worktrees/proj-wt\n")
            with self.assertRaisesRegex(AdoptUsageError, "main checkout"):
                self._adopt(str(wt), reg)

    def test_case_variant_index_blocks_before_any_write(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            repo = self._repo(ws)
            (repo / "docs").mkdir()
            (repo / "docs" / "index.md").write_text("# lowercase\n")
            before = reg.read_text()
            with self.assertRaisesRegex(AdoptError, "case variant"):
                self._adopt("proj", reg, description="d", keywords=["k"])
            self.assertEqual(reg.read_text(), before)  # registry untouched
            self.assertFalse((repo / "lefthook.yml").exists())

    def test_no_domain_without_dk_is_usage_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            repo = self._repo(ws)
            with self.assertRaises(AdoptUsageError):
                self._adopt("proj", reg)
            os.environ["KB_SOURCES"] = str(reg)
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                rc = main(["adopt", str(repo)])
            self.assertEqual(rc, 2)
            self.assertIn("no KB domain yet", stderr.getvalue())

    def test_missing_lefthook_binary_is_usage_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(ws, domain="docs")
            with self.assertRaisesRegex(AdoptUsageError, "lefthook not found"):
                run_adopt(
                    "proj",
                    registry=reg,
                    runner=_Runner(),
                    which=lambda n: None if n == "lefthook" else f"/fake/bin/{n}",
                    cwd=Path("/"),
                )

    def test_failed_recovery_rerun_converges(self):
        # Step ② failed (missing -d/-k); re-run WITH them must finish the job.
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            repo = self._repo(ws)
            with self.assertRaises(AdoptUsageError):
                self._adopt("proj", reg)
            res, _ = self._adopt("proj", reg, description="d", keywords=["k"])
            self.assertTrue((repo / "docs" / "INDEX.md").is_file())
            self.assertEqual({s["status"] for s in res["steps"]}, {"changed"})

    # ---- lefthook -------------------------------------------------------------

    def test_existing_lefthook_yml_never_rewritten(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            repo = self._repo(ws, domain="docs")
            custom = "pre-commit:\n  commands:\n    mine:\n      run: echo hi\n"
            (repo / "lefthook.yml").write_text(custom)
            res, runner = self._adopt("proj", reg)
            self.assertEqual((repo / "lefthook.yml").read_text(), custom)
            self.assertTrue(any("no `kb check`" in w for w in res["warnings"]))
            self.assertEqual(len(runner.install_calls), 1)  # still converges the hook

    def test_commented_kb_check_is_not_enough(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            repo = self._repo(ws, domain="docs")
            (repo / "lefthook.yml").write_text(
                "# kb check lives here someday\npre-commit:\n"
            )
            res, _ = self._adopt("proj", reg)
            self.assertTrue(any("no `kb check`" in w for w in res["warnings"]))

    def test_precommit_framework_gate_counts_as_converged(self):
        # Live case: the kb gate wired through .pre-commit-config.yaml.
        # Installing lefthook on top would rename the framework's hook to .old.
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            repo = self._repo(ws, domain="docs")
            (repo / ".pre-commit-config.yaml").write_text(
                "repos:\n  - repo: local\n    hooks:\n      - id: kb-check\n        entry: kb check\n"
            )
            res, runner = self._adopt("proj", reg)
            step = next(s for s in res["steps"] if s["step"] == "lefthook")
            self.assertEqual(step["status"], "ok")
            self.assertIn("pre-commit framework", step["detail"])
            self.assertFalse((repo / "lefthook.yml").exists())
            self.assertEqual(runner.install_calls, [])
            self.assertFalse(any("compete" in w for w in res["warnings"]))
            self.assertTrue(
                any("pre-commit install" in w for w in res["warnings"])
            )  # hook file absent

    def test_precommit_config_without_gate_still_gets_lefthook(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            repo = self._repo(ws, domain="docs")
            (repo / ".pre-commit-config.yaml").write_text(
                "repos:\n  - repo: local\n    hooks:\n      - id: ruff\n        entry: ruff check\n"
                "# kb check mentioned only in a comment\n"
            )
            res, runner = self._adopt("proj", reg)
            self.assertTrue((repo / "lefthook.yml").exists())
            self.assertEqual(len(runner.install_calls), 1)
            self.assertTrue(any("compete" in w for w in res["warnings"]))

    def test_occupied_hookspath_skips_install(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(ws, domain="docs")
            res, runner = self._adopt(
                "proj", reg, runner=_Runner(hooks_path=".husky/_")
            )
            step = next(s for s in res["steps"] if s["step"] == "lefthook")
            self.assertEqual(step["status"], "skipped")
            self.assertEqual(runner.install_calls, [])

    def test_install_failure_raises(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(ws, domain="docs")
            with self.assertRaisesRegex(AdoptError, "lefthook install failed"):
                self._adopt("proj", reg, runner=_Runner(install_rc=1))

    # ---- misc -----------------------------------------------------------------

    def test_dk_ignored_when_domains_exist(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(ws, domain="docs")
            res, _ = self._adopt("proj", reg, description="d", keywords=["k"])
            self.assertTrue(any("ignored" in w for w in res["warnings"]))
            self.assertEqual(
                next(s for s in res["steps"] if s["step"] == "index")["status"], "ok"
            )

    def test_stray_md_counted(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            repo = self._repo(ws, domain="docs")
            (repo / "README.md").write_text("# readme\n")
            res, _ = self._adopt("proj", reg)
            self.assertEqual(res["stray_md"], 1)

    def test_json_output_shape(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            repo = self._repo(ws, domain="docs")
            # Fully adopted already → no real lefthook run from main().
            (repo / "lefthook.yml").write_text(
                "pre-commit:\n  commands:\n    kb-check:\n      run: kb check {staged_files}\n"
            )
            hooks = repo / ".git" / "hooks"
            hooks.mkdir(parents=True, exist_ok=True)
            (hooks / "pre-commit").write_text("# managed by lefthook\n")
            reg.write_text(
                f'workspace_root = "{ws}"\n\n[[source]]\nname = "proj"\n',
                encoding="utf-8",
            )
            os.environ["KB_SOURCES"] = str(reg)
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                rc = main(["adopt", str(repo), "--json"])
            self.assertEqual(rc, 0)
            out = json.loads(stdout.getvalue())
            self.assertEqual(out["name"], "proj")
            self.assertEqual(
                {s["step"] for s in out["steps"]}, {"registry", "index", "lefthook"}
            )
            self.assertEqual({s["status"] for s in out["steps"]}, {"ok"})


if __name__ == "__main__":
    unittest.main()
