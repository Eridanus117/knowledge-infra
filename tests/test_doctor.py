"""rhizome doctor --sources: fleet pipeline-integrity check.

Mirrors test_adopt.py: fake which(), tmp registry + repo fixtures, no shelling
out. Covers the three pipeline-integrity items (gate present, gate resolvable,
INDEX present), the all-green path, each failure mode (with its diagnostic
reason), fleet-wide non-zero exit, JSON shape, and the CLI wiring.
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path

from rhizome import doctor
from rhizome.cli import main

_LEFTHOOK_OK = (
    "pre-commit:\n  commands:\n    kb-check:\n      run: rhizome check {staged_files}\n"
)
_INDEX = (
    '---\ndescription: "seed domain"\nkeywords: [seed]\nkind: index\n---\n\n# seed\n'
)


def _which(name):  # both lefthook and rhizome "found"
    return f"/fake/bin/{name}"


def _which_no_rhizome(name):
    return None if name == "rhizome" else f"/fake/bin/{name}"


class TestDoctor(unittest.TestCase):
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
        reg.write_text(f'workspace_root = "{ws}"\n', encoding="utf-8")
        return ws, reg

    def _source(self, reg: Path, name: str, repo: Path | None = None) -> None:
        text = reg.read_text(encoding="utf-8").rstrip("\n") + "\n\n"
        text += f'[[source]]\nname = "{name}"\n'
        if repo is not None:
            text += f'path = "{repo}"\n'
        reg.write_text(text, encoding="utf-8")

    def _repo(
        self,
        parent: Path,
        name: str,
        *,
        gate: str | None = _LEFTHOOK_OK,
        precommit: str | None = None,
        domain: str | None = "docs",
    ) -> Path:
        """Build a repo with optional gate file(s) and a docs/INDEX.md domain."""
        repo = parent / name
        repo.mkdir(parents=True)
        if gate is not None:
            (repo / "lefthook.yml").write_text(gate, encoding="utf-8")
        if precommit is not None:
            (repo / ".pre-commit-config.yaml").write_text(precommit, encoding="utf-8")
        if domain is not None:
            d = repo / domain
            d.mkdir(parents=True)
            (d / "INDEX.md").write_text(_INDEX, encoding="utf-8")
        return repo

    # ---- the happy path -----------------------------------------------------

    def test_all_green(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(ws, "alpha")
            self._repo(ws, "beta")
            self._source(reg, "alpha")
            self._source(reg, "beta")
            report = doctor.run_doctor(registry=reg, which=_which)
            self.assertTrue(report["ok"])
            self.assertEqual(len(report["sources"]), 2)
            for r in report["sources"]:
                self.assertTrue(r["ok"])
                self.assertEqual({c["status"] for c in r["checks"]}, {doctor.PASS})

    def test_precommit_framework_gate_counts(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(
                ws,
                "alpha",
                gate=None,
                precommit="repos:\n  - repo: local\n    hooks:\n      - entry: rhizome check\n",
            )
            self._source(reg, "alpha")
            report = doctor.run_doctor(registry=reg, which=_which)
            self.assertTrue(report["ok"])
            gate = next(
                c
                for c in report["sources"][0]["checks"]
                if c["check"] == "gate-present"
            )
            self.assertEqual(gate["status"], doctor.PASS)
            self.assertIn(".pre-commit-config.yaml", gate["detail"])

    def test_old_kb_check_name_tolerated(self):
        # adopt tolerates the legacy `kb check` name in the gate file; doctor must
        # agree (otherwise it would flag every not-yet-migrated repo as gateless).
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(
                ws,
                "alpha",
                gate="pre-commit:\n  commands:\n    kb-check:\n      run: kb check\n",
            )
            self._source(reg, "alpha")
            report = doctor.run_doctor(registry=reg, which=_which)
            gate = next(
                c
                for c in report["sources"][0]["checks"]
                if c["check"] == "gate-present"
            )
            self.assertEqual(gate["status"], doctor.PASS)

    # ---- failure modes (each must name a diagnostic reason) -----------------

    def test_gate_missing_fails_with_reason(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(ws, "alpha", gate=None)  # no lefthook.yml, no pre-commit
            self._source(reg, "alpha")
            report = doctor.run_doctor(registry=reg, which=_which)
            self.assertFalse(report["ok"])
            gate = next(
                c
                for c in report["sources"][0]["checks"]
                if c["check"] == "gate-present"
            )
            self.assertEqual(gate["status"], doctor.FAIL)
            self.assertIn("no KB commit gate", gate["detail"])
            self.assertIn("lefthook.yml (absent)", gate["detail"])

    def test_commented_gate_is_not_enough(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(
                ws, "alpha", gate="# rhizome check lives here someday\npre-commit:\n"
            )
            self._source(reg, "alpha")
            report = doctor.run_doctor(registry=reg, which=_which)
            gate = next(
                c
                for c in report["sources"][0]["checks"]
                if c["check"] == "gate-present"
            )
            self.assertEqual(gate["status"], doctor.FAIL)
            self.assertIn("no `rhizome check` command", gate["detail"])

    def test_gate_unresolvable_fails_fleet_wide(self):
        # which(rhizome)=None → the gate dies exit 127 in every repo's fresh shell.
        # This is the exact kb→rhizome rename bug this check was written for.
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(ws, "alpha")
            self._repo(ws, "beta")
            self._source(reg, "alpha")
            self._source(reg, "beta")
            report = doctor.run_doctor(registry=reg, which=_which_no_rhizome)
            self.assertFalse(report["ok"])
            self.assertFalse(report["gate_resolvable"]["ok"])
            self.assertIn("which(rhizome)=None", report["gate_resolvable"]["detail"])
            for r in report["sources"]:  # every repo fails the resolvable check
                res = next(c for c in r["checks"] if c["check"] == "gate-resolvable")
                self.assertEqual(res["status"], doctor.FAIL)
                self.assertFalse(r["ok"])

    def test_no_index_fails_with_reason(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(ws, "alpha", domain=None)  # gate ok, but no INDEX.md
            self._source(reg, "alpha")
            report = doctor.run_doctor(registry=reg, which=_which)
            self.assertFalse(report["ok"])
            idx = next(
                c
                for c in report["sources"][0]["checks"]
                if c["check"] == "index-present"
            )
            self.assertEqual(idx["status"], doctor.FAIL)
            self.assertIn("no INDEX.md domain found", idx["detail"])

    def test_missing_repo_is_fail(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._source(reg, "ghost", repo=ws / "does-not-exist")
            report = doctor.run_doctor(registry=reg, which=_which)
            self.assertFalse(report["ok"])
            r = report["sources"][0]
            self.assertFalse(r["ok"])
            self.assertEqual(r["checks"][0]["check"], "repo-exists")
            self.assertIn("does not exist", r["checks"][0]["detail"])

    def test_one_bad_source_fails_whole_fleet(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(ws, "good")
            self._repo(ws, "bad", gate=None)
            self._source(reg, "good")
            self._source(reg, "bad")
            report = doctor.run_doctor(registry=reg, which=_which)
            self.assertFalse(report["ok"])  # one fail → fleet fail
            ok = {r["name"]: r["ok"] for r in report["sources"]}
            self.assertEqual(ok, {"good": True, "bad": False})

    # ---- CLI wiring ---------------------------------------------------------

    def test_cli_exit_nonzero_on_fail(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(ws, "bad", gate=None)
            self._source(reg, "bad")
            os.environ["KB_SOURCES"] = str(reg)
            err = io.StringIO()
            with (
                contextlib.redirect_stdout(io.StringIO()),
                contextlib.redirect_stderr(err),
            ):
                rc = main(["doctor", "--sources"])
            self.assertEqual(rc, 1)
            self.assertIn("FAIL", err.getvalue())

    def test_cli_exit_zero_all_green(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(ws, "alpha")
            self._source(reg, "alpha")
            os.environ["KB_SOURCES"] = str(reg)
            with (
                contextlib.redirect_stdout(io.StringIO()),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                rc = main(["doctor", "--sources"])
            self.assertEqual(rc, 0)

    def test_cli_json_shape(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws, reg = self._ws(tmp)
            self._repo(ws, "alpha")
            self._source(reg, "alpha")
            os.environ["KB_SOURCES"] = str(reg)
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                rc = main(["doctor", "--sources", "--json"])
            self.assertEqual(rc, 0)
            report = json.loads(out.getvalue())
            self.assertTrue(report["ok"])
            self.assertEqual([r["name"] for r in report["sources"]], ["alpha"])
            self.assertEqual(
                {c["check"] for c in report["sources"][0]["checks"]},
                {"gate-present", "gate-resolvable", "index-present"},
            )
            self.assertIn("gate_resolvable", report)

    def test_cli_requires_sources_flag(self):
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = main(["doctor"])
        self.assertEqual(rc, 2)
        self.assertIn("--sources", err.getvalue())

    def test_cli_missing_registry_loud(self):
        with tempfile.TemporaryDirectory() as tmp:
            os.environ["KB_SOURCES"] = str(Path(tmp) / "nope.toml")
            err = io.StringIO()
            with contextlib.redirect_stderr(err):
                rc = main(["doctor", "--sources"])
            self.assertEqual(rc, 2)
            self.assertIn("missing file", err.getvalue())


from rhizome import adopt  # noqa: E402


class TestDoctorSelf(unittest.TestCase):
    """doctor --self: tool-chain gate template/probe self-consistency (B-class).

    The core regression is test_template_rename_break_fails: monkeypatch the
    LEFTHOOK_YML template back to the dead `kb check` name (the kb→rhizome
    rename bug) and assert --self FAILs — proving it catches a rename at the
    template, which no source-repo scan can see.
    """

    def setUp(self):
        self._orig_template = adopt.LEFTHOOK_YML
        self._orig_gate_cmd = adopt._GATE_COMMAND

    def tearDown(self):
        adopt.LEFTHOOK_YML = self._orig_template
        adopt._GATE_COMMAND = self._orig_gate_cmd

    # ---- happy path (current, fixed state) ----------------------------------

    def test_self_passes_in_fixed_state(self):
        report = doctor.run_self_check(which=_which)
        self.assertTrue(report["ok"])
        self.assertEqual({c["status"] for c in report["checks"]}, {doctor.PASS})
        names = {c["check"] for c in report["checks"]}
        self.assertEqual(
            names,
            {
                "template-parses",
                "template-resolvable",
                "template-probe-agree",
                "no-legacy-names",
            },
        )

    def test_self_detail_names_parsed_command(self):
        report = doctor.run_self_check(which=_which)
        parses = next(c for c in report["checks"] if c["check"] == "template-parses")
        self.assertIn("rhizome", parses["detail"])

    # ---- the core regression: a renamed/broken template must FAIL -----------

    def test_template_rename_break_fails(self):
        # Revert the template to the dead `kb check` name — the exact rename bug
        # (template + probe were both `kb`, "consistently wrong"). The retired
        # `kb` command is not on PATH, so the freshly-adopted hook would exit 127.
        adopt.LEFTHOOK_YML = self._orig_template.replace("rhizome check", "kb check")

        def which_no_kb(name):  # `kb` retired off PATH, everything else resolves
            return None if name == "kb" else f"/fake/bin/{name}"

        report = doctor.run_self_check(which=which_no_kb)
        self.assertFalse(report["ok"])
        res = next(c for c in report["checks"] if c["check"] == "template-resolvable")
        self.assertEqual(res["status"], doctor.FAIL)
        self.assertIn("kb", res["detail"])
        self.assertIn("127", res["detail"])
        # The same broken template also trips the probe-agreement + legacy-name
        # checks, so it FAILs even if the dead command happened to stay on PATH.
        agree = next(
            c for c in report["checks"] if c["check"] == "template-probe-agree"
        )
        legacy = next(c for c in report["checks"] if c["check"] == "no-legacy-names")
        self.assertEqual(agree["status"], doctor.FAIL)
        self.assertEqual(legacy["status"], doctor.FAIL)

    def test_template_unresolvable_command_fails(self):
        # which(rhizome)=None: even with the right name, an unresolvable command
        # is a fail — this is the literal rename-break judge (PATH on the box).
        report = doctor.run_self_check(which=_which_no_rhizome)
        self.assertFalse(report["ok"])
        res = next(c for c in report["checks"] if c["check"] == "template-resolvable")
        self.assertEqual(res["status"], doctor.FAIL)
        self.assertIn("rhizome", res["detail"])

    def test_template_probe_disagree_fails(self):
        # Half-done rename: template updated to a new name, probe constant not.
        # Even though the new command resolves, template ⇔ probe must agree.
        adopt.LEFTHOOK_YML = self._orig_template.replace(
            "rhizome check", "rhizome2 check"
        )
        report = doctor.run_self_check(which=_which)  # everything "resolves"
        self.assertFalse(report["ok"])
        agree = next(
            c for c in report["checks"] if c["check"] == "template-probe-agree"
        )
        self.assertEqual(agree["status"], doctor.FAIL)
        self.assertIn("disagree", agree["detail"])

    def test_legacy_name_in_template_fails(self):
        adopt.LEFTHOOK_YML = self._orig_template.replace("rhizome check", "kb check")
        report = doctor.run_self_check(
            which=_which
        )  # kb "resolves" so isolate this check
        legacy = next(c for c in report["checks"] if c["check"] == "no-legacy-names")
        self.assertEqual(legacy["status"], doctor.FAIL)
        self.assertIn("kb", legacy["detail"])

    def test_commented_run_line_is_not_a_gate(self):
        # A commented `run:` line ships no command — the parser must ignore it.
        cmds = doctor._template_gate_commands(
            "pre-commit:\n  commands:\n    kb-check:\n      # run: kb check\n      run: rhizome check\n"
        )
        self.assertEqual(cmds, ["rhizome"])

    # ---- CLI wiring ---------------------------------------------------------

    def test_cli_self_exit_zero_in_fixed_state(self):
        with (
            contextlib.redirect_stdout(io.StringIO()),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            rc = main(["doctor", "--self"])
        self.assertEqual(rc, 0)

    def test_cli_self_exit_nonzero_on_broken_template(self):
        # Revert to the dead `kb check` name: even if `rhizome` happens to be on
        # the test process PATH, the bug still surfaces via template-probe-agree
        # and no-legacy-names, so --self FAILs regardless of PATH.
        adopt.LEFTHOOK_YML = self._orig_template.replace("rhizome check", "kb check")
        err = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
            rc = main(["doctor", "--self"])
        self.assertEqual(rc, 1)
        self.assertIn("FAIL", err.getvalue())

    def test_cli_self_json_shape(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rc = main(["doctor", "--self", "--json"])
        self.assertEqual(rc, 0)
        report = json.loads(out.getvalue())
        self.assertTrue(report["ok"])
        self.assertEqual(
            {c["check"] for c in report["checks"]},
            {
                "template-parses",
                "template-resolvable",
                "template-probe-agree",
                "no-legacy-names",
            },
        )

    def test_cli_all_runs_both_modes(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws = Path(tmp) / "ws"
            ws.mkdir()
            reg = Path(tmp) / "kb-sources.toml"
            reg.write_text(f'workspace_root = "{ws}"\n', encoding="utf-8")
            repo = ws / "alpha"
            repo.mkdir()
            (repo / "lefthook.yml").write_text(_LEFTHOOK_OK, encoding="utf-8")
            d = repo / "docs"
            d.mkdir()
            (d / "INDEX.md").write_text(_INDEX, encoding="utf-8")
            reg.write_text(
                reg.read_text() + '\n[[source]]\nname = "alpha"\n', encoding="utf-8"
            )
            os.environ["KB_SOURCES"] = str(reg)
            try:
                out = io.StringIO()
                with (
                    contextlib.redirect_stdout(out),
                    contextlib.redirect_stderr(io.StringIO()),
                ):
                    rc = main(["doctor", "--all", "--json"])
                self.assertEqual(rc, 0)
                payload = json.loads(out.getvalue())
                self.assertIn("sources", payload)
                self.assertIn("self", payload)
                self.assertTrue(payload["ok"])
            finally:
                os.environ.pop("KB_SOURCES", None)

    def test_cli_requires_a_mode(self):
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = main(["doctor"])
        self.assertEqual(rc, 2)
        self.assertIn("--self", err.getvalue())
        self.assertIn("--sources", err.getvalue())


if __name__ == "__main__":
    unittest.main()
