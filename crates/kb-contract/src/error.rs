use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

/// A structured failure at the source-contract boundary.
///
/// Codes are stable machine identifiers. Paths and fields remain optional so
/// configuration-wide failures can use the same transport as note failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractError {
    code: &'static str,
    path: Option<PathBuf>,
    field: Option<String>,
    message: String,
}

impl ContractError {
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            path: None,
            field: None,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn at_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    #[must_use]
    pub fn for_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    #[must_use]
    pub fn field(&self) -> Option<&str> {
        self.field.as_deref()
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ContractError {}
