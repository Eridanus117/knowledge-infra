from __future__ import annotations

import os
from pathlib import Path

import pytest
import typer

from memex import telemetry


def _rec(
    command: str, exit_code: int = 0, duration_ms: int = 10, err: str = ""
) -> dict:
    return {
        "ts": "2026-01-01T00:00:00.000+00:00",
        "pid": 1,
        "command_path": [command],
        "args": [],
        "exit_code": exit_code,
        "duration_ms": duration_ms,
        "err": err,
    }


def test_record_and_stats_roundtrip(tmp_path: Path) -> None:
    db = tmp_path / "t.db"
    for i in range(5):
        telemetry.record(_rec("list", duration_ms=i * 10), path=db)
    telemetry.record(_rec("new", exit_code=2, err="boom"), path=db)
    out = telemetry.stats(path=db)
    assert "list" in out
    assert "new" in out
    assert "6 calls" in out
    assert "boom" in out  # the fault message surfaces in recent errors


def test_stats_missing_db(tmp_path: Path) -> None:
    assert "no telemetry yet" in telemetry.stats(path=tmp_path / "absent.db")


def test_percentile() -> None:
    xs = list(range(1, 101))  # 1..100
    assert telemetry._pctile(xs, 50) == 51  # idx = 50*100//100 = 50 -> xs[50] = 51
    assert telemetry._pctile(xs, 95) == 96
    assert telemetry._pctile(xs, 100) == 100  # max special-cased
    assert telemetry._pctile([], 50) == 0


def test_is_fault() -> None:
    # real fault: non-empty err, or hard exit (>= 2)
    assert telemetry._is_fault(1, "boom") is True
    assert telemetry._is_fault(2, "") is True
    # not a fault: clean exit, or intentional exit-1 without a message
    assert telemetry._is_fault(0, "") is False
    assert telemetry._is_fault(1, "") is False  # e.g. a lint that found problems


def test_disabled_suppresses_write(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    db = tmp_path / "off.db"
    monkeypatch.setenv("DO_NOT_TRACK", "1")
    telemetry.record(_rec("list"), path=db)
    assert not db.exists()  # nothing written when opted out


def test_run_instrumented_records_and_maps_exit(tmp_path: Path) -> None:
    app = typer.Typer()

    @app.command()
    def ok() -> None:
        typer.echo("done")

    @app.command()
    def boom() -> None:
        raise RuntimeError("kaboom")

    db = tmp_path / "r.db"
    assert telemetry.run_instrumented(app, ["ok"], prog_name="t", path=db) == 0
    assert telemetry.run_instrumented(app, ["boom"], prog_name="t", path=db) == 1
    # unknown command → click usage error (standalone_mode owns it) → exit 2
    assert telemetry.run_instrumented(app, ["nope"], prog_name="t", path=db) == 2
    # bare/help carries no verb → recorded but excluded from per-verb stats
    telemetry.run_instrumented(app, ["--help"], prog_name="t", path=db)

    out = telemetry.stats(path=db)
    assert "ok" in out
    assert "boom" in out
    assert "nope" in out  # the usage-error verb is counted as a fault
    assert "kaboom" in out  # the raised fault message was captured into the ledger


def test_verb_skips_leading_flags(tmp_path: Path) -> None:
    """A global flag before the subcommand must not blank out the verb."""
    app = typer.Typer()

    @app.callback()
    def _root(verbose: bool = False) -> None:
        pass

    @app.command()
    def sync() -> None:
        typer.echo("synced")

    db = tmp_path / "v.db"
    assert (
        telemetry.run_instrumented(
            app, ["--verbose", "sync"], command_path=["sync"], prog_name="t", path=db
        )
        == 0
    )
    assert "sync" in telemetry.stats(path=db)


def test_record_best_effort_never_raises(tmp_path: Path) -> None:
    bad_parent = tmp_path / "afile"
    bad_parent.write_text("x")  # a file → can't mkdir a dir under it
    telemetry.record(_rec("x"), path=bad_parent / "sub.db")  # must not raise
    assert not (bad_parent / "sub.db").exists()


def test_run_instrumented_best_effort_on_unwritable_db(tmp_path: Path) -> None:
    bad_parent = tmp_path / "afile"
    bad_parent.write_text("x")
    app = typer.Typer()

    @app.callback()
    def _root() -> None:  # keep it a group so "ok" stays a subcommand
        pass

    @app.command()
    def ok() -> None:
        typer.echo("ok")

    # telemetry can't write, but the command still succeeds with its real code
    assert (
        telemetry.run_instrumented(app, ["ok"], prog_name="t", path=bad_parent / "s.db")
        == 0
    )


def test_tee_accounting_never_raises() -> None:
    class _Sink:
        def __init__(self) -> None:
            self.parts: list = []

        def write(self, s: object) -> int:
            self.parts.append(s)
            return 1

    t = telemetry.Tee(_Sink(), cap=100)
    t.write("hello")
    t.write(b"bytes")  # non-str input must not raise in the accounting path
    assert "hello" in t.sample


def test_stats_corrupt_db(tmp_path: Path) -> None:
    db = tmp_path / "bad.db"
    db.write_text("not a database")
    assert "unreadable" in telemetry.stats(path=db)  # human note, never a traceback


def test_concurrent_writes(tmp_path: Path) -> None:
    """Many processes appending at once must not lose or corrupt rows — the whole
    reason for SQLite (WAL + busy_timeout) over a flat jsonl."""
    if not hasattr(os, "fork"):
        pytest.skip("needs fork for true multi-process concurrency")
    import multiprocessing as mp

    db = tmp_path / "c.db"
    telemetry.record(_rec("seed"), path=db)  # create schema once before the race

    per_proc, n_proc = 25, 8

    def worker() -> None:  # fork inherits this closure — no pickling needed
        for _ in range(per_proc):
            telemetry.record(_rec("hit"), path=db)

    ctx = mp.get_context("fork")
    procs = [ctx.Process(target=worker) for _ in range(n_proc)]
    for p in procs:
        p.start()
    for p in procs:
        p.join()
        assert p.exitcode == 0

    conn = telemetry._connect(db)
    try:
        total = conn.execute("SELECT count(*) FROM calls").fetchone()[0]
        hits = conn.execute(
            "SELECT count(*) FROM calls WHERE command_path='[\"hit\"]'"
        ).fetchone()[0]
    finally:
        conn.close()
    assert hits == per_proc * n_proc  # no lost writes under contention
    assert total == per_proc * n_proc + 1
