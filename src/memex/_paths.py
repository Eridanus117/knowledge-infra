"""Filesystem-only paths; keep logical paths in identities, reports and return values."""

from __future__ import annotations

import os
from pathlib import Path


def io_path(path: Path) -> Path:
    """Use Windows extended paths for IO, including children of short directories.

    Normalize ordinary paths before adding the prefix: extended paths do not
    interpret relative segments. Already extended paths retain their semantics.
    This removes MAX_PATH limits, not the filesystem's per-component limit.
    """
    if os.name != "nt":
        return path
    value = str(path)
    if value.startswith("\\\\?\\"):
        return path
    value = os.path.abspath(value)
    if value.startswith("\\\\"):
        return Path("\\\\?\\UNC\\" + value[2:])
    return Path("\\\\?\\" + value)
