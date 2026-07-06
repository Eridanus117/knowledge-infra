"""capture: append a fleeting thought to the inbox (run_capture + CLI)."""

from __future__ import annotations

import io
import json
import os
import tempfile
import unittest
from datetime import datetime
from pathlib import Path
from unittest import mock

from rhizome import capture
from rhizome.cli import main

FIXED = datetime(2026, 7, 2, 9, 30, 15).astimezone()


class TestRunCapture(unittest.TestCase):
    def test_appends_timestamped_line_and_creates_parents(self):
        with tempfile.TemporaryDirectory() as tmp:
            inbox = Path(tmp) / "nested" / "inbox.md"
            res = capture.run_capture("buy oat milk #groceries", inbox=inbox, now=FIXED)
            self.assertEqual(res["path"], str(inbox))
            self.assertEqual(res["text"], "buy oat milk #groceries")
            self.assertTrue(inbox.exists())
            self.assertEqual(
                inbox.read_text(encoding="utf-8"),
                f"- {FIXED.isoformat(timespec='seconds')} buy oat milk #groceries\n",
            )

    def test_appends_in_order_preserving_existing(self):
        with tempfile.TemporaryDirectory() as tmp:
            inbox = Path(tmp) / "inbox.md"
            capture.run_capture("first", inbox=inbox, now=FIXED)
            capture.run_capture("second", inbox=inbox, now=FIXED)
            lines = inbox.read_text(encoding="utf-8").splitlines()
            self.assertEqual(len(lines), 2)
            self.assertTrue(lines[0].endswith("first"))
            self.assertTrue(lines[1].endswith("second"))

    def test_collapses_whitespace_to_one_line(self):
        with tempfile.TemporaryDirectory() as tmp:
            inbox = Path(tmp) / "inbox.md"
            res = capture.run_capture("a\n  b\t c  ", inbox=inbox, now=FIXED)
            self.assertEqual(res["text"], "a b c")
            self.assertEqual(len(inbox.read_text(encoding="utf-8").splitlines()), 1)

    def test_empty_thought_raises(self):
        with tempfile.TemporaryDirectory() as tmp:
            inbox = Path(tmp) / "inbox.md"
            with self.assertRaises(capture.CaptureError):
                capture.run_capture("   \n  ", inbox=inbox)
            self.assertFalse(inbox.exists())


class TestInboxPath(unittest.TestCase):
    def test_env_override(self):
        with mock.patch.dict(os.environ, {"RHIZOME_INBOX": "/tmp/x/inbox.md"}):
            self.assertEqual(capture.inbox_path(), Path("/tmp/x/inbox.md"))

    def test_default_under_config(self):
        # conftest strips RHIZOME_INBOX, so this exercises the default branch.
        self.assertEqual(
            capture.inbox_path(),
            Path("~/.config/rhizome/inbox.md").expanduser(),
        )


class TestCaptureCli(unittest.TestCase):
    def test_cli_positional_words(self):
        with tempfile.TemporaryDirectory() as tmp:
            inbox = Path(tmp) / "inbox.md"
            with mock.patch.dict(os.environ, {"RHIZOME_INBOX": str(inbox)}):
                rc = main(["capture", "ship", "the", "capture", "verb"])
            self.assertEqual(rc, 0)
            self.assertIn("ship the capture verb", inbox.read_text(encoding="utf-8"))

    def test_cli_stdin(self):
        with tempfile.TemporaryDirectory() as tmp:
            inbox = Path(tmp) / "inbox.md"
            with (
                mock.patch.dict(os.environ, {"RHIZOME_INBOX": str(inbox)}),
                mock.patch("sys.stdin", io.StringIO("piped thought\n")),
            ):
                rc = main(["capture"])
            self.assertEqual(rc, 0)
            self.assertIn("piped thought", inbox.read_text(encoding="utf-8"))

    def test_cli_json(self):
        with tempfile.TemporaryDirectory() as tmp:
            inbox = Path(tmp) / "inbox.md"
            buf = io.StringIO()
            with (
                mock.patch.dict(os.environ, {"RHIZOME_INBOX": str(inbox)}),
                mock.patch("sys.stdout", buf),
            ):
                rc = main(["capture", "--json", "hi"])
            self.assertEqual(rc, 0)
            payload = json.loads(buf.getvalue())
            self.assertEqual(payload["text"], "hi")
            self.assertEqual(payload["path"], str(inbox))

    def test_cli_tty_no_input_is_usage_error(self):
        fake_stdin = mock.Mock()
        fake_stdin.isatty.return_value = True
        with mock.patch("sys.stdin", fake_stdin):
            rc = main(["capture"])
        self.assertEqual(rc, 2)
        fake_stdin.read.assert_not_called()


if __name__ == "__main__":
    unittest.main()
