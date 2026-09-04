"""User-level rhizome configuration."""

from __future__ import annotations

import os
import tomllib
from pathlib import Path
from typing import Any

CONFIG_ENV = "RHIZOME_CONFIG"
DEFAULT_CONFIG = Path("~/.config/rhizome/config.toml")


class ConfigError(Exception):
    """config.toml is explicitly requested but missing or invalid."""


def config_path() -> Path:
    raw = os.environ.get(CONFIG_ENV)
    if raw:
        return Path(os.path.expandvars(raw)).expanduser()
    return DEFAULT_CONFIG.expanduser()


def load_config(path: Path | None = None) -> dict[str, Any]:
    explicit = path is not None or bool(os.environ.get(CONFIG_ENV))
    cfg = path or config_path()
    if not cfg.is_file():
        if explicit:
            raise ConfigError(f"config file missing: {cfg}")
        return {}
    try:
        data = tomllib.loads(cfg.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise ConfigError(f"{cfg}: {exc}") from exc
    if not isinstance(data, dict):
        raise ConfigError(f"{cfg}: top-level config must be a TOML table")
    return data


def expand_path(raw: str) -> Path:
    return Path(os.path.expandvars(raw)).expanduser()
