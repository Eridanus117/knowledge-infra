# Human Index v2

This contract defines the source-side human index projection owned by
`rhizome-core`. Markdown remains the only source of truth. The generated
catalog is a rebuildable projection inside a human-maintained Markdown page.

## Public boundary

The source plane exposes the following values and functions:

```rust
pub struct CheckReport {
    pub schema: &'static str,
    pub source: SourceName,
    pub findings: Vec<Diagnostic>,
}

pub struct HumanIndexPlan {
    pub path: PathBuf,
    pub replacement: Vec<u8>,
    pub prefix_sha256: String,
    pub suffix_sha256: String,
}

pub fn check_source(ctx: &SourceContext) -> Result<CheckReport, CoreError>;
pub fn plan_human_index(
    snapshot: &SourceSnapshot,
    index: &Path,
) -> Result<HumanIndexPlan, CoreError>;
pub fn apply_human_index(plan: HumanIndexPlan) -> Result<(), CoreError>;
```

`CheckReport.schema` is exactly `"rhizome-check-v2"`. `source` is the
registry logical source name from `SourceContext`/`SourceSnapshot`; it is never
derived from a Git root or physical directory basename.

`check_source` first consumes the existing source discovery boundary. A source
discovery failure is returned through `CoreError`; it is not converted into a
partial report and no later check runs on a partial snapshot. Successful
checks return a report, including reports with findings.

## Source-root relationship

For `plan_human_index`, `index.parent()` is the canonical source root. The
function MUST reject an index whose parent is not the source root represented by
the snapshot. In particular, an index outside that source root is never used as
an alternate input tree. Catalog paths are normalized source-relative POSIX
paths (`/` separators), independent of the host platform.

The root `INDEX.md` is the human index target. Source discovery excludes that
root landing page and every non-root domain `INDEX.md` from the generated
catalog. A valid non-index note is every `SnapshotNote` whose locator has
`is_domain_index == false`; no filesystem re-walk or second frontmatter parse
is performed by the projection.

## Markers and generated region

The only recognized markers are these exact UTF-8 byte strings:

```text
<!-- rhizome:generated-index:start -->
<!-- rhizome:generated-index:end -->
```

A human index MUST contain exactly one start marker and exactly one end marker,
and the start marker MUST precede the end marker. Missing, duplicated, or
reversed markers are errors. The marker lines and generated catalog use LF in
new output; bytes outside the marker span are not normalized.

The replacement is the complete marker span, including both marker lines. It
has this exact line-oriented shape:

```text
<!-- rhizome:generated-index:start -->
### <domain>
- [<identity>](<source-relative-posix-path>) — <description>
<!-- rhizome:generated-index:end -->
```

There is one `###` heading for each domain represented by an eligible note.
Each eligible note produces exactly one list entry. Entries are ordered first
by the NFC display spelling of `locator.domain`, then by the normalized
source-relative POSIX path of `locator.path`. Domain headings follow that same
order. A domain landing page and the root human index never produce an entry.

`<identity>` is the validated logical identity already present in the snapshot.
`<description>` is copied verbatim from the validated note frontmatter. The
projection MUST NOT trim, normalize, escape, fold, or otherwise rewrite that
string. The link destination is the normalized source-relative POSIX path and
is not an absolute path.

`plan_human_index` is side-effect-free: it reads the selected index and returns
an in-memory plan without writing any file. The plan's `prefix_sha256` and
`suffix_sha256` are lowercase SHA-256 digests of the exact bytes before the
start marker and after the end marker, respectively. The plan's `replacement`
is deterministic for an equal snapshot and equal index bytes.

`apply_human_index` rereads `plan.path`, locates the same unique marker span,
and recomputes both outside-region digests. It writes only when both digests
match the plan. A mismatch fails closed and leaves the current file untouched;
no best-effort merge or marker repair is allowed. On success, every byte before
the start marker and after the end marker is copied exactly and only the marker
span may differ.

## Check findings

All check findings reuse `kb_contract::Diagnostic`. Their `path` identifies
the source note or human index, and their `field` identifies `links`, `code`,
or the relevant projection field. The stable Task 6 findings are:

| Code | Severity | Field | Exact message | Condition |
|---|---|---|---|---|
| `KBV2-LINK-BROKEN` | Error | `links` | `link target does not resolve: <identity>` | A slug or logical-identity link does not resolve in the source snapshot. `<identity>` is the resolved logical identity, not a physical path. |
| `KBV2-CODE-BROKEN` | Warning | `code` | `code pointer does not resolve from the Git root` | A path-shaped `code` pointer does not resolve from `SourceContext.git_root`. |
| `KBV2-HUMAN-INDEX-MARKER` | Error | none | `human index markers are missing` | One or both markers are absent. |
| `KBV2-HUMAN-INDEX-MARKER` | Error | none | `human index markers must occur exactly once` | Either marker occurs more than once. |
| `KBV2-HUMAN-INDEX-MARKER` | Error | none | `human index end marker must follow start marker` | The end marker occurs before the start marker. |
| `KBV2-HUMAN-INDEX-DRIFT` | Error | none | `human index generated catalog is out of date` | The unique marker region differs from the deterministic catalog for the current snapshot. |

A `links` value that is a slug is resolved in its note's current domain. A
value already shaped as `<source>:<domain>:<slug>` is resolved as that exact
logical identity. Both forms are valid when they identify a snapshot note;
physical source paths and physical Git-root names are not identities.

A note with `status: frozen` is exempt from link and code-pointer findings, as
it is immutable historical material. Non-path code hints (for example a symbol
or a database relation) are not claimed to be filesystem paths and produce no
code-pointer finding. Path-shaped but missing pointers produce the warning
above.
