"""memex telemetry — thin binding over gnomon core.

memex is an in-process Typer app, so it uses the `run_instrumented` posture:
the core tees stdout/stderr, times the run, and records a row automatically.

The shared core (schema / connect / record / stats / percentiles) lives in
gnomon under an identical `calls` schema, so tool ledgers can be
unioned for cross-tool analysis.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import gnomon as ot
from gnomon.telemetry import (  # re-export for tests + internal callers
    Tee as Tee,
)
from gnomon.telemetry import (
    _is_fault as _is_fault,
)
from gnomon.telemetry import (
    _pctile as _pctile,
)
from gnomon.telemetry import (
    connect as _connect,  # noqa: F401
)

from memex import __version__

CFG = ot.Cfg(tool="memex", version=__version__)


def db_path() -> Path:
    return ot.db_path(CFG)


def record(rec: dict, *, path: Path | None = None) -> None:
    """Insert one invocation row; delegates to gnomon core under memex's Cfg."""
    ot.record(rec, CFG, path=path)


def run_instrumented(  # noqa: PLR0913 — thin pass-through mirror of ot.run_instrumented; each kwarg is an independent caller-tunable override, not a cohesive object
    app: Any,
    argv: list[str],
    *,
    command_path: list[str] | None = None,
    prog_name: str | None = None,
    meta: dict | None = None,
    path: Path | None = None,
) -> int:
    """Run a Typer/Click `app` under telemetry capture; delegates to gnomon core."""
    return ot.run_instrumented(
        app,
        argv,
        CFG,
        command_path=command_path,
        prog_name=prog_name,
        meta=meta,
        path=path,
    )


def stats(path: Path | None = None) -> str:
    return ot.stats(CFG, path=path)
