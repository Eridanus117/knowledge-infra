# Docs NDJSON v2

`docs.ndjson` is the versioned compiled-document stream consumed by the
Memex retrieval projections. Markdown, frontmatter, the source registry, and
Git remain authoritative; this file is a rebuildable view and is never written
back into a source root.

## Schema and record order

The schema identifier is `knowledge-doc-v2`. Every record is one compact JSON
object with exactly these fields, in this order:

```text
schema
identity
source
domain
domain_prefixes
title
description
keywords
kind
kind_explicit
status
body_text
source_path
source_hash
compiled_hash
commit_time
```

The corresponding Rust type is `memex_core::DocumentRecord`:

```rust
pub struct DocumentRecord {
    pub schema: &'static str,
    pub identity: String,
    pub source: String,
    pub domain: String,
    pub domain_prefixes: Vec<String>,
    pub title: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub kind: String,
    pub kind_explicit: bool,
    pub status: Option<String>,
    pub body_text: String,
    pub source_path: String,
    pub source_hash: String,
    pub compiled_hash: String,
    pub commit_time: Option<String>,
}
```

`schema` is always `knowledge-doc-v2`. `kind` uses the exact lowercase
source-contract spelling (`spec`, `reference`, `runbook`, `decision`,
`research`, `note`, or `index`). The only current non-null `status` is
`"frozen"`; an absent source status is JSON `null`.

JSON strings use the standard compact `serde_json` representation: UTF-8
non-ASCII code points remain UTF-8, quotes and backslashes are escaped, and
control characters use JSON escapes. Objects and arrays contain no formatting
whitespace. A record is terminated by one LF byte (`\n`), never CRLF.

## Source boundary and compilation

`compile_snapshot` consumes only a validated `rhizome_core::SourceSnapshot`
and an injected `CommitTimeSource`:

```rust
pub trait CommitTimeSource {
    fn commit_time(&self, path: &std::path::Path)
        -> Result<Option<String>, MemexError>;
}
```

The provider receives the absolute validated note path. Compilation does not
invoke Git, parse frontmatter, or write source files. `commit_time` is metadata
only and is not part of embedding text.

Only notes present in the validated snapshot are compiled. Consequently:

- a non-root domain `INDEX.md` appears once as `kind: "index"`;
- the source-root `INDEX.md` is absent;
- Markdown outside every domain is absent;
- a domain's `domain_prefixes` are its cumulative prefixes, so `a/b` becomes
  `["a", "a/b"]`;
- `source_path` is the normalized source-relative POSIX path, with `/`
  separators and no `.`/`..`, empty, absolute, backslash, colon, or control
  segments.

Records are sorted by their complete logical `identity` using the Rust string
ordering. The caller's snapshot and record slice are not mutated.

## Text projection

`body_text` is the validated body decoded as UTF-8, with CRLF and lone CR
normalized to LF and leading/trailing LF characters removed. Other code points
and spaces are retained.

`title` is the first body line beginning with exactly one `#` followed by an
ASCII space or tab, with surrounding whitespace removed from the heading text.
If no such non-empty heading exists, it is the UTF-8 filename stem (including
`INDEX` for a domain index).

The embedding input is not a field in the NDJSON record. It is exposed by
`DocumentRecord::embedding_text` and is exactly the non-empty concatenation of:

1. `description.trim()`;
2. all keywords joined with one ASCII space, then trimmed;
3. `body_text.trim()`;

Non-empty parts are separated by two LF bytes. `title`, `identity`, `source_path`,
hashes, `status`, and `commit_time` do not contribute to this input.

## Hashes

`source_hash` is lowercase hexadecimal SHA-256 over the complete validated
`ValidatedNote::original` UTF-8 input. Before hashing, every CRLF and lone CR
line ending is replaced with one LF. The optional leading UTF-8 BOM, if present,
is included as source content; only line endings are normalized.

`compiled_hash` is lowercase hexadecimal SHA-256 over the compact UTF-8 JSON
object formed from the fixed field order below, with `compiled_hash` omitted:

```text
schema, identity, source, domain, domain_prefixes, title, description,
keywords, kind, kind_explicit, status, body_text, source_path, source_hash,
commit_time
```

The emitted record then inserts `compiled_hash` between `source_hash` and
`commit_time`, without changing the hash projection. Hashes are exactly 64
lowercase hexadecimal characters.

## Canonical codec

The public codec is:

```rust
pub fn encode_ndjson(records: &[DocumentRecord]) -> Result<Vec<u8>, MemexError>;
pub fn decode_ndjson(bytes: &[u8]) -> Result<Vec<DocumentRecord>, MemexError>;
```

`encode_ndjson` validates each record, sorts a borrowed copy by identity,
rejects duplicate identities, and emits one canonical line per record. An
empty input emits an empty byte vector. It never mutates its input.

`decode_ndjson` accepts an empty byte vector as an empty stream. Otherwise it
requires exactly one final LF, rejects blank lines and CR bytes, parses exactly
the sixteen fields above, validates field types, schema, source-relative path,
hash syntax, and the compiled-hash projection, and rejects duplicate or
out-of-order identities. Re-encoding each decoded record must reproduce its
input line byte-for-byte; non-canonical field order, escaping, whitespace,
number/boolean/null spelling, or missing/extra fields is rejected.

Failures are returned as typed `MemexError` values. The stable categories are
`InvalidSchema`, `InvalidHash`, `InvalidPath`, `DuplicateIdentity`,
`NonCanonicalNdjson`, `InvalidNdjson`, `InvalidDocument`, and `CommitTime`.
Errors never cause a partial decoded stream to be returned.

The public byte fixture in `fixtures/memex/docs/expected.ndjson` is the
canonical output for `fixtures/memex/docs/tree` with the test commit-time
provider. `crates/memex-core/tests/docs_ndjson.rs` is the executable contract
for ordering, escaping, line endings, filtering, title and embedding rules,
hash validation, duplicate rejection, and deterministic bytes.
