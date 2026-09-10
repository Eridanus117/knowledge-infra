"""link-checker: links 断链 ERROR / code 漂移 WARN / frozen 豁免。"""

from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path

from rhizome import check, links


def _mkrepo(root: Path) -> Path:
    (root / ".git").mkdir()
    d = root / "docs"
    d.mkdir()
    (d / "INDEX.md").write_text("# idx\n", encoding="utf-8")
    return d


def _note(
    link_slugs: list[str] | None = None,
    code: list[str] | None = None,
    frozen: bool = False,
) -> str:
    lines = ['description: "d"', "keywords: [a]", "kind: note"]
    if link_slugs is not None:
        lines.append("links: [" + ", ".join(link_slugs) + "]")
    if code is not None:
        lines.append("code: [" + ", ".join(f'"{c}"' for c in code) + "]")
    if frozen:
        lines.append("status: frozen")
    return "---\n" + "\n".join(lines) + "\n---\n\nbody\n"


class TestLinkFindings(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.domain = _mkrepo(self.root)
        # 隔离 registry/workspace: 指向空 tmp → 无跨仓索引、code 解析根受控。
        self._old_ws = os.environ.get("KB_WORKSPACE_ROOT")
        os.environ["KB_WORKSPACE_ROOT"] = str(self.root / "no-such-ws")
        links._slug_cache.clear()
        links._foreign_cache.clear()

    def tearDown(self):
        if self._old_ws is None:
            os.environ.pop("KB_WORKSPACE_ROOT", None)
        else:
            os.environ["KB_WORKSPACE_ROOT"] = self._old_ws
        self._tmp.cleanup()

    def _write(self, name: str, text: str) -> Path:
        p = self.domain / name
        p.write_text(text, encoding="utf-8")
        return p

    def _findings(self, name: str, text: str):
        p = self._write(name, text)
        return links.link_findings(p, text)

    def test_resolvable_link_clean(self):
        self._write("target.md", _note())
        self.assertEqual(self._findings("a.md", _note(["target"])), [])

    def test_broken_link_errors(self):
        f = self._findings("a.md", _note(["no-such-note"]))
        self.assertEqual([x.severity for x in f], [check.ERROR])
        self.assertEqual(f[0].field, "links")

    def test_identity_form_link_errors(self):
        f = self._findings("a.md", _note(["repo:domain:slug"]))
        self.assertEqual([x.severity for x in f], [check.ERROR])
        self.assertIn("identity", f[0].message)

    def test_frozen_doc_exempt(self):
        text = _note(["no-such-note"], code=["gone/path.py"], frozen=True)
        self.assertEqual(self._findings("a.md", text), [])

    def test_code_ellipsis_warns(self):
        f = self._findings("a.md", _note(code=["repo/.../Foo.java"]))
        self.assertEqual([x.severity for x in f], [check.WARN])
        self.assertEqual(f[0].field, "code")

    def test_code_resolvable_in_repo_clean(self):
        (self.root / "src").mkdir()
        (self.root / "src" / "x.py").write_text("", encoding="utf-8")
        self.assertEqual(self._findings("a.md", _note(code=["src/x.py"])), [])

    def test_code_unresolvable_path_warns(self):
        f = self._findings("a.md", _note(code=["gone/path.py"]))
        self.assertEqual([x.severity for x in f], [check.WARN])

    def test_code_non_path_hint_silent(self):
        self.assertEqual(
            self._findings("a.md", _note(code=["mydb.s_report_config"])), []
        )

    def test_code_root_unresolvable_without_env(self):
        # A code entry living under a workspace aggregate dir does not resolve
        # while RHIZOME_CODE_ROOTS is unset (public default = no extra roots).
        ws = self.root / "ws"
        (ws / "aggdir" / "sub").mkdir(parents=True)
        (ws / "aggdir" / "sub" / "x.py").write_text("", encoding="utf-8")
        os.environ["KB_WORKSPACE_ROOT"] = str(ws)
        os.environ.pop("RHIZOME_CODE_ROOTS", None)
        try:
            f = self._findings("a.md", _note(code=["sub/x.py"]))
        finally:
            os.environ["KB_WORKSPACE_ROOT"] = str(self.root / "no-such-ws")
        self.assertEqual([x.severity for x in f], [check.WARN])

    def test_code_root_resolvable_via_env(self):
        # Injecting the aggregate dir name via RHIZOME_CODE_ROOTS makes the same
        # entry resolve — env extends the code-root search set additively.
        ws = self.root / "ws"
        (ws / "aggdir" / "sub").mkdir(parents=True)
        (ws / "aggdir" / "sub" / "x.py").write_text("", encoding="utf-8")
        os.environ["KB_WORKSPACE_ROOT"] = str(ws)
        os.environ["RHIZOME_CODE_ROOTS"] = "aggdir"
        try:
            f = self._findings("a.md", _note(code=["sub/x.py"]))
        finally:
            os.environ["KB_WORKSPACE_ROOT"] = str(self.root / "no-such-ws")
            os.environ.pop("RHIZOME_CODE_ROOTS", None)
        self.assertEqual(f, [])

    def test_check_path_surfaces_link_findings(self):
        p = self._write("a.md", _note(["no-such-note"]))
        fields = {f.field for f in check.check_path(p) if f.severity == check.ERROR}
        self.assertIn("links", fields)

    def test_outside_domain_skipped(self):
        p = self.root / "scratch.md"
        text = _note(["no-such-note"])
        p.write_text(text, encoding="utf-8")
        self.assertEqual(links.link_findings(p, text), [])


if __name__ == "__main__":
    unittest.main()
