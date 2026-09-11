# Source Contract v2

## Registry scope

This document defines the `source-contract-v2` source registry. The registry is the only source of logical source membership and physical source roots. Registry loading is strict and fail-loud: a failure returns diagnostics, no partial registry, and no fallback data.

The public Rust boundary is:

```rust
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub path: Option<PathBuf>,
    pub field: Option<String>,
    pub message: String,
}

pub enum Severity {
    Error,
    Warning,
}

pub struct SourceName(String);

pub enum Surface {
    Core,
    Vertical,
}

pub struct SourceSpec {
    pub name: SourceName,
    pub root: PathBuf,
    pub surface: Surface,
}

pub struct Registry {
    pub origin: PathBuf,
    pub sources: BTreeMap<SourceName, SourceSpec>,
}

pub struct RegistryLocator {
    pub explicit: Option<PathBuf>,
    pub cwd: PathBuf,
    pub env_path: Option<PathBuf>,
    pub workspace_root: Option<PathBuf>,
    pub user_config: PathBuf,
}

pub fn resolve_registry(locator: &RegistryLocator) -> Result<Registry, Vec<Diagnostic>>;
```

`resolve_registry` is a library boundary. It returns typed values or `Vec<Diagnostic>` and never writes to stdout or stderr.

## Main registry shape

A registry is UTF-8 TOML with only these top-level fields:

```toml
workspace_root = "optional/base"

[[source]]
name = "required-logical-name"
path = "optional/physical/root"
surface = "core" # optional; `vertical` is the default
```

The strict field and type rules are:

| Location | Field | Presence | TOML type | Meaning |
|---|---|---:|---|---|
| top level | `workspace_root` | optional | string | Base used only for a source whose `path` is omitted. |
| top level | `source` | required, non-empty collection | array of tables (`[[source]]`) | Source rows. |
| source row | `name` | required | string | Stable logical identity. |
| source row | `path` | optional | string | Physical source root. |
| source row | `surface` | optional | string | `core` or `vertical`; defaults to `vertical`. |

There is no scalar coercion. Every other top-level or source-row field is an error. In particular, `legacy` is removed rather than merely unknown and always uses `KBV2-REGISTRY-REMOVED-FIELD`.

A legal `name` matches the ASCII, case-sensitive regular expression `^[a-z][a-z0-9-]*$`. The shortest legal name is one lowercase letter. Uppercase letters, a leading digit or hyphen, whitespace, underscore, dot, slash, and backslash are illegal. Names are unique within the main registry.

## Registry location

The candidates, in order, are:

1. `RegistryLocator.explicit` (an explicit caller argument).
2. `RegistryLocator.env_path` (the caller-provided value of `KB_SOURCES`).
3. `RegistryLocator.workspace_root/kb-sources.toml` (the caller-provided value of `KB_WORKSPACE_ROOT`).
4. `RegistryLocator.user_config` (normally `~/.config/knowledge-infra/sources.toml`, expanded by the caller).

An explicit candidate is authoritative. If it is present in the locator but missing on disk, resolution fails with `KBV2-REGISTRY-NOT-FOUND`; lower tiers are not considered. `env_path` has the same fail-loud rule when no explicit candidate is present.

When neither authoritative candidate is supplied, resolution chooses the first existing workspace or user candidate. A missing workspace candidate may continue to the user candidate. If neither exists, resolution fails with `KBV2-REGISTRY-NOT-FOUND`.

`Registry.origin` is the selected main-registry path. The sibling overlay never replaces `origin`.

A selected main registry or existing overlay is opened once and classified from that opened handle before it is read. A symlink is accepted when its opened target is a regular file. Directories, FIFOs, devices, and other non-regular targets fail with `KBV2-REGISTRY-READ` without being parsed.

`cwd` is not searched and none of its ancestors are searched. There is no built-in `docs` source or any other fallback registry. Once a candidate has been selected, read, parse, schema, overlay, or source-root failures never continue to a lower tier.

## Path resolution and source-root validation

Let `origin_dir` be the parent of the chosen main registry.

- An absolute TOML `workspace_root` remains absolute. A relative TOML `workspace_root` resolves as `origin_dir/workspace_root`.
- If TOML `workspace_root` is absent, the default base is `origin_dir`.
- An absolute source `path` remains absolute. A relative source `path` resolves directly as `origin_dir/path`; it is not relative to `workspace_root`.
- If source `path` is absent, its path is `<base>/<name>`.
- A relative overlay `path` also resolves from `origin_dir`, because the overlay is a machine-local patch to the chosen registry.

After applying the overlay, every source root must canonicalize to an existing directory. `SourceSpec.root` is the resulting absolute canonical path. A missing path, a non-directory, or a canonicalization failure is `KBV2-REGISTRY-SOURCE-ROOT`.

No two logical source names may resolve to the same canonical physical root. This includes lexical aliases containing `.` or `..` and filesystem aliases resolved by canonicalization. The later main-registry row receives `KBV2-REGISTRY-DUPLICATE-ROOT`.

## Local overlay

For a selected registry `<stem>.toml`, the optional sibling overlay is `<stem>.local.toml`. For example, `kb-sources.toml` pairs with `kb-sources.local.toml`, and `sources.toml` pairs with `sources.local.toml`.

An overlay has only `[[source]]` rows. Each row identifies an existing main-registry source by required string `name` and supplies a required string `path`:

```toml
[[source]]
name = "knowledge"
path = "machine-local/knowledge"
```

The overlay may change only `path`. It cannot add a source, rename one, remove one, change `surface`, set a workspace root, or carry any other field. An unknown `name` is `KBV2-REGISTRY-UNKNOWN-OVERLAY`; any forbidden field is `KBV2-REGISTRY-OVERLAY-FIELD`. A malformed overlay is a registry error, not an ignored local preference. Overlay changes are applied before source-root canonicalization and duplicate-root detection.

## Determinism

`Registry.sources` is a `BTreeMap<SourceName, SourceSpec>`. Iteration is therefore by ascending logical source name, independent of TOML row order, filesystem enumeration order, locale, or host platform. Each map key equals its `SourceSpec.name`.

Source indices in diagnostic fields are zero-based main- or overlay-document row indices. A syntax error prevents schema validation of that file, and no partial `Registry` is returned.

## Diagnostic envelope

Every registry diagnostic has `severity: Severity::Error`. Messages below are exact, versioned consumer text: implementations must not append parser output, OS error text, source names, or paths. Machine-specific detail is carried only in `path` and `field`.

Field notation is also versioned:

- top-level field: `workspace_root`, `source`, or the literal unknown field name;
- source-row field: `source[N].name`, `source[N].path`, `source[N].surface`, or the literal row field name in that form;
- `N` is the zero-based row index in the file named by `path`.

| Code | Condition | `path` | `field` | Exact `message` |
|---|---|---|---|---|
| `KBV2-REGISTRY-NOT-FOUND` | supplied explicit path is missing | attempted explicit path | none | `explicit registry does not exist` |
| `KBV2-REGISTRY-NOT-FOUND` | supplied `KB_SOURCES` path is missing | attempted env path | none | `KB_SOURCES registry does not exist` |
| `KBV2-REGISTRY-NOT-FOUND` | neither workspace nor user candidate exists | none | none | `no source registry exists in the configured workspace or user locations` |
| `KBV2-REGISTRY-READ` | selected main registry or existing overlay cannot be read as a regular file | affected file | none | `source registry could not be read` |
| `KBV2-REGISTRY-TOML` | selected main registry or overlay is not valid UTF-8 TOML | affected file | none | `source registry is not valid TOML` |
| `KBV2-REGISTRY-UNKNOWN-FIELD` | main registry contains an unrecognized top-level or source field other than a removed field | main registry | offending field | `registry field is not allowed by source-contract-v2` |
| `KBV2-REGISTRY-REMOVED-FIELD` | main registry or overlay contains `legacy` | affected file | offending `legacy` field | `registry field was removed in source-contract-v2` |
| `KBV2-REGISTRY-MISSING-FIELD` | a required main or overlay row field is absent | affected file | missing field | `required registry field is missing` |
| `KBV2-REGISTRY-WRONG-TYPE` | a known field has the wrong TOML type | affected file | offending field | `registry field has the wrong TOML type` |
| `KBV2-REGISTRY-INVALID-NAME` | a source name fails `^[a-z][a-z0-9-]*$` | affected file | `source[N].name` | `source name must match ^[a-z][a-z0-9-]*$` |
| `KBV2-REGISTRY-DUPLICATE-NAME` | a main-registry name repeats | main registry | later `source[N].name` | `source name is duplicated` |
| `KBV2-REGISTRY-INVALID-SURFACE` | `surface` is neither `core` nor `vertical` | main registry | `source[N].surface` | ``source surface must be `core` or `vertical``` |
| `KBV2-REGISTRY-SOURCE-ROOT` | final source root is missing, not a directory, or cannot be canonicalized | resolved attempted root | `source[N].path` | `source root must be an existing directory` |
| `KBV2-REGISTRY-DUPLICATE-ROOT` | final canonical root repeats | repeated canonical root | later `source[N].path` | `canonical source root is already registered` |
| `KBV2-REGISTRY-UNKNOWN-OVERLAY` | overlay row names no main-registry source | overlay | `source[N].name` | `overlay source does not exist in the main registry` |
| `KBV2-REGISTRY-OVERLAY-FIELD` | overlay contains any field other than `name` and `path`, except removed `legacy` | overlay | offending field | `local overlay may override only source.path` |

The classification of `legacy` takes precedence over generic unknown-field or overlay-field classification. Its code and exact message are always the removed-field contract above.

## Domain, identity, and source discovery

The opaque logical values and source snapshot boundary are:

```rust
pub struct DomainId(String);
pub struct Identity(String);

pub struct SourceContext {
    pub source: SourceSpec,
    pub git_root: PathBuf,
    pub registry_origin: PathBuf,
}

pub struct DomainNode {
    pub id: DomainId,
    pub physical_dir: PathBuf,
    pub index_path: PathBuf,
}

pub struct NoteLocator {
    pub identity: Identity,
    pub domain: DomainId,
    pub slug: String,
    pub path: PathBuf,
    pub is_domain_index: bool,
}

pub struct SnapshotNote {
    pub locator: NoteLocator,
    pub note: ValidatedNote,
}

pub struct SourceSnapshot {
    pub source: SourceName,
    pub domains: Vec<DomainNode>,
    pub notes: Vec<SnapshotNote>,
}

pub fn derive_identity(
    source: &SourceName,
    domain: &DomainId,
    slug: &str,
) -> Result<Identity, Diagnostic>;

pub fn discover_source(
    context: &SourceContext,
) -> Result<SourceSnapshot, Vec<Diagnostic>>;
```

`SourceName`, `DomainId`, and `Identity` expose `as_str`, implement `Display`, and implement `Borrow<str>` for borrowed lookup. `DomainId::new` is the only public direct constructor added here. `Identity` is derive-only; there is no unchecked constructor.

### C2 domains and logical identities

A `DomainId` is one or more non-empty `/`-separated Unicode segments. An exact segment `.` or `..`, `:`, `\`, or any Unicode control character is invalid. Each accepted segment is normalized to NFC and joined with `/`; that NFC spelling is retained for display.

A domain exists only at a non-root directory containing a regular file whose directory-entry name is exactly uppercase `INDEX.md`. Its C2 ID is the `/`-joined sequence of NFC-normalized basenames of only those non-root ancestors that also contain exact `INDEX.md`. Physical ancestors without such a file do not contribute a segment. A source-root `INDEX.md` creates neither an empty domain nor a note and is not parsed.

An ordinary note belongs to its nearest ancestor domain. Its slug is only its filename stem, normalized to NFC. Non-domain subdirectories never enter the slug or identity. A slug is exactly one non-empty segment: `.`, `..`, `/`, `\`, `:`, and Unicode control characters are invalid. Dots inside a longer filename stem are valid.

An identity is exactly `<logical-source>:<C2-domain>:<NFC-filename-stem>`. The first component is `SourceSpec.name`; a Git-root basename, source-root basename, checkout name, and worktree name never contribute. Moving a source root or its containing Git root therefore leaves identities unchanged. Moving a note to a different domain or changing its filename stem changes its identity.

Collision keys are computed from the NFC display value with locale-independent full Unicode case folding, not lowercase conversion or simple case folding. Display values remain NFC and are not replaced by folded keys. Two physical domain directories with the same folded C2 key are a domain collision. Two in-domain notes with the same folded full identity are an identity collision, including equal filename stems below different non-domain physical subdirectories.

### Bounded source walk

`SourceContext.git_root` must canonicalize to an existing directory with an exact `.git` entry that is a file or directory. `SourceSpec.root` is canonicalized again at discovery and may equal the Git root or be strictly below it; canonical containment is required. A lexical child that resolves outside through a source-root symlink is outside the Git root.

Discovery is iterative and bounded by the canonical source root. It never traverses a directory symlink. Before reading directory contents below an entry, it prunes `.git`, `.obsidian`, every dot-prefixed directory, `.venv`, `.legacy-index`, `node_modules`, `target`, and `dist`.

Only exact lowercase-extension `.md` regular files are note candidates. Markdown outside every domain is ignored without reading or frontmatter parsing. Every non-root exact `INDEX.md` is parsed and validated, must declare `kind: index`, and appears exactly once in `SourceSnapshot.notes` with `is_domain_index: true` and slug `INDEX`. Ordinary in-domain notes have `is_domain_index: false`.

`SourceSnapshot.domains` is ordered by `DomainId` and then source-relative physical path. `SourceSnapshot.notes` is ordered by domain and then source-relative physical path. Filesystem enumeration order, physical Git-root name, and registry row order do not affect the result. Discovery returns the complete snapshot or diagnostics, never a partial snapshot.

### Domain, identity, and discovery diagnostics

Every diagnostic below has `severity: Severity::Error`. Messages are exact and never append parser output, OS errors, private paths, source names, or offending text. Collision failures produce one diagnostic at each colliding structural path; for a two-path collision this is exactly two diagnostics ordered by folded logical key and then source-relative path.

| Code | Condition | `path` | `field` | Exact `message` |
|---|---|---|---|---|
| `KBV2-DOMAIN-INVALID` | direct domain input has an invalid segment | none | `domain` | `domain must contain only safe non-empty path segments` |
| `KBV2-DOMAIN-INVALID` | an INDEX-owning basename cannot form a domain segment | affected `INDEX.md` | `domain` | `domain must contain only safe non-empty path segments` |
| `KBV2-IDENTITY-INVALID-SLUG` | direct filename-stem slug input is invalid | none | `slug` | `note slug must be one safe non-empty path segment` |
| `KBV2-IDENTITY-INVALID-SLUG` | an in-domain filename stem is invalid | affected note | `slug` | `note slug must be one safe non-empty path segment` |
| `KBV2-IDENTITY-COLLISION` | two or more notes share one NFC plus full-casefold identity key | each colliding note | none | `note identity is duplicated` |
| `KBV2-SOURCE-GIT-ROOT` | Git root is missing, not a directory, unreadable, or has no exact file/directory `.git` entry | attempted Git root | none | `Git root must be an existing directory containing .git` |
| `KBV2-SOURCE-OUTSIDE-GIT` | canonical source root is outside canonical Git root | canonical source root | none | `source root must be contained by the Git root` |
| `KBV2-SOURCE-READ` | source root, eligible directory entry, domain landing, or in-domain note cannot be read | affected path | none | `source entry could not be read` |
| `KBV2-SOURCE-DUPLICATE-DOMAIN` | two or more INDEX-owning directories share one NFC plus full-casefold C2 key | each colliding `INDEX.md` | none | `domain is duplicated` |
| `KBV2-SOURCE-INDEX-KIND` | a non-root exact `INDEX.md` validates with a kind other than `index` | affected `INDEX.md` | `kind` | ``domain INDEX.md must declare kind `index``` |

## Removed v1 behavior

`source-contract-v2` deliberately has no compatibility lane:

- no `legacy` source flag;
- no current-directory or ancestor registry search;
- no built-in `docs` source;
- no fallback after an authoritative explicit or `KB_SOURCES` candidate is missing;
- no fallback after a selected registry or overlay fails to read, parse, validate, or resolve roots;
- no ignored unknown fields or unknown overlay names.

## Note frontmatter boundary

The note boundary consumes bytes supplied by its caller. It does not read the filesystem and it never writes diagnostics to stdout or stderr.

```rust
pub enum NoteKind {
    Spec,
    Reference,
    Runbook,
    Decision,
    Research,
    Note,
    Index,
}

pub enum NoteStatus {
    Frozen,
}

pub struct NoteFrontmatter {
    pub description: String,
    pub keywords: Vec<String>,
    pub kind: NoteKind,
    pub links: Vec<String>,
    pub code: Vec<String>,
    pub assets: Vec<String>,
    pub supersedes: Option<String>,
    pub status: Option<NoteStatus>,
}

pub struct ValidatedNote {
    pub frontmatter: NoteFrontmatter,
    pub kind_explicit: bool,
    pub body: Vec<u8>,
    pub original: Vec<u8>,
}

pub fn parse_and_validate_note(
    path: &Path,
    bytes: &[u8],
) -> Result<ValidatedNote, Vec<Diagnostic>>;

pub fn render_note(frontmatter: &NoteFrontmatter, body: &[u8]) -> Vec<u8>;
```

### Byte envelope and fences

`parse_and_validate_note` applies these rules in order:

1. The complete input must be UTF-8. An optional UTF-8 BOM (`EF BB BF`) is accepted only at byte offset zero. The BOM is ignored for fence and field parsing.
2. After the optional BOM, the first logical line must be exactly `---`. Leading or trailing spaces, a trailing comment, and an indented fence do not match.
3. A frontmatter line ending may be LF or CRLF. Each physical line may use either accepted ending; a lone CR inside frontmatter is syntax. The first later logical line exactly equal to `---` is the closing fence. A closing fence may be the final bytes of the file.
4. The frontmatter payload is limited to 1 MiB (1,048,576 bytes), counted from the first byte after the opening fence's line ending through the byte immediately before the closing fence. Internal line endings count toward the limit; the opening and closing fence lines and the complete body do not. The scanner must locate a closing fence within that bound before it collects logical-line slices or parses fields. Exceeding the bound fails with `KBV2-NOTE-FRONTMATTER-SYNTAX`; note bodies have no size limit.
5. If there is no opening fence, parsing fails with `KBV2-NOTE-FRONTMATTER-MISSING`. If an opening fence exists, the payload stays within 1 MiB, and no closing fence exists, parsing fails with `KBV2-NOTE-FRONTMATTER-UNTERMINATED`.
6. `ValidatedNote.original` is an exact copy of the caller's complete input, including a BOM, all original line endings, both fences, and the body.
7. `ValidatedNote.body` is an exact copy of every byte after the closing fence's line ending. Therefore a conventional blank separator is the first LF or CRLF in `body`. If the closing fence is at end of file, `body` is empty. Parsing and validation never trim, normalize, inject, or inspect the body. In particular, a body does not need an H1.

The `path` argument is diagnostic identity only. It is copied as supplied, without filesystem access, canonicalization, or normalization.

### Controlled flat grammar

The frontmatter is deliberately not general YAML. Parsing operates on logical frontmatter lines after removing their LF or CRLF terminators. The accepted grammar is:

- A blank line or a comment-only line is accepted. A comment-only line may have leading ASCII space or tab before `#`.
- Every field begins in column one and has the form `<key>:<value>`. A key matches `[A-Za-z][A-Za-z0-9_-]*`. ASCII space and tab around a value are syntactic and are not part of a bare value.
- An inline comment starts at `#` outside quotes when the `#` is the first value character or is preceded by ASCII space or tab. The comment runs to the physical line ending. A `#` inside quotes, or one not preceded by whitespace such as `repo/file#section`, is data.
- A bare scalar is the non-empty text remaining after outer space, tab, and an inline comment are removed. Bare `~` and ASCII-case-insensitive bare `null` are the distinct null value. Other bare tokens, including booleans, numbers, and dates, are strings.
- A single-quoted scalar begins and ends on the same logical line. Two adjacent single quotes inside it decode to one single quote. Backslash has no special meaning. An unmatched quote is syntax.
- A double-quoted scalar begins and ends on the same logical line. It accepts exactly `\\`, `\"`, `\n`, `\r`, `\t`, and `\uXXXX`, where every `X` is an ASCII hexadecimal digit. A non-surrogate `\uXXXX` decodes directly to its Unicode scalar value. A high-surrogate escape (`\uD800` through `\uDBFF`) must be followed immediately by a low-surrogate escape (`\uDC00` through `\uDFFF`); the pair decodes to one supplementary Unicode scalar. Every other backslash escape, an unmatched quote, a lone high or low surrogate, and a high surrogate followed by anything other than the required low-surrogate escape are syntax.
- A flow list is `[` followed by zero or more comma-separated scalar items and `]`. It may span logical lines. Space, tab, line endings, and inline comments outside an item are separators. A leading, trailing, or doubled comma is syntax. Nested lists, mappings, and block scalars are not items.
- A block list starts when a field has no value other than whitespace or an inline comment and its next line is an item. Every item line begins with exactly two ASCII spaces, `-`, and one ASCII space, followed by one bare, single-quoted, or double-quoted scalar and an optional inline comment. Items are consecutive; any other indented content is syntax.
- A block scalar indicator is an otherwise complete value of `|` or `>`, optionally followed by an inline comment. Every non-blank content line begins with exactly two ASCII spaces; those two spaces are removed and any further indentation is data. Blank lines are scalar content. The next non-blank unindented line ends the scalar and must itself be a valid top-level field, comment, or closing fence. Trailing blank content lines and the structural final line ending are discarded.
- Literal `|` joins its remaining content lines with LF. Folded `>` replaces a line ending between adjacent non-empty content lines with one ASCII space; each run of one or more empty content lines becomes one LF. It never adds a terminal LF. These are the complete controlled folding rules; YAML indentation indicators and chomping indicators such as `|2`, `|-`, and `>+` are syntax.
- A field with no scalar and no block-list items is an empty value. Empty is distinct from an explicit null token and from an empty list `[]`.

Decoded strings retain their code points and quoted leading or trailing whitespace. The parser performs no Unicode normalization. Bare-value outer whitespace, grammar indentation, comments, quote delimiters, and escape notation are syntax and are not retained in the decoded value.

A raw U+FEFF code point anywhere inside frontmatter is syntax. The ASCII escape notation `\uFEFF` remains valid and decodes to U+FEFF; the optional raw UTF-8 BOM remains valid only at byte offset zero before the opening fence.

The following constructs are always `KBV2-NOTE-FRONTMATTER-SYNTAX`: non-comment content indented outside a legal continuation; nested mappings or lists; flow mappings; anchors (`&`), aliases (`*`), tags (`!`), directives (`%`), reserved indicators used as an unquoted value; document terminators or additional-document markers (`...`); unconsumed characters after a quoted scalar or closed flow list other than whitespace/comment; malformed continuations; and any line not completely recognized by the grammar. Duplicate keys are the one syntax-family exception and use `KBV2-NOTE-DUPLICATE-FIELD`.

### Fields and validation

Keys are ASCII case-sensitive. The only current fields and semantic types are:

| Field | Presence | Controlled type | Validation and result |
|---|---:|---|---|
| `description` | required | string | Must contain at least one non-whitespace character and no decoded CR or LF. |
| `keywords` | required | list of strings | Must contain at least one item; every item must contain at least one non-whitespace character. |
| `kind` | optional | string | Exact lowercase `spec`, `reference`, `runbook`, `decision`, `research`, `note`, or `index`. Omission yields `NoteKind::Note` and `kind_explicit: false`; presence yields `kind_explicit: true`. |
| `links` | optional | list of strings | Every item must contain at least one non-whitespace character. Omission yields an empty vector. |
| `code` | optional | list of strings | Every item must contain at least one non-whitespace character. Omission yields an empty vector. |
| `assets` | optional, decision only | list of strings | Every item must contain at least one non-whitespace character. Omission yields an empty vector. The field's presence, including `assets: []`, is forbidden unless the resolved `kind` is `decision`. |
| `supersedes` | optional, decision only | string | Must contain at least one non-whitespace character. Omission yields `None`. The field's presence is forbidden unless the resolved `kind` is `decision`. |
| `status` | optional | string | The only value is exact lowercase `frozen`, yielding `Some(NoteStatus::Frozen)`. Omission yields `None`. |

An explicit null is not omission and is the wrong type for every field. The same is true when a scalar field receives a list or a list field receives a scalar. An empty string or whitespace-only string has the correct type but is empty. An empty required list is empty; empty optional lists are valid. A null list item has the wrong type and an empty or whitespace-only string item is empty. Valid decoded strings are stored without trimming.

`assets` and `supersedes` undergo their type and empty-value checks first. `KBV2-NOTE-FIELD-NOT-ALLOWED` is emitted only when the field otherwise has a valid value but the resolved kind is not `decision`. Conditional checks are skipped when `kind` itself is invalid.

These legacy, derived, and projection fields are removed, not generic unknown fields:

- killed v1 fields: `object_id`, `object_key`, `topic`, `workset`, `schema_version`, `updated_at`, `created_at`, `authored_from`, and `retrieval_hint`;
- source-derived fields: `domain`, `title`, `identity`, and `verified`;
- hash/projection fields: `hash` and every key ending in `_hash`, including `source_hash`, `compiled_hash`, `text_hash`, and `content_hash`.

Every removed field produces `KBV2-NOTE-REMOVED-FIELD`, regardless of its value. Every other unrecognized key produces `KBV2-NOTE-UNKNOWN-FIELD`. Removed-field classification takes precedence over unknown-field classification.

After a successful grammar parse, validation reports every independent field error. Present fields are reported in physical occurrence order, list-item errors use ascending zero-based item index, and absent required fields are appended in the order `description`, `keywords`. No partial `ValidatedNote` is returned.

### Note diagnostics

Every note diagnostic has `severity: Severity::Error`, `path` equal to the caller-supplied path, and the exact message below. Messages never include a parser error, line or column number, offending value, body text, or platform-specific detail.

Field notation is versioned:

- a top-level field uses its literal key, such as `description`, `assets`, or an unknown/removed key;
- a list item uses `<field>[N]`, where `N` is its zero-based item index;
- byte-envelope and generic syntax diagnostics have no field.

UTF-8 and fence-envelope failures return exactly one diagnostic and stop. A controlled-grammar failure also returns exactly one diagnostic and prevents schema validation. A repeated key uses the duplicate-field diagnostic instead of the generic syntax diagnostic and names the repeated key.

| Code | Condition | `field` | Exact `message` |
|---|---|---|---|
| `KBV2-NOTE-UTF8` | the complete input is not UTF-8 | none | `note is not valid UTF-8` |
| `KBV2-NOTE-FRONTMATTER-MISSING` | no exact opening fence follows the optional BOM | none | `note must begin with a frontmatter fence` |
| `KBV2-NOTE-FRONTMATTER-UNTERMINATED` | an opening fence has no exact closing fence | none | `frontmatter opening fence has no closing fence` |
| `KBV2-NOTE-FRONTMATTER-SYNTAX` | frontmatter violates the controlled grammar or exceeds its 1 MiB payload limit | none | `frontmatter does not match the controlled flat grammar` |
| `KBV2-NOTE-DUPLICATE-FIELD` | a key repeats | repeated key | `frontmatter field is duplicated` |
| `KBV2-NOTE-MISSING-FIELD` | a required field is absent | missing field | `required note field is missing` |
| `KBV2-NOTE-EMPTY-FIELD` | a correctly typed field or list item is empty | field or item | `note field must not be empty` |
| `KBV2-NOTE-WRONG-TYPE` | a field or list item has the wrong controlled type, including explicit null | field or item | `note field has the wrong type` |
| `KBV2-NOTE-UNKNOWN-FIELD` | an unrecognized field is not in a removed family | offending key | `note field is not allowed by source-contract-v2` |
| `KBV2-NOTE-REMOVED-FIELD` | a killed, derived, or hash/projection field appears | offending key | `note field was removed in source-contract-v2` |
| `KBV2-NOTE-INVALID-KIND` | a string `kind` is outside the seven-value enum | `kind` | `note kind must be spec, reference, runbook, decision, research, note, or index` |
| `KBV2-NOTE-INVALID-STATUS` | a string `status` is not exact `frozen` | `status` | ``note status must be `frozen``` |
| `KBV2-NOTE-FIELD-NOT-ALLOWED` | valid `assets` or `supersedes` appears on a non-decision | offending field | ``note field is allowed only for kind `decision``` |
| `KBV2-NOTE-INVALID-VALUE` | a correctly typed, non-empty value violates another field rule; currently a multiline `description` | offending field | `note field has a value not allowed by source-contract-v2` |

### Canonical renderer

`render_note` serializes its `NoteFrontmatter`; it does not validate it. It has the following byte contract for a UTF-8 `body`:

1. Output is UTF-8 and uses LF only. CRLF and lone CR in `body` become LF.
2. Field order is exactly `description`, `keywords`, `kind`, `links`, `code`, `assets`, `supersedes`, `status`.
3. `description`, `keywords`, and `kind` are always emitted. Thus the enum value `NoteKind::Note` is rendered explicitly as `kind: note`, even when it originated from an omitted field.
4. Empty `links`, `code`, and `assets` vectors and `None` `supersedes` or `status` are omitted. Empty strings inside an emitted vector are serialized as `""`; renderer omission never filters items.
5. `description` is always double quoted. List items and `supersedes` remain bare only when they match ASCII `[A-Za-z][A-Za-z0-9._/-]*` and are not, case-insensitively, `true`, `false`, `yes`, `no`, `on`, `off`, `null`, `none`, `nan`, or `inf`. Empty, reserved, numeric, date-shaped, indicator-leading, whitespace-bearing, flow-special, and non-ASCII/Unicode strings are double quoted.
6. Double-quoted output escapes backslash as `\\`, double quote as `\"`, LF as `\n`, CR as `\r`, and tab as `\t`. Other C0 control characters and DEL use lowercase `\uXXXX` hexadecimal escapes. U+FEFF is always emitted as lowercase `\ufeff`, never as raw `EF BB BF` bytes. Other Unicode scalar values are emitted directly inside the quotes.
7. Lists use flow form with comma-space separators: `[first, second]`. `kind` and `status` use their exact lowercase bare enum spellings.
8. The opening and closing fences are exact `---`. After normalizing line endings, all leading and trailing LF characters are removed from `body`. The renderer emits exactly one blank line between the closing fence and non-empty body content and exactly one trailing LF after the final body content. An empty body renders as a closing-fence line followed by one empty separator line, ending `---\n\n`.
9. The renderer never inserts an H1. It never creates `domain`, `title`, `identity`, `verified`, a killed v1 field, or any hash/projection field. Text with those spellings inside the caller-owned body is preserved as body text.

Because the public renderer cannot return a diagnostic, callers must provide a UTF-8 body. `ValidatedNote.body` always satisfies that precondition. Behavior for an independently supplied non-UTF-8 body is outside this versioned contract.
