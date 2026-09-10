"""rhizome capture — low-friction fleeting-thought inbox.

  rhizome capture <text ...>        # words joined into one line
  echo "..." | rhizome capture      # or pipe the thought via stdin

A fleeting thought is not yet knowledge. The write bars for a §存 note
(`rhizome new`, 5-field frontmatter, a domain) and a source-beta issue are
deliberately high; there was no low-bar slot for "jot this before it's gone".
`capture` fills that slot: one timestamped line appended to a plain-Markdown
inbox, then triaged later into `rhizome new` / source-beta and deleted.

This is `capture (raw)` in ADR-022 terms — raw, out of the KB boundary, NOT a
note and NOT indexed. The default inbox lives under `~/.config/rhizome/`, which
has no `INDEX.md`, so it is structurally outside every domain.
"""

from __future__ import annotations

import os
from datetime import datetime
from pathlib import Path

from . import config

INBOX_ENV = "RHIZOME_INBOX"
DEFAULT_INBOX = Path("~/.config/rhizome/inbox.md")


class CaptureError(Exception):
    """Nothing to capture (empty thought) → exit 1."""


def inbox_path() -> Path:
    """Resolve the inbox file: $RHIZOME_INBOX, else the default under ~/.config."""
    env = os.environ.get(INBOX_ENV)
    if env:
        return config.expand_path(env)
    return DEFAULT_INBOX.expanduser()


def run_capture(
    text: str, *, inbox: Path | None = None, now: datetime | None = None
) -> dict:
    """Append one timestamped line to the inbox; return {path, timestamp, text, line}.

    Pure I/O, no printing. `text` is collapsed to a single line so each capture
    is exactly one timeline entry (memos-style). `inbox`/`now` are injectable for
    tests; production resolves them from the environment / wall clock.
    """
    text = " ".join(text.split())
    if not text:
        raise CaptureError(
            "nothing to capture; give the thought as arguments or pipe it via stdin"
        )

    dest = inbox or inbox_path()
    dest.parent.mkdir(parents=True, exist_ok=True)

    stamp = (now or datetime.now().astimezone()).isoformat(timespec="seconds")
    line = f"- {stamp} {text}"
    with dest.open("a", encoding="utf-8") as fh:
        fh.write(line + "\n")

    return {"path": str(dest), "timestamp": stamp, "text": text, "line": line}
