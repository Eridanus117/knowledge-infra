"""User-level config.toml parsing."""

from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from rhizome import check, config


class TestConfig(unittest.TestCase):
    def test_missing_default_config_is_empty(self):
        with mock.patch.dict(os.environ, {"HOME": "/no/such/home"}, clear=False):
            self.assertEqual(config.load_config(), {})

    def test_explicit_missing_config_errors(self):
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.dict(
                os.environ, {"RHIZOME_CONFIG": str(Path(tmp) / "missing.toml")}
            ):
                with self.assertRaises(config.ConfigError):
                    config.load_config()

    def test_mermaid_validator_dir_comes_from_config(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            validator = base / "validator"
            cfg = base / "config.toml"
            cfg.write_text(
                f'[mermaid]\nvalidator_dir = "{validator}"\n',
                encoding="utf-8",
            )
            with mock.patch.dict(os.environ, {"RHIZOME_CONFIG": str(cfg)}):
                self.assertEqual(check._mermaid_validator_dir(), validator)

    def test_mermaid_env_overrides_config(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            cfg = base / "config.toml"
            cfg.write_text(
                f'[mermaid]\nvalidator_dir = "{base / "from-config"}"\n',
                encoding="utf-8",
            )
            with mock.patch.dict(
                os.environ,
                {
                    "RHIZOME_CONFIG": str(cfg),
                    "RHIZOME_MERMAID_VALIDATOR_DIR": str(base / "from-env"),
                },
            ):
                self.assertEqual(check._mermaid_validator_dir(), base / "from-env")

    def test_mermaid_config_error_is_a_finding(self):
        with tempfile.TemporaryDirectory() as tmp:
            cfg = Path(tmp) / "config.toml"
            cfg.write_text("[mermaid]\nvalidator_dir = []\n", encoding="utf-8")
            text = "```mermaid\nflowchart TD\nA --> B\n```\n"
            with mock.patch.dict(os.environ, {"RHIZOME_CONFIG": str(cfg)}):
                findings = check.mermaid_findings(text)
            self.assertTrue(check.has_errors(findings))
            self.assertIn("config error", findings[0].message)
