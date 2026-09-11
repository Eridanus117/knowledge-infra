# Frozen Ledger v2

This contract defines the Git-backed safety boundary for source operations. It is
separate from the source registry and note parser, but it consumes their validated
values. A caller must resolve a `Registry`, use its `SourceSpec` roots, and use
`parse_and_validate_note` for every note that is inspected. There is no compatibility
lane for the legacy tab-separated ledgers.

## Public Rust boundary

The `rhizome-core` crate exposes these modules and types:

```rust
pub mod git {
    pub struct GitBackend { /* canonical repository root */ }
    impl GitBackend {
        pub fn new(root: impl Into<PathBuf>) -> Result<Self, GitError>;
        pub fn head_oid(&self) -> Result<String, GitError>;
        pub fn head_blob(&self, path: &Path) -> Result<Vec<u8>, GitError>;
        pub fn head_blob_sha256(&self, path: &Path) -> Result<String, GitError>;
        pub fn canonical_blob_sha256(bytes: &[u8]) -> String;
    }
}

pub mod frozen {
    pub struct ApprovalMarker { pub path: PathBuf, pub reason: String }
    impl ApprovalMarker {
        pub fn for_one_file(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self;
    }
    pub fn is_head_frozen(
        git: &GitBackend,
        path: &Path,
    ) -> Result<bool, FrozenError>;
    pub fn check_worktree_change(
        git: &GitBackend,
        path: &Path,
        approval: Option<&ApprovalMarker>,
    ) -> Result<(), FrozenError>;
    pub fn check_staged_frozen(
        git: &GitBackend,
    ) -> Result<(), FrozenError>;
}

pub mod relocate {
    pub fn plan_relocate(
        registry: &Registry,
        source: &Path,
        target: &str,
    ) -> Result<RelocatePlan, RelocateError>;
    pub fn apply_relocate(
        plan: &RelocatePlan,
    ) -> Result<(), RelocateError>;
}

pub mod amend {
    pub fn plan_amend(
        context: &SourceContext,
        path: &Path,
        replacement: &[u8],
        reason: &str,
        approval: &ApprovalMarker,
    ) -> Result<AmendPlan, AmendError>;
    pub fn apply_amend(
        plan: &AmendPlan,
    ) -> Result<(), AmendError>;
}
pub mod ledger {
    pub struct LedgerRecord {
        pub schema: String,
        pub operation: String,
        pub logical_source: String,
        pub old_identity: String,
        pub new_identity: String,
        pub old_path: String,
        pub new_path: String,
        pub head_oid: String,
        pub canonical_git_blob_sha256: String,
        pub reason: String,
    }
}

`RelocatePlan` and `AmendPlan` are opaque, owned plans. A plan captures the
canonical repository root, the complete Git `HEAD` object ID, the old path, the
old logical identity, and the old content's canonical Git-blob SHA-256. Applying a
plan MUST re-read and revalidate every captured value before changing any byte.
A changed `HEAD`, changed old blob, changed target, changed source registry, or
changed approval path fails closed and leaves all source bytes and ledgers as they
were before the apply attempt.

All Git interaction is through the system `git` executable. `gix`, libgit2, and
in-process Git object readers are not part of this boundary.

## Frozen determination and gate

A note is **HEAD-frozen** only when the committed bytes at `HEAD:<path>` parse as a
valid note and either have `status: frozen` or `kind: decision`. The working-tree
frontmatter MUST NOT determine this decision. In particular, changing a committed
`status: frozen` note to a living note in the same working tree is still an edit of
a HEAD-frozen note and is blocked.

A new, uncommitted note may itself contain `status: frozen` and is accepted as a
new note; it has no frozen predecessor in `HEAD`. An unchanged HEAD-frozen note is
accepted. A working-tree edit of a HEAD-frozen note, a staged delete, and a staged
rename are rejected by the gate unless they are one of the two audited operations
below. The gate must inspect staged paths with system Git and must not infer a
rename from filesystem names alone.

`ApprovalMarker::for_one_file` identifies exactly one canonical absolute path and a
non-empty reason. It is not a wildcard, a list, or a repository-wide bypass. An
amend approval for file A MUST NOT release a frozen edit of file B, a delete, or a
rename.

## Relocate operation

A relocate target is expressed as `logical-source:domain` or
`logical-source:domain:new-slug`. The logical source MUST be present in the resolved
`Registry`; the target domain MUST be a real C2 domain with its exact uppercase
`INDEX.md`; the target path MUST be inside that source root and MUST NOT already
exist. The source note must be a discovered, validated note. A source and target
may be in the same registered Git repository or in two different registered
repositories.

A valid relocate moves the exact source bytes without normalizing or rewriting
them, recomputes the position-derived identity, and writes an append-only record
under `.rhizome/relocate-ledger.ndjson` in every repository touched. A frozen
relocate is permitted only when the source working-tree bytes equal the committed
HEAD bytes. The destination's canonical Git-blob SHA-256 must equal the recorded
source hash. A destination copy with edited bytes, a forged hash, a missing
cross-source destination, a stale plan, or an existing destination path is
rejected. An unrecorded staged deletion or rename remains blocked.

A same-slug relocate changes only identity and path. A slug-changing relocate is
also required to preserve the note bytes; link rewriting is outside this contract.

## Amend operation

An amend is an audited, one-file replacement of a HEAD-frozen note. It requires a
non-empty reason and an `ApprovalMarker` whose canonical path is exactly the file
being amended. The replacement must parse as a valid v2 note and the operation
must stage/commit only that file plus its amend ledger record; it must never use a
blanket `--no-verify` or `git add -A`. A changed HEAD, forged/stale approval, second
staged frozen file, or content validation failure rejects the operation.

The amend ledger is `.rhizome/amend-ledger.ndjson`. The successful operation is
provenanced by its ledger record and by the system-Git commit made by the amend
operation. A failed operation MUST NOT append a record or alter the committed
HEAD.

## Canonical Git-blob SHA-256

`canonical_git_blob_sha256` is **not** the Git object ID. For raw blob bytes `B`,
it is the lowercase hexadecimal SHA-256 digest of:

```text
b"blob " + ASCII decimal byte length of B + b"\0" + B
```

The bytes `B` are the canonical bytes returned by `git show HEAD:<path>` (or the
staged Git blob when validating a staged destination). Working-tree decoding,
platform newline translation, and Unicode normalization are never applied before
this digest. Git's normal SHA-1 object ID, if available, is metadata only.

A working-tree CRLF materialization of a committed LF blob therefore MUST compare
using the LF Git blob bytes and its canonical Git-blob SHA-256. Hashing the CRLF
working-tree bytes directly is incorrect and MUST NOT unlock a frozen operation.

## NDJSON ledger schema

Each successful record is exactly one UTF-8 line terminated by `\n`. The file is
append-only. Blank lines, duplicate keys, unknown keys, non-object JSON, and
records with a different schema are invalid. Serialization is deterministic: the
keys occur in the order below, strings use JSON escaping, and there is no extra
whitespace.

```json
{"schema":"frozen-ledger-v2","operation":"relocate","logical_source":"knowledge","old_identity":"knowledge:docs:adr-1","new_identity":"knowledge:archive:adr-1","old_path":"docs/adr-1.md","new_path":"archive/adr-1.md","head_oid":"0123456789abcdef0123456789abcdef01234567","canonical_git_blob_sha256":"...64 lowercase hex...","reason":"approved content-preserving relocate"}
```

Every record has exactly these fields:

| Field | Type | Meaning |
|---|---|---|
| `schema` | string | Always `frozen-ledger-v2`. |
| `operation` | string | `relocate` or `amend`. |
| `logical_source` | string | Registry logical source owning the operation. |
| `old_identity` | string | Identity before the operation. For amend, it equals `new_identity`. |
| `new_identity` | string | Identity after the operation. |
| `old_path` | string | Git-root-relative POSIX path before the operation. |
| `new_path` | string | Git-root-relative POSIX path after the operation. For amend, it equals `old_path`. |
| `head_oid` | string | Full lowercase Git `HEAD` object ID captured by the plan. |
| `canonical_git_blob_sha256` | string | Lowercase SHA-256 of the canonical Git blob bytes defined above. |
| `reason` | string | One-line, non-empty audit reason; newline and control characters are rejected. |

The v2 schema deliberately has no timestamp, host name, absolute path, user name,
working-tree hash, Git OID in place of the blob digest, or free-form extension
field. These would make replay and comparison nondeterministic or machine-local.
Ledger parsing and frozen-gate validation MUST compare the logical source, both
identities, both paths, `head_oid`, and canonical blob digest; a matching reason
alone never authorizes a change.
