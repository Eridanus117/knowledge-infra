use crate::check::{CoreError, HUMAN_INDEX_DRIFT_CODE, HUMAN_INDEX_MARKER_CODE};
use crate::source::{
    SourceSnapshot, open_absolute_dir_nofollow, open_regular_file_for_update_nofollow,
    read_regular_file_nofollow_bounded,
};
use kb_contract::Diagnostic;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

const START_MARKER: &[u8] = b"<!-- rhizome:generated-index:start -->";
const END_MARKER: &[u8] = b"<!-- rhizome:generated-index:end -->";
const MARKERS_MISSING: &str = "human index markers are missing";
const MARKERS_DUPLICATED: &str = "human index markers must occur exactly once";
const MARKERS_REVERSED: &str = "human index end marker must follow start marker";
const DRIFT: &str = "human index generated catalog is out of date";

/// An in-memory, stale-safe replacement for one human index's generated region.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HumanIndexPlan {
    pub path: PathBuf,
    pub replacement: Vec<u8>,
    pub prefix_sha256: String,
    pub suffix_sha256: String,
    source_root: PathBuf,
    target: PathBuf,
}

pub fn check_human_index(
    snapshot: &SourceSnapshot,
    index: &Path,
) -> Result<Vec<Diagnostic>, CoreError> {
    let (canonical_index, _) = validate_index_root(snapshot, index)?;
    let bytes = read_regular_file_nofollow_bounded(&canonical_index, 64 * 1024 * 1024).map_err(
        |source| CoreError::Io {
            path: canonical_index.clone(),
            source,
        },
    )?;
    let marker = match marker_region(&bytes) {
        Ok(region) => region,
        Err(message) => {
            return Ok(vec![
                Diagnostic::error(HUMAN_INDEX_MARKER_CODE, message)
                    .at_path(diagnostic_path(&canonical_index)),
            ]);
        }
    };
    let expected = build_replacement(snapshot, &canonical_index)?;
    if bytes[marker.start..marker.end] != expected {
        return Ok(vec![
            Diagnostic::error(HUMAN_INDEX_DRIFT_CODE, DRIFT)
                .at_path(diagnostic_path(&canonical_index)),
        ]);
    }
    Ok(Vec::new())
}

pub fn plan_human_index(
    snapshot: &SourceSnapshot,
    index: &Path,
) -> Result<HumanIndexPlan, CoreError> {
    let (canonical_index, source_root) = validate_index_root(snapshot, index)?;
    let bytes = read_regular_file_nofollow_bounded(&canonical_index, 64 * 1024 * 1024).map_err(
        |source| CoreError::Io {
            path: canonical_index.clone(),
            source,
        },
    )?;
    let region = marker_region(&bytes).map_err(|message| CoreError::HumanIndex {
        path: canonical_index.clone(),
        message,
    })?;
    let replacement = build_replacement(snapshot, &canonical_index)?;
    Ok(HumanIndexPlan {
        path: canonical_index.clone(),
        replacement,
        prefix_sha256: sha256(&bytes[..region.start]),
        suffix_sha256: sha256(&bytes[region.end..]),
        source_root,
        target: canonical_index,
    })
}
pub fn apply_human_index(plan: HumanIndexPlan) -> Result<(), CoreError> {
    if plan.path != plan.target || plan.path.file_name() != Some(OsStr::new("INDEX.md")) {
        return Err(CoreError::OutsideSourceRoot {
            path: plan.path.clone(),
        });
    }
    let Some(parent) = plan.target.parent() else {
        return Err(CoreError::OutsideSourceRoot {
            path: plan.target.clone(),
        });
    };
    let current_parent = fs::canonicalize(parent).map_err(|source| CoreError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let current_target = fs::canonicalize(&plan.target).map_err(|source| CoreError::Io {
        path: plan.target.clone(),
        source,
    })?;
    if current_parent != plan.source_root || current_target != plan.target {
        return Err(CoreError::ConcurrentModification {
            path: plan.target.clone(),
        });
    }
    if is_index_alias(&plan.target)? {
        return Err(CoreError::OutsideSourceRoot { path: plan.target });
    }
    let directory =
        open_absolute_dir_nofollow(&plan.source_root).map_err(|source| CoreError::Io {
            path: plan.source_root.clone(),
            source,
        })?;
    let mut file = open_regular_file_for_update_nofollow(&directory, OsStr::new("INDEX.md"))
        .map_err(|source| CoreError::Io {
            path: plan.target.clone(),
            source,
        })?
        .into_std();
    file.lock().map_err(|source| CoreError::Io {
        path: plan.target.clone(),
        source,
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| CoreError::Io {
            path: plan.target.clone(),
            source,
        })?;
    let region = marker_region(&bytes).map_err(|message| CoreError::HumanIndex {
        path: plan.target.clone(),
        message,
    })?;
    if sha256(&bytes[..region.start]) != plan.prefix_sha256
        || sha256(&bytes[region.end..]) != plan.suffix_sha256
    {
        return Err(CoreError::ConcurrentModification {
            path: plan.target.clone(),
        });
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|source| CoreError::Io {
            path: plan.target.clone(),
            source,
        })?;
    let mut latest = Vec::new();
    file.read_to_end(&mut latest)
        .map_err(|source| CoreError::Io {
            path: plan.target.clone(),
            source,
        })?;
    let latest_region = marker_region(&latest).map_err(|message| CoreError::HumanIndex {
        path: plan.target.clone(),
        message,
    })?;
    if sha256(&latest[..latest_region.start]) != plan.prefix_sha256
        || sha256(&latest[latest_region.end..]) != plan.suffix_sha256
    {
        return Err(CoreError::ConcurrentModification {
            path: plan.target.clone(),
        });
    }
    let bytes = latest;
    let region = latest_region;
    let mut output =
        Vec::with_capacity(bytes.len() - (region.end - region.start) + plan.replacement.len());
    output.extend_from_slice(&bytes[..region.start]);
    output.extend_from_slice(&plan.replacement);
    output.extend_from_slice(&bytes[region.end..]);
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.set_len(0))
        .and_then(|_| file.write_all(&output))
        .and_then(|_| file.sync_all())
        .map_err(|source| CoreError::Io {
            path: plan.target,
            source,
        })
}
fn lexical_absolute(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(name) => normalized.push(name),
        }
    }
    Ok(normalized)
}

fn validate_index_root(
    snapshot: &SourceSnapshot,
    index: &Path,
) -> Result<(PathBuf, PathBuf), CoreError> {
    if index.file_name() != Some(OsStr::new("INDEX.md")) {
        return Err(CoreError::OutsideSourceRoot {
            path: index.to_path_buf(),
        });
    }
    let Some(raw_parent) = index.parent() else {
        return Err(CoreError::OutsideSourceRoot {
            path: index.to_path_buf(),
        });
    };
    let lexical_parent = lexical_absolute(raw_parent).map_err(|source| CoreError::Io {
        path: raw_parent.to_path_buf(),
        source,
    })?;
    open_absolute_dir_nofollow(&lexical_parent).map_err(|source| CoreError::Io {
        path: raw_parent.to_path_buf(),
        source,
    })?;
    let canonical_index = fs::canonicalize(index).map_err(|source| CoreError::Io {
        path: index.to_path_buf(),
        source,
    })?;
    let expected_index =
        fs::canonicalize(snapshot.source_root.join("INDEX.md")).map_err(|source| {
            CoreError::Io {
                path: snapshot.source_root.join("INDEX.md"),
                source,
            }
        })?;
    if canonical_index != expected_index
        || fs::symlink_metadata(index)
            .map_err(|source| CoreError::Io {
                path: index.to_path_buf(),
                source,
            })?
            .file_type()
            .is_symlink()
        || is_index_alias(&canonical_index)?
    {
        return Err(CoreError::OutsideSourceRoot {
            path: index.to_path_buf(),
        });
    }
    let Some(parent) = canonical_index.parent().map(Path::to_path_buf) else {
        return Err(CoreError::OutsideSourceRoot {
            path: index.to_path_buf(),
        });
    };
    if parent != snapshot.source_root {
        return Err(CoreError::OutsideSourceRoot {
            path: index.to_path_buf(),
        });
    }
    Ok((canonical_index, parent))
}

fn is_index_alias(index: &Path) -> Result<bool, CoreError> {
    let metadata = fs::symlink_metadata(index).map_err(|source| CoreError::Io {
        path: index.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Ok(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return Ok(metadata.nlink() > 1);
    }
    #[cfg(windows)]
    {
        let file = fs::File::open(index).map_err(|source| CoreError::Io {
            path: index.to_path_buf(),
            source,
        })?;
        let information =
            winapi_util::file::information(&file).map_err(|source| CoreError::Io {
                path: index.to_path_buf(),
                source,
            })?;
        return Ok(information.number_of_links() > 1);
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(true)
    }
}

#[derive(Clone, Copy)]
struct MarkerRegion {
    start: usize,
    end: usize,
}

fn marker_region(bytes: &[u8]) -> Result<MarkerRegion, &'static str> {
    let starts = find_all(bytes, START_MARKER);
    let ends = find_all(bytes, END_MARKER);
    if starts.is_empty() || ends.is_empty() {
        return Err(MARKERS_MISSING);
    }
    if starts.len() != 1 || ends.len() != 1 {
        return Err(MARKERS_DUPLICATED);
    }
    if ends[0] < starts[0] {
        return Err(MARKERS_REVERSED);
    }
    Ok(MarkerRegion {
        start: starts[0],
        end: ends[0] + END_MARKER.len(),
    })
}

fn find_all(bytes: &[u8], needle: &[u8]) -> Vec<usize> {
    bytes
        .windows(needle.len())
        .enumerate()
        .filter_map(|(position, window)| {
            let at_line_start = position == 0 || bytes[position - 1] == b'\n';
            let after = position + needle.len();
            let at_line_end = after == bytes.len()
                || bytes[after] == b'\n'
                || (bytes[after] == b'\r' && after + 1 < bytes.len() && bytes[after + 1] == b'\n');
            (at_line_start && at_line_end && window == needle).then_some(position)
        })
        .collect()
}

fn build_replacement(snapshot: &SourceSnapshot, index: &Path) -> Result<Vec<u8>, CoreError> {
    let (_, source_root) = validate_index_root(snapshot, index)?;
    let mut notes = snapshot
        .notes
        .iter()
        .filter(|note| !note.locator.is_domain_index)
        .collect::<Vec<_>>();
    notes.sort_by_cached_key(|note| {
        (
            note.locator.domain.as_str().to_owned(),
            relative_posix(&source_root, &note.locator.path),
        )
    });
    let mut output = Vec::new();
    output.extend_from_slice(START_MARKER);
    let mut domain = None::<&str>;
    for note in notes {
        let current = note.locator.domain.as_str();
        if domain != Some(current) {
            output.push(b'\n');
            output.extend_from_slice(b"### ");
            output.extend_from_slice(current.as_bytes());
            domain = Some(current);
        }
        output.push(b'\n');
        output.extend_from_slice(b"- [");
        output.extend_from_slice(note.locator.identity.as_str().as_bytes());
        output.extend_from_slice(b"](");
        output.extend_from_slice(relative_posix(&source_root, &note.locator.path).as_bytes());
        output.extend_from_slice(b") \xE2\x80\x94 ");
        output.extend_from_slice(note.note.frontmatter.description.as_bytes());
    }
    output.push(b'\n');
    output.extend_from_slice(END_MARKER);
    Ok(output)
}

fn relative_posix(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn diagnostic_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\")
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}
