"""rhizome telemetry — thin glue over the gnomon shared core.

All schema / connect / record / stats / Tee / cap constants live in the shared
`gnomon` core (identical `calls` schema across source-gamma tools, so ledgers can
be unioned for cross-tool analysis). This module re-exports the public surface
rhizome callers expect, binding rhizome's Cfg once so call sites don't pass it.

rhizome is an argparse CLI (not Typer/Click), so it uses the `record` posture:
`cli.run()` owns the capture loop itself rather than
gnomon.run_instrumented's in-process Click wrapper. Only
record/stats/db_path/connect/Tee/caps are needed here.
"""

from __future__ import annotations

from pathlib import Path

import gnomon as ot
from gnomon.telemetry import STDERR_CAP, STDOUT_CAP, Tee, detect_caller

from rhizome import __version__

CFG = ot.Cfg(tool="rhizome", version=__version__)

# re-export connect unbound — same signature as core (takes a Path)
connect = ot.connect


def db_path() -> Path:
    return ot.db_path(CFG)


def record(rec: dict, *, path: Path | None = None) -> None:
    """Insert one invocation row. Best-effort: delegates to the shared core."""
    ot.record(rec, CFG, path=path)


def stats(path: Path | None = None) -> str:
    """Per-command summary (count / p50·p95·max ms / error rate) + recent faults."""
    return ot.stats(CFG, path=path)


__all__ = [
    "STDERR_CAP",
    "STDOUT_CAP",
    "Tee",
    "connect",
    "db_path",
    "detect_caller",
    "record",
    "stats",
]
