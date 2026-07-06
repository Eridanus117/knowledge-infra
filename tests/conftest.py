"""Make the suite hermetic against the host environment.

The `rhizome` launcher and local developer shells may export KB_*/RHIZOME_*
config-override vars (KB_SOURCES, KB_WORKSPACE_ROOT, …). The production code
reads those as overrides by design, so when they leak into the test process
they hijack the temp registries the tests build — `load_sources` /
`build_tree` / `diff` then resolve repos under the host workspace instead of
the tmpdir and tests fail. Strip them for every test; a test that needs a
specific value sets it itself.
"""

from __future__ import annotations

import pytest

# Everything the production code treats as a config override (see
# rhizome.sources): registry location, workspace root, and the central-index
# coordinates used by `domains --diff`.
_LEAK_VARS = (
    "KB_SOURCES",
    "KB_WORKSPACE_ROOT",
    "KB_QDRANT_URL",
    "KB_CENTRAL_COLLECTION",
    "RHIZOME_ASSET_PREFIXES",
    "RHIZOME_CODE_ROOTS",
    "RHIZOME_CONFIG",
    "RHIZOME_MERMAID_VALIDATOR_DIR",
    "RHIZOME_INBOX",
)


@pytest.fixture(autouse=True)
def _isolate_kb_env(monkeypatch):
    for var in _LEAK_VARS:
        monkeypatch.delenv(var, raising=False)
