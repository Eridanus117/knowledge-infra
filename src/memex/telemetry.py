"""Per-invocation telemetry: capture each CLI run into a local SQLite ledger and
expose `stats` for per-verb count / p50·p95 duration / error-rate.

Local-only, no network — this is usage observability to drive the tool's own
improvement (which verbs run, how long, what fails), not product analytics.

Why SQLite over a flat jsonl: several processes may invoke the CLI at once
(agents, scripts, shells). WAL + busy_timeout gives clean concurrent appends
with no torn-write footguns, and SQL keeps `stats` fast as the ledger grows.
Percentiles are computed in pure Python (SQLite has no portable percentile).

Best-effort throughout: telemetry must never fail the command or change its exit
code — every write path swallows its own errors.
"""

from __future__ import annotations

import json
import os
import sqlite3
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Any

from memex import __version__

# stderr is small + high-value (errors/warnings) so sample generously; stdout can
# be huge so keep only a head sample (full size lives in out_bytes).
STDOUT_CAP = 2048
STDERR_CAP = 4096

_ENV_DB = "KB_SEARCH_TELEMETRY_DB"
_ENV_OFF = "KB_SEARCH_TELEMETRY_OFF"


def db_path() -> Path:
    """Resolve the ledger path: $KB_SEARCH_TELEMETRY_DB override,
    else ($XDG_DATA_HOME or ~/.local/share)/memex/telemetry.db."""
    override = os.environ.get(_ENV_DB)
    if override:
        return Path(override)
    base = os.environ.get("XDG_DATA_HOME") or os.path.join(
        os.path.expanduser("~"), ".local", "share"
    )
    return Path(base) / "memex" / "telemetry.db"


def _disabled() -> bool:
    # DO_NOT_TRACK is the de-facto opt-out standard; honour it plus a per-tool flag.
    return bool(os.environ.get(_ENV_OFF) or os.environ.get("DO_NOT_TRACK"))


class Tee:
    """Wraps the real stdout/stderr: writes through while counting total UTF-8
    bytes and keeping the first `cap` bytes as a sample — lets telemetry observe
    output size without the command knowing it's watched."""

    def __init__(self, real: Any, cap: int) -> None:
        self.real = real
        self.cap = cap
        self.total = 0
        self._buf = bytearray()

    def write(self, s: str) -> int:
        n = self.real.write(s)  # pass through; the real stream owns its own errors
        try:
            # telemetry accounting must NEVER raise into the command's write path
            b = s.encode("utf-8", "replace") if isinstance(s, str) else bytes(s)
            self.total += len(b)
            if len(self._buf) < self.cap:
                self._buf.extend(b[: self.cap - len(self._buf)])
        except Exception:
            pass
        return n if isinstance(n, int) else len(s)

    def flush(self) -> None:
        self.real.flush()

    def isatty(self) -> bool:
        try:
            return self.real.isatty()
        except Exception:
            return False

    @property
    def sample(self) -> str:
        return self._buf.decode("utf-8", "replace")


_SCHEMA = """
CREATE TABLE IF NOT EXISTS calls (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts          TEXT    NOT NULL DEFAULT '',
    pid         INTEGER NOT NULL DEFAULT 0,
    verb        TEXT    NOT NULL DEFAULT '',
    args        TEXT    NOT NULL DEFAULT '[]',
    exit_code   INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    out_bytes   INTEGER NOT NULL DEFAULT 0,
    stdout      TEXT    NOT NULL DEFAULT '',
    stderr      TEXT    NOT NULL DEFAULT '',
    err         TEXT    NOT NULL DEFAULT '',
    cwd         TEXT    NOT NULL DEFAULT '',
    version     TEXT    NOT NULL DEFAULT '',
    is_tty      INTEGER NOT NULL DEFAULT 0,
    is_ci       INTEGER NOT NULL DEFAULT 0,
    meta        TEXT    NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS calls_verb ON calls(verb);
"""

_COLUMNS = (
    "ts",
    "pid",
    "verb",
    "args",
    "exit_code",
    "duration_ms",
    "out_bytes",
    "stdout",
    "stderr",
    "err",
    "cwd",
    "version",
    "is_tty",
    "is_ci",
    "meta",
)


def _connect(path: Path) -> sqlite3.Connection:
    """Open the ledger in WAL mode with a busy timeout so concurrent CLI
    processes queue rather than fail. isolation_level=None → explicit txns.
    Closes itself if schema setup fails, so callers never leak a connection."""
    path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(path), timeout=5.0, isolation_level=None)
    try:
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA busy_timeout=5000")
        conn.execute("PRAGMA synchronous=NORMAL")
        conn.executescript(_SCHEMA)
        conn.execute("PRAGMA user_version=1")  # migration anchor for future columns
    except Exception:
        conn.close()
        raise
    return conn


def record(rec: dict, *, path: Path | None = None) -> None:
    """Insert one invocation row. Best-effort: any failure (locked db, bad fs,
    disabled) is swallowed so telemetry never affects the command's outcome."""
    if _disabled():
        return
    try:
        conn = _connect(path or db_path())
        try:
            values = (
                str(rec.get("ts", "")),
                int(rec.get("pid", 0)),
                str(rec.get("verb", "")),
                json.dumps(rec.get("args", []), ensure_ascii=False),
                int(rec.get("exit_code", 0)),
                int(rec.get("duration_ms", 0)),
                int(rec.get("out_bytes", 0)),
                str(rec.get("stdout", "")),
                str(rec.get("stderr", "")),
                str(rec.get("err", "")),
                str(rec.get("cwd", "")),
                str(rec.get("version", "")),
                1 if rec.get("is_tty") else 0,
                1 if rec.get("is_ci") else 0,
                json.dumps(rec.get("meta", {}), ensure_ascii=False),
            )
            placeholders = ",".join("?" * len(_COLUMNS))
            conn.execute("BEGIN IMMEDIATE")
            conn.execute(
                f"INSERT INTO calls ({','.join(_COLUMNS)}) VALUES ({placeholders})",
                values,
            )
            conn.execute("COMMIT")
        finally:
            conn.close()
    except Exception:
        return


def _now_iso() -> str:
    # local time, ms precision, with offset — e.g. 2026-06-09T20:00:00.123+08:00
    return datetime.now().astimezone().isoformat(timespec="milliseconds")


def run_instrumented(
    app: Any,
    argv: list[str],
    *,
    prog_name: str = "memex",
    meta: dict | None = None,
    path: Path | None = None,
) -> int:
    """Run a Typer/Click `app` for one invocation under telemetry capture, record
    a row, and return the exit code. Lets click own its exit semantics
    (standalone_mode=True — exits via SystemExit, prints its own usage/errors);
    the wrapper just captures stdout/stderr + the exit code + any unexpected
    fault, uniformly for every command. Wire it as the console-script entry:
    `def run(): raise SystemExit(run_instrumented(app, sys.argv[1:]))`."""
    import typer

    cmd = typer.main.get_command(app) if isinstance(app, typer.Typer) else app

    start = time.monotonic()
    # first non-flag token is the subcommand; a heuristic (a flag *value* before
    # the subcommand can fool it, but it beats argv[0] for `tool --flag verb`).
    verb = next((a for a in argv if not a.startswith("-")), "")
    try:
        is_tty = sys.stdout.isatty()
    except Exception:
        is_tty = False

    real_out, real_err = sys.stdout, sys.stderr
    out_tee, err_tee = Tee(real_out, STDOUT_CAP), Tee(real_err, STDERR_CAP)
    sys.stdout, sys.stderr = out_tee, err_tee

    exit_code = 0
    err_msg = ""
    try:
        cmd(args=argv, standalone_mode=True, prog_name=prog_name)
    except SystemExit as e:
        # click exits via SystemExit: 0 ok, 2 usage error, 1 abort, ...
        exit_code = e.code if isinstance(e.code, int) else (0 if e.code is None else 1)
    except Exception as e:
        # an unexpected fault inside a command (click re-raises non-click errors):
        # map to a clean exit + a recorded row, never a traceback.
        print(f"error: {e}", file=sys.stderr)
        err_msg = str(e)
        exit_code = 1
    finally:
        sys.stdout, sys.stderr = real_out, real_err

    # Stamp the agent session id when present: Claude Code sets
    # CLAUDE_CODE_SESSION_ID on the Bash subprocess, so telemetry from several
    # tools in one agent task can be grouped/sequenced by meta->>'session'.
    # Absent under a plain shell / CI → simply omitted, no-op.
    session = os.environ.get("CLAUDE_CODE_SESSION_ID", "")
    record(
        {
            "ts": _now_iso(),
            "pid": os.getpid(),
            "verb": verb,
            "args": argv[1:],
            "exit_code": exit_code,
            "duration_ms": int((time.monotonic() - start) * 1000),
            "out_bytes": out_tee.total,
            "stdout": out_tee.sample,
            "stderr": err_tee.sample,
            "err": err_msg,
            "cwd": os.getcwd(),
            "version": __version__,
            "is_tty": is_tty,
            "is_ci": bool(os.environ.get("CI")),
            "meta": {**(meta or {}), **({"session": session} if session else {})},
        },
        path=path,
    )
    return exit_code


# ---- stats ----


def _pctile(xs: list[int], p: int) -> int:
    if not xs:
        return 0
    s = sorted(xs)
    if p >= 100:
        return s[-1]
    idx = p * len(s) // 100
    return s[min(idx, len(s) - 1)]


def _is_fault(exit_code: int, err: str) -> bool:
    """A real fault = a non-empty err message OR a hard exit (>= 2: usage /
    unknown command). An intentional exit-1-without-message (e.g. a lint that
    found problems) is the tool working, not failing — not counted as an error."""
    return bool(err.strip()) or exit_code >= 2


def _first_line(s: str) -> str:
    s = s.strip()
    i = s.find("\n")
    return s[:i] if i >= 0 else s


def stats(path: Path | None = None) -> str:
    """Return a per-verb summary (count / p50·p95·max ms via nearest-rank / error
    count) plus the most recent faults. Returns text (callers print, tests assert)
    and is itself best-effort: a missing/unreadable ledger yields a human note,
    never a traceback — `stats` runs inside the same telemetry wrapper.

    Note: the ledger grows unbounded (one row per call). It's local and cheap to
    delete to reset; heavy users can prune `DELETE FROM calls WHERE id < ...`."""
    p = path or db_path()
    if not p.exists():
        return f"no telemetry yet ({p}): run the CLI a few times first"
    try:
        conn = _connect(p)
        try:
            # bulk pass pulls only the light columns (no stdout/stderr) so memory
            # stays flat as the ledger grows; stderr is fetched only for the few
            # recent fault rows below.
            rows = conn.execute(
                "SELECT verb, exit_code, duration_ms, err "
                "FROM calls WHERE verb != '' ORDER BY id"
            ).fetchall()
            recent_rows = conn.execute(
                "SELECT ts, verb, err, stderr FROM calls "
                "WHERE verb != '' AND (err != '' OR exit_code >= 2) "
                "ORDER BY id DESC LIMIT 10"
            ).fetchall()
        finally:
            conn.close()
    except sqlite3.DatabaseError:
        return f"telemetry ledger unreadable ({p}); remove it to reset"

    if not rows:
        return "telemetry is empty"

    count: dict[str, int] = {}
    durs: dict[str, list[int]] = {}
    err_cnt: dict[str, int] = {}
    total = total_err = 0

    for verb, exit_code, duration_ms, err in rows:
        total += 1
        count[verb] = count.get(verb, 0) + 1
        durs.setdefault(verb, []).append(int(duration_ms or 0))
        if _is_fault(int(exit_code or 0), err or ""):
            total_err += 1
            err_cnt[verb] = err_cnt.get(verb, 0) + 1

    verbs = sorted(count, key=lambda v: (-count[v], v))
    rate = 100.0 * total_err / total
    lines = [f"memex telemetry — {total} calls · {total_err} errors ({rate:.1f}%)", ""]
    lines.append(f"{'verb':<14}{'count':>7}{'p50':>8}{'p95':>8}{'max':>8}{'err':>6}")
    for v in verbs:
        d = durs[v]
        lines.append(
            f"{v:<14}{count[v]:>7}"
            f"{str(_pctile(d, 50)) + 'ms':>8}"
            f"{str(_pctile(d, 95)) + 'ms':>8}"
            f"{str(_pctile(d, 100)) + 'ms':>8}"
            f"{err_cnt.get(v, 0):>6}"
        )
    if recent_rows:
        lines.append("")
        lines.append("recent errors (last 10):")
        for ts, verb, err, stderr in reversed(recent_rows):  # oldest → newest
            msg = (err or "").strip() or _first_line(stderr or "")
            lines.append(f"  {ts}  {verb:<12} {msg}")
    return "\n".join(lines)
