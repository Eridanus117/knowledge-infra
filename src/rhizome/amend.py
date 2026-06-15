"""rhizome amend — audited in-place edit of ONE frozen KB doc.

Frozen docs (`status: frozen` / `kind: decision`) are read-only history; the
commit gate (`rhizome check`) blocks any edit to a HEAD-frozen file. The only
prior bypass was a bare `git commit --no-verify` — no record, no provenance,
靠人记得批. This command replaces that with a narrow, audited channel for the
true exceptions (typo fix / same-day reversal / dead link):

  rhizome amend <file> -m "<reason>"

What it does, fail-closed and self-verifying:

  1. Verify `<file>` is a HEAD-frozen KB note (its committed frontmatter is
     `status: frozen` or `kind: decision`) and the working tree actually
     diverges from HEAD — there is nothing to amend otherwise.
  2. Stage exactly that one file (never `-A`).
  3. Spawn the real `git commit` with RHIZOME_AMEND_APPROVED=<abs path> in its
     env. lefthook inherits the env and runs `rhizome check`, which drops ONLY
     that file's frozen-modification block (logging `approved amend: <file>`)
     — every other check still fires, so a frontmatter/Mermaid/link error in
     the amended file STILL blocks the commit. The approval is one path, not a
     `--no-verify` blanket.
  4. Record provenance two ways: a `Frozen-Amend-Approved: <reason>` commit
     trailer (per-commit, travels with git history) and an append-only ledger
     line under the repo's `decisions/.frozen-amend-ledger` (greppable audit).

Running this command interactively IS the approval action — the go/no-go stays
with a human; the code does not try to authenticate, it makes the exception
leave a trail.
"""

from __future__ import annotations

import os
import subprocess
from datetime import UTC, datetime
from pathlib import Path

from . import check, contract

LEDGER_NAME = ".frozen-amend-ledger"
TRAILER_KEY = "Frozen-Amend-Approved"


class AmendError(Exception):
    """Amend cannot proceed (not frozen, no changes, not a note) → exit 1."""


class AmendUsageError(AmendError):
    """Bad arguments / environment (no file, no repo, empty reason) → exit 2."""


def _git(
    repo_root: Path, *args: str, env: dict | None = None
) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", "-C", str(repo_root), *args],
        capture_output=True,
        text=True,
        timeout=30,
        env=env,
    )


def _working_tree_differs(repo_root: Path, rel: str) -> bool:
    """True if `rel` has staged or unstaged changes vs HEAD (incl. new/deleted)."""
    proc = _git(repo_root, "status", "--porcelain", "--", rel)
    if proc.returncode != 0:
        return False
    return bool(proc.stdout.strip())


def run_amend(file_arg: str, *, reason: str, cwd: Path | None = None) -> dict:
    """Commit an audited in-place edit of one frozen doc. Pure-ish (no printing).

    Returns {file, repo, rel, reason, commit, ledger}. Raises AmendUsageError
    (exit 2) / AmendError (exit 1). The actual git commit is the side effect.
    """
    cwd = cwd or Path.cwd()

    reason = reason.strip()
    if not reason:
        raise AmendUsageError(
            "a non-empty -m/--reason is required (it is the audit record)"
        )

    path = Path(file_arg).expanduser()
    if not path.is_absolute():
        path = cwd / path
    path = path.resolve()
    if not path.is_file():
        raise AmendUsageError(f"no such file: {path}")

    repo_root = contract.find_repo_root(path.parent)
    if repo_root is None:
        raise AmendUsageError(
            f"not inside a git repo (no .git found from {path.parent})"
        )
    repo_root = repo_root.resolve()

    if not contract.is_note_location(path):
        raise AmendError(
            f"{path} is not inside a KB domain (no INDEX.md ancestor) — amend only "
            "guards KB notes; ordinary files commit normally"
        )

    # The gate judges by the HEAD version's frontmatter; so must this. A file
    # whose HEAD is NOT frozen needs no amend channel — it commits normally.
    if not check.head_frozen(path):
        raise AmendError(
            f"{path} is not a frozen document in HEAD (status: frozen / kind: decision) — "
            "amend is only for frozen docs; edit and commit it normally"
        )

    try:
        rel = path.relative_to(repo_root).as_posix()
    except ValueError as exc:
        raise AmendError(f"{path} is not under repo root {repo_root}") from exc

    if not _working_tree_differs(repo_root, rel):
        raise AmendError(f"{rel} has no changes vs HEAD — nothing to amend")

    # Stage exactly this one file (never -A): the amend approval is per-file.
    add = _git(repo_root, "add", "--", rel)
    if add.returncode != 0:
        raise AmendError(f"git add {rel} failed: {add.stderr.strip()}")

    # Append-only audit ledger, greppable independent of git trailers.
    ledger = _append_ledger(repo_root, rel, reason)
    _git(repo_root, "add", "--", ledger.relative_to(repo_root).as_posix())

    # The narrow approval: one absolute path, only for this commit's subprocess.
    env = dict(os.environ)
    env[check._AMEND_APPROVED_ENV] = str(path)

    message = f"amend(frozen): {rel}\n\n{reason}\n\n{TRAILER_KEY}: {reason}\n"
    commit = _git(repo_root, "commit", "-m", message, env=env)
    if commit.returncode != 0:
        # The gate (or another check) refused — surface it; do NOT retry with
        # --no-verify. A non-frozen error (bad frontmatter, broken Mermaid) is a
        # real block the amend channel must not paper over.
        detail = (commit.stderr or commit.stdout).strip()
        raise AmendError(
            f"git commit failed (the gate or a content check refused — amend bypasses ONLY "
            f"the frozen block, never frontmatter/Mermaid/link errors):\n{detail}"
        )

    head = _git(repo_root, "rev-parse", "HEAD")
    commit_hash = head.stdout.strip() if head.returncode == 0 else "?"

    return {
        "file": str(path),
        "repo": str(repo_root),
        "rel": rel,
        "reason": reason,
        "commit": commit_hash,
        "ledger": str(ledger),
    }


def _append_ledger(repo_root: Path, rel: str, reason: str) -> Path:
    """Append one audit line to decisions/.frozen-amend-ledger; create if absent.

    The ledger lives next to the decisions it guards (a real domain dir if the
    repo has one, else the repo root). One ISO-8601 line per amend; the reason
    is single-lined so the file stays grep-friendly.
    """
    decisions = repo_root / "decisions"
    ledger_dir = decisions if decisions.is_dir() else repo_root
    ledger = ledger_dir / LEDGER_NAME
    ts = datetime.now(UTC).isoformat(timespec="seconds")
    one_line_reason = " ".join(reason.split())
    line = f"{ts}\t{rel}\t{one_line_reason}\n"
    with ledger.open("a", encoding="utf-8") as fh:
        fh.write(line)
    return ledger
