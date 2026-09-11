use crate::source::{create_directory_tree_nofollow, open_regular_file_for_append_nofollow};
use kb_contract::Diagnostic;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

const EMPTY: &str = "KBV2-CAPTURE-EMPTY";
const EMPTY_MESSAGE: &str = "capture text must not be empty";
const INVALID_TIMESTAMP: &str = "KBV2-CAPTURE-TIMESTAMP";
const INVALID_TIMESTAMP_MESSAGE: &str = "capture timestamp must be one line";

/// Input to a raw inbox capture. Capture deliberately has no source or domain context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureRequest {
    pub inbox: PathBuf,
    pub text: String,
    pub timestamp: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturePlan {
    path: PathBuf,
    line: Vec<u8>,
}

impl CapturePlan {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn line(&self) -> &[u8] {
        &self.line
    }
}

#[derive(Debug)]
pub enum CaptureError {
    Diagnostics(Vec<Diagnostic>),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Diagnostics(diagnostics) => match diagnostics.first() {
                Some(diagnostic) => formatter.write_str(&diagnostic.message),
                None => formatter.write_str("capture failed"),
            },
            Self::Io { path, .. } => write!(
                formatter,
                "could not write capture inbox {}",
                path.display()
            ),
        }
    }
}
impl std::error::Error for CaptureError {}
impl CaptureError {
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        match self {
            Self::Diagnostics(diagnostics) => diagnostics,
            Self::Io { path, .. } => vec![
                Diagnostic::error("KBV2-CAPTURE-IO", "could not write capture inbox").at_path(path),
            ],
        }
    }
}

/// Collapse all Unicode whitespace and prepare exactly one inbox line.
pub fn plan_capture(request: &CaptureRequest) -> Result<CapturePlan, CaptureError> {
    let text = request
        .text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        return Err(CaptureError::Diagnostics(vec![
            Diagnostic::error(EMPTY, EMPTY_MESSAGE).for_field("text"),
        ]));
    }
    if request.timestamp.is_empty() || request.timestamp.chars().any(char::is_control) {
        return Err(CaptureError::Diagnostics(vec![
            Diagnostic::error(INVALID_TIMESTAMP, INVALID_TIMESTAMP_MESSAGE).for_field("timestamp"),
        ]));
    }
    let path = if request.inbox.is_absolute() {
        request.inbox.clone()
    } else {
        std::path::absolute(&request.inbox).map_err(|source| CaptureError::Io {
            path: request.inbox.clone(),
            source,
        })?
    };
    let line = format!("- {} {}\n", request.timestamp, text).into_bytes();
    Ok(CapturePlan { path, line })
}

/// Append a capture using no-follow file access; parent directories are created as needed.
pub fn apply_capture(plan: &CapturePlan) -> Result<(), CaptureError> {
    let parent = plan.path.parent().ok_or_else(|| {
        CaptureError::Diagnostics(vec![
            Diagnostic::error(INVALID_TIMESTAMP, INVALID_TIMESTAMP_MESSAGE).for_field("inbox"),
        ])
    })?;
    create_directory_tree_nofollow(parent).map_err(|source| CaptureError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let mut file =
        open_regular_file_for_append_nofollow(&plan.path).map_err(|source| CaptureError::Io {
            path: plan.path.clone(),
            source,
        })?;
    file.write_all(&plan.line)
        .and_then(|_| file.sync_all())
        .map_err(|source| CaptureError::Io {
            path: plan.path.clone(),
            source,
        })
}
