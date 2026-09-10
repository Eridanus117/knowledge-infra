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

## Shared source-path diagnostics

The existing `SourceRoot` and `SourcePath` boundaries also return `Diagnostic` directly. Their acceptance and path-normalization behavior is unchanged; their machine codes are versioned for v2. These diagnostics have `severity: Severity::Error` and no `field`.

| Code | Condition |
|---|---|
| `KBV2-SOURCE-ROOT-EMPTY` | The source-root input is empty. |
| `KBV2-SOURCE-ROOT-RELATIVE` | The source-root input is not absolute. |
| `KBV2-SOURCE-ROOT-MISSING` | The absolute source root does not exist. |
| `KBV2-SOURCE-ROOT-UNAVAILABLE` | Metadata for the absolute source root cannot be read for another reason. |
| `KBV2-SOURCE-ROOT-NOT-DIRECTORY` | The absolute source root is not a directory. |
| `KBV2-SOURCE-PATH-INVALID` | A source-relative path is empty, absolute, or contains `.`, `..`, a root, or a platform prefix. |

## Removed v1 behavior

`source-contract-v2` deliberately has no compatibility lane:

- no `legacy` source flag;
- no current-directory or ancestor registry search;
- no built-in `docs` source;
- no fallback after an authoritative explicit or `KB_SOURCES` candidate is missing;
- no fallback after a selected registry or overlay fails to read, parse, validate, or resolve roots;
- no ignored unknown fields or unknown overlay names.
