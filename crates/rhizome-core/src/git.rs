use sha2::{Digest, Sha256};
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[derive(Debug)]
pub enum GitError {
    InvalidRoot(PathBuf),
    InvalidPath(PathBuf),
    CommandFailed(&'static str),
    InvalidUtf8(&'static str),
    NotFound(PathBuf),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}
impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRoot(_) => f.write_str("Git root is invalid"),
            Self::InvalidPath(_) => f.write_str("path is outside the Git root"),
            Self::CommandFailed(command) => write!(f, "git {command} failed"),
            Self::InvalidUtf8(command) => write!(f, "git {command} returned invalid UTF-8"),
            Self::NotFound(_) => f.write_str("Git object was not found"),
            Self::Io { .. } => f.write_str("could not access a Git path"),
        }
    }
}
impl std::error::Error for GitError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitBackend {
    pub(crate) root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StagedChange {
    pub status: String,
    pub old_path: String,
    pub new_path: Option<String>,
}

impl GitBackend {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, GitError> {
        let input = root.into();
        let root = fs::canonicalize(&input).map_err(|_| GitError::InvalidRoot(input.clone()))?;
        if !root.is_dir() {
            return Err(GitError::InvalidRoot(root));
        }
        let backend = Self { root };
        let reported = backend.run_text(&["rev-parse", "--show-toplevel"], None)?;
        let reported = fs::canonicalize(PathBuf::from(reported.trim()))
            .map_err(|_| GitError::InvalidRoot(backend.root.clone()))?;
        if reported != backend.root {
            return Err(GitError::InvalidRoot(backend.root));
        }
        Ok(backend)
    }
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn head_oid(&self) -> Result<String, GitError> {
        Ok(self
            .run_text(&["rev-parse", "HEAD"], None)?
            .trim()
            .to_owned())
    }

    pub fn head_blob(&self, path: &Path) -> Result<Vec<u8>, GitError> {
        let spec = format!("HEAD:{}", self.relative_path(path)?);
        let output = self.run(&["show", &spec], None)?;
        if !output.status.success() {
            return Err(GitError::NotFound(path.to_path_buf()));
        }
        Ok(output.stdout)
    }
    pub fn head_blob_sha256(&self, path: &Path) -> Result<String, GitError> {
        Ok(Self::canonical_blob_sha256(&self.head_blob(path)?))
    }
    #[must_use]
    pub fn canonical_blob_sha256(bytes: &[u8]) -> String {
        let header = format!("blob {}\0", bytes.len());
        let mut hasher = Sha256::new();
        hasher.update(header.as_bytes());
        hasher.update(bytes);
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    pub(crate) fn relative_path(&self, path: &Path) -> Result<String, GitError> {
        let input = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let absolute = normalize_path(&input);
        let relative = absolute
            .strip_prefix(&self.root)
            .map_err(|_| GitError::InvalidPath(path.to_path_buf()))?;
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(GitError::InvalidPath(path.to_path_buf()));
        }
        let mut text = String::new();
        for (i, component) in relative.components().enumerate() {
            if i > 0 {
                text.push('/');
            }
            let part = component
                .as_os_str()
                .to_str()
                .ok_or(GitError::InvalidPath(path.to_path_buf()))?;
            if part.is_empty() || part == "." || part == ".." {
                return Err(GitError::InvalidPath(path.to_path_buf()));
            }
            text.push_str(part);
        }
        Ok(text)
    }
    pub(crate) fn absolute_path(&self, relative: &str) -> PathBuf {
        self.root
            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
    }
    pub(crate) fn worktree_bytes(&self, path: &Path) -> Result<Vec<u8>, GitError> {
        fs::read(path).map_err(|source| GitError::Io {
            path: path.to_path_buf(),
            source,
        })
    }
    pub(crate) fn worktree_exact_matches_head(&self, path: &Path) -> Result<bool, GitError> {
        Ok(self.worktree_bytes(path)? == self.head_blob(path)?)
    }
    /// Compare raw bytes. Git's text filters are deliberately not consulted;
    /// a CRLF materialization is accepted only when every CRLF pair maps exactly
    /// to the committed LF blob and there are no lone carriage returns.
    pub(crate) fn worktree_matches_head(&self, path: &Path) -> Result<bool, GitError> {
        let raw = self.worktree_bytes(path)?;
        let head = self.head_blob(path)?;
        if raw == head {
            return Ok(true);
        }
        let mut normalized = Vec::with_capacity(raw.len());
        let mut index = 0;
        while index < raw.len() {
            if raw[index] == b'\r' {
                if index + 1 >= raw.len() || raw[index + 1] != b'\n' {
                    return Ok(false);
                }
                index += 1;
            }
            normalized.push(raw[index]);
            index += 1;
        }
        Ok(normalized == head && raw.contains(&b'\r'))
    }
    pub(crate) fn canonical_blob_bytes_for_path(
        &self,
        path: &Path,
        bytes: &[u8],
    ) -> Result<Vec<u8>, GitError> {
        let relative = self.relative_path(path)?;
        let attributes_path = self.absolute_path(".gitattributes");
        let attributes = match self.head_blob(&attributes_path) {
            Ok(bytes) => bytes,
            Err(GitError::NotFound(_)) => fs::read(&attributes_path).unwrap_or_default(),
            Err(error) => return Err(error),
        };
        if !attributes_require_lf(&attributes, &relative) {
            return Ok(bytes.to_vec());
        }
        let mut normalized = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'\r' {
                if index + 1 >= bytes.len() || bytes[index + 1] != b'\n' {
                    return Ok(bytes.to_vec());
                }
                index += 1;
            }
            normalized.push(bytes[index]);
            index += 1;
        }
        Ok(normalized)
    }

    pub(crate) fn staged_status(&self) -> Result<Vec<StagedChange>, GitError> {
        let output = self.run(&["diff", "--cached", "--name-status", "-z"], None)?;
        if !output.status.success() {
            return Err(GitError::CommandFailed("diff"));
        }
        parse_staged_status(&output.stdout)
    }
    pub fn is_staged_deletion(&self, path: &Path) -> Result<bool, GitError> {
        let relative = self.relative_path(path)?;
        Ok(self.staged_status()?.iter().any(|change| {
            matches!(change.status.as_bytes().first(), Some(b'D' | b'R' | b'C'))
                && change.old_path == relative
        }))
    }
    pub(crate) fn attributes_clean(&self) -> bool {
        let path = self.absolute_path(".gitattributes");
        self.head_blob(&path).ok() == self.worktree_bytes(&path).ok()
    }
    pub(crate) fn staged_blob(&self, path: &Path) -> Result<Vec<u8>, GitError> {
        let spec = format!(":{}", self.relative_path(path)?);
        let output = self.run(&["show", &spec], None)?;
        if !output.status.success() {
            return Err(GitError::NotFound(path.to_path_buf()));
        }
        Ok(output.stdout)
    }
    pub(crate) fn staged_regular(&self, path: &Path) -> Result<bool, GitError> {
        let rel = self.relative_path(path)?;
        let output = self.run(
            &["--literal-pathspecs", "ls-files", "--stage", "--", &rel],
            None,
        )?;
        if !output.status.success() {
            return Err(GitError::CommandFailed("ls-files"));
        }
        let text =
            String::from_utf8(output.stdout).map_err(|_| GitError::InvalidUtf8("ls-files"))?;
        let mode = text.split_whitespace().next().unwrap_or("");
        Ok(mode == "100644" || mode == "100755")
    }
    pub(crate) fn head_regular(&self, path: &Path) -> Result<bool, GitError> {
        let rel = self.relative_path(path)?;
        let output = self.run(
            &[
                "--literal-pathspecs",
                "ls-tree",
                "--full-tree",
                "HEAD",
                "--",
                &rel,
            ],
            None,
        )?;
        if !output.status.success() {
            return Err(GitError::CommandFailed("ls-tree"));
        }
        let mode = String::from_utf8(output.stdout)
            .map_err(|_| GitError::InvalidUtf8("ls-tree"))?
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_owned();
        Ok(mode == "100644" || mode == "100755")
    }
    pub(crate) fn add(&self, paths: &[String]) -> Result<(), GitError> {
        self.add_with_flag(paths, false)
    }
    pub(crate) fn add_all(&self, paths: &[String]) -> Result<(), GitError> {
        self.add_with_flag(paths, true)
    }
    fn add_with_flag(&self, paths: &[String], all: bool) -> Result<(), GitError> {
        let mut args = vec!["--literal-pathspecs".to_owned(), "add".to_owned()];
        if all {
            args.push("--all".to_owned());
        }
        args.push("--".to_owned());
        args.extend(paths.iter().cloned());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = self.run(&refs, None)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(GitError::CommandFailed("add"))
        }
    }
    pub(crate) fn write_regular_nosymlink(path: &Path, bytes: &[u8]) -> Result<(), GitError> {
        crate::source::write_regular_file_nofollow(path, bytes).map_err(|source| GitError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    pub(crate) fn commit(&self, subject: &str, reason: &str) -> Result<(), GitError> {
        let message = format!("{subject}\n\nFrozen-Amend-Approved: {reason}");
        let output = self.run(&["commit", "-m", &message], None)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(GitError::CommandFailed("commit"))
        }
    }
    pub(crate) fn reset_hard(&self, oid: &str) -> Result<(), GitError> {
        let output = self.run(&["reset", "--hard", oid], None)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(GitError::CommandFailed("reset"))
        }
    }
    pub(crate) fn reset_index(&self) -> Result<(), GitError> {
        let output = self.run(&["reset", "--mixed", "HEAD"], None)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(GitError::CommandFailed("reset"))
        }
    }
    pub(crate) fn commit_message(&self) -> Result<String, GitError> {
        self.run_text(&["show", "-s", "--format=%B", "HEAD"], None)
    }
    pub(crate) fn commit_changes(&self) -> Result<Vec<StagedChange>, GitError> {
        let output = self.run(&["show", "--format=", "--name-status", "-z", "HEAD"], None)?;
        if !output.status.success() {
            return Err(GitError::CommandFailed("show"));
        }
        parse_staged_status(&output.stdout)
    }
    pub(crate) fn worktree_status(&self) -> Result<Vec<u8>, GitError> {
        let output = self.run(&["status", "--porcelain=v1", "-z"], None)?;
        if !output.status.success() {
            return Err(GitError::CommandFailed("status"));
        }
        Ok(output.stdout)
    }
    pub(crate) fn run(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<Output, GitError> {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1");
        if let Some(bytes) = stdin {
            command.stdin(Stdio::piped());
            let mut child = command
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|source| GitError::Io {
                    path: self.root.clone(),
                    source,
                })?;
            child
                .stdin
                .take()
                .expect("piped stdin")
                .write_all(bytes)
                .map_err(|source| GitError::Io {
                    path: self.root.clone(),
                    source,
                })?;
            return child.wait_with_output().map_err(|source| GitError::Io {
                path: self.root.clone(),
                source,
            });
        }
        command.output().map_err(|source| GitError::Io {
            path: self.root.clone(),
            source,
        })
    }
    fn run_text(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, GitError> {
        let output = self.run(args, stdin)?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(command_name(args)));
        }
        String::from_utf8(output.stdout).map_err(|_| GitError::InvalidUtf8(command_name(args)))
    }
}

fn parse_staged_status(bytes: &[u8]) -> Result<Vec<StagedChange>, GitError> {
    let mut fields = bytes.split(|byte| *byte == 0);
    let mut changes = Vec::new();
    while let Some(status) = fields.next() {
        if status.is_empty() {
            break;
        }
        let status = std::str::from_utf8(status).map_err(|_| GitError::InvalidUtf8("diff"))?;
        let old = fields.next().ok_or(GitError::CommandFailed("diff"))?;
        let old = std::str::from_utf8(old)
            .map_err(|_| GitError::InvalidUtf8("diff"))?
            .to_owned();
        let new_path = if status.starts_with('R') || status.starts_with('C') {
            let new = fields.next().ok_or(GitError::CommandFailed("diff"))?;
            Some(
                std::str::from_utf8(new)
                    .map_err(|_| GitError::InvalidUtf8("diff"))?
                    .to_owned(),
            )
        } else {
            None
        };
        changes.push(StagedChange {
            status: status.to_owned(),
            old_path: old,
            new_path,
        });
    }
    Ok(changes)
}
fn attributes_require_lf(attributes: &[u8], relative: &str) -> bool {
    attributes_mode(attributes, relative) == Some(true)
}
fn attributes_mode(attributes: &[u8], relative: &str) -> Option<bool> {
    let Ok(text) = std::str::from_utf8(attributes) else {
        return None;
    };
    let mut mode = None;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(pattern) = fields.next() else {
            continue;
        };
        if pattern.starts_with('#') || !attribute_pattern_matches(pattern, relative) {
            continue;
        }
        for attr in fields {
            match attr {
                "eol=lf" => mode = Some(true),
                "eol=crlf" | "-crlf" | "-text" => mode = Some(false),
                _ => {}
            }
        }
    }
    mode
}
fn attribute_pattern_matches(pattern: &str, relative: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let candidate = if pattern.contains('/') {
        relative
    } else {
        relative.rsplit('/').next().unwrap_or(relative)
    };
    glob_matches(pattern, candidate)
}
fn glob_matches(pattern: &str, value: &str) -> bool {
    let (mut p, mut v, mut star, mut checkpoint) = (0usize, 0usize, None, 0usize);
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    while v < value.len() {
        if p < pattern.len() && pattern[p] == value[v] {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            checkpoint = v;
        } else if let Some(index) = star {
            p = index + 1;
            checkpoint += 1;
            v = checkpoint;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}
fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(path) = fs::canonicalize(path) {
        return path;
    }
    let Some(parent) = path.parent() else {
        return path.to_path_buf();
    };
    fs::canonicalize(parent)
        .map(|parent| parent.join(path.file_name().unwrap_or_default()))
        .unwrap_or_else(|_| path.to_path_buf())
}
fn command_name(args: &[&str]) -> &'static str {
    match args.first().copied() {
        Some("rev-parse") => "rev-parse",
        Some("show") => "show",
        _ => "git",
    }
}
