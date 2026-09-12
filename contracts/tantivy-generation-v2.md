# Tantivy immutable generations v2

A generation is a durable, immutable lexical projection of one canonical
`docs.ndjson` stream. Source repositories remain authoritative; the generation
store is rebuildable state and is never written back to a source repository.
Only the v2 contract described here is accepted. Readers do not fall back to an
older layout, stale `CURRENT`, or an incomplete generation.

## Layout and publication boundary

An `IndexManager` owns one directory with this exact public layout:

```text
<root>/
  CURRENT
  index.lock
  generations/
    <generation-id>/
      docs.ndjson
      manifest.json
      tantivy/
```

`<generation-id>` is 64 lowercase hexadecimal characters. A generation is
constructed in a uniquely named sibling temporary directory under
`generations/`, then its files and Tantivy index are synchronized, and only
then is that directory renamed to its final id. Temporary directories are
never valid generations. A failed build removes the temporary directory and
cannot change `CURRENT`.

`CURRENT` is exactly the UTF-8 bytes `<generation-id>\n`. Publication writes
those bytes to a temporary sibling file, synchronizes it, and atomically
replaces `CURRENT`. Unix uses `rename(2)`/`std::fs::rename`; Windows uses
`MoveFileExW` with both `MOVEFILE_REPLACE_EXISTING` and
`MOVEFILE_WRITE_THROUGH`. There is no delete-then-rename window and no
best-effort fallback. The root directory is synchronized after publication on
platforms that expose directory fsync.

Build work is performed without the publication lock. The short publication
critical section takes the exclusive `index.lock` advisory lock, validates the
candidate generation again, and then replaces `CURRENT`. Lock contention is a
failed, diagnosed operation. The lock file itself is persistent (and may be
zero length); ownership is the open file handle, so a crashed process does not
leave a stale logical lock. The current and previous generations are not
removed by this contract; retention is outside this API.

## Contract constants

The manifest schema and contract version are both `tantivy-generation-v2`.
The index profile is the central Tantivy profile `tantivy-central-v2` supplied
by `memex-core::tantivy_schema::INDEX_PROFILE`. A valid Tantivy commit must
contain the exact v2 schema and the exact commit payload:

```json
{"index_profile":"tantivy-central-v2"}
```

## Generation id and manifest

The public Rust boundary is:

```rust
pub struct GenerationId(String);
pub struct GenerationManifest {
    pub schema: String,
    pub id: GenerationId,
    pub docs_sha256: String,
    pub doc_count: u64,
    pub contract_version: String,
    pub index_profile: String,
}
pub struct GenerationReader {
    pub id: GenerationId,
    pub manifest: GenerationManifest,
    pub index: tantivy::Index,
}
pub struct IndexManager {
    pub root: std::path::PathBuf,
}

pub fn build_generation(
    manager: &IndexManager,
    records: &[memex_core::DocumentRecord],
) -> Result<GenerationId, memex_core::MemexError>;
pub fn open_current(
    manager: &IndexManager,
) -> Result<GenerationReader, memex_core::MemexError>;
pub fn publish(
    manager: &IndexManager,
    id: &GenerationId,
) -> Result<(), memex_core::MemexError>;
```

`docs.ndjson` is the canonical byte stream from
`memex_core::encode_ndjson(records)`. `docs_sha256` is lowercase SHA-256 over
those bytes, and `doc_count` is the number of records. The generation id is
lowercase SHA-256 over three framed byte strings, in order:

1. UTF-8 `contract_version` (`tantivy-generation-v2`);
2. UTF-8 `index_profile` (`tantivy-central-v2`);
3. the complete `docs.ndjson` bytes.

Each component is encoded as an unsigned 64-bit big-endian byte length,
followed immediately by that many bytes. Thus the hashed input is:

```text
u64be(len(contract_version)) || contract_version ||
u64be(len(index_profile))  || index_profile  ||
u64be(len(docs.ndjson))    || docs.ndjson
```

This framing makes boundaries unambiguous and is part of v2. Any change to the
contract version, index profile, or canonical document bytes changes the id.

`manifest.json` is one compact UTF-8 JSON object with exactly these six fields,
in exactly this order, followed by one LF byte:

```text
{"schema":"tantivy-generation-v2","id":"<generation-id>","docs_sha256":"<docs-sha256>","doc_count":<u64>,"contract_version":"tantivy-generation-v2","index_profile":"tantivy-central-v2"}\n
```

Strings use compact `serde_json` escaping. No leading/trailing whitespace,
extra fields, duplicate keys, alternate field order, floating-point count,
CRLF, or missing final LF is accepted. The codec rejects any schema,
contract-version, profile, id, docs digest, or count that does not match the
validated generation contents.

## Build, validation, and readers

`build_generation` first encodes and validates all records, calculates the id,
then writes `docs.ndjson`, the strict manifest, and a committed Tantivy index
to the temporary directory. Every regular file is synchronized before the
temporary directory is made visible; directory synchronization is performed
where supported. The final directory is immutable and never overwritten. If a
same-id final directory already exists, it must validate byte-for-byte against
the requested records or the operation fails closed.

Validation reads and decodes the entire canonical docs stream, checks its hash,
count, and id, strictly decodes the manifest, opens `tantivy/`, checks the
exact schema and commit payload, and verifies all committed segment files and
their live document count. `open_current` reads exactly one valid `CURRENT`
line and returns a reader for that validated generation. It never silently
selects another generation. Since `CURRENT` changes by atomic replacement and
published generation directories are immutable, a reader observes only the
old complete snapshot or the new complete snapshot.

`publish` validates before taking the short lock and revalidates while holding
it. Any validation, lock, synchronization, or atomic replacement failure
leaves the previous `CURRENT` bytes untouched. A generation cannot become
current unless its docs, manifest, and Tantivy commit all agree.
