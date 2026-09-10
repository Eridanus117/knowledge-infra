# knowledge-infra

Shared Rust infrastructure for Git-backed Markdown knowledge workflows.
Markdown files and Git history remain the authoritative source; generated indexes and other projections are rebuildable views.

## Workspace

- `kb-contract` defines source-format boundary errors and will own the versioned source contract.
- `rhizome-core` exposes source-root, relative-path, and byte-reader boundaries for source-plane operations.
- `rhizome` is the command-line entry point.

The complete public histories of the predecessor Rhizome and Memex repositories are retained under `legacy/rhizome` and `legacy/memex` respectively. No Memex Rust package or vector/search backend is part of this bootstrap.

## Toolchain

The workspace uses Rust 1.97.0 and edition 2024. CI exercises the locked workspace on Windows x64 and macOS arm64.
