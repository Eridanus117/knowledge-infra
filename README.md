# knowledge-infra

Shared Rust infrastructure for Git-backed Markdown knowledge workflows.
Markdown files and Git history remain the authoritative source; generated indexes and other projections are rebuildable views.

## Workspace

- `kb-contract` owns the strict `source-contract-v2` registry values and versioned diagnostics.
- `rhizome-core` discovers validated C2 domains and logical note identities inside bounded Git-backed source roots.
- `rhizome` is the command-line entry point.

The complete histories of the predecessor Rhizome and Memex repositories are retained under `legacy/rhizome` and `legacy/memex` respectively. No Memex Rust package or vector/search backend is part of this bootstrap.

## Rhizome CLI v2

The `rhizome` binary exposes only the source-plane commands `new`, `check`,
`domains`, `adopt`, `doctor`, `amend`, `relocate`, `capture`, `stats`, and
`index check|sync`. Machine consumers can pass `--json` to receive one
`rhizome-cli-v2` envelope; logs never share stdout with that envelope.

`capture` is intentionally raw and appends to `RHIZOME_INBOX` (or
`~/.config/rhizome/inbox.md`) rather than creating a source-domain note.

## Toolchain

The workspace uses Rust 1.97.0 and edition 2024. CI exercises the locked workspace on Windows x64 and macOS arm64.
