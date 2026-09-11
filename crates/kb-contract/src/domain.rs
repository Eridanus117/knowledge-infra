use crate::Diagnostic;
use std::borrow::Borrow;
use std::fmt;
use unicode_normalization::UnicodeNormalization;

const INVALID: &str = "KBV2-DOMAIN-INVALID";
const INVALID_MESSAGE: &str = "domain must contain only safe non-empty path segments";

/// A validated C2 domain identifier in NFC display form.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DomainId(String);

impl DomainId {
    pub fn new(value: &str) -> Result<Self, Diagnostic> {
        if value.is_empty() {
            return Err(invalid_domain());
        }

        let mut normalized = String::with_capacity(value.len());
        for (position, segment) in value.split('/').enumerate() {
            if !valid_segment(segment) {
                return Err(invalid_domain());
            }
            if position != 0 {
                normalized.push('/');
            }
            normalized.extend(segment.nfc());
        }

        Ok(Self(normalized))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for DomainId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for DomainId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn valid_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment
            .chars()
            .any(|character| character == ':' || character == '\\' || character.is_control())
}

fn invalid_domain() -> Diagnostic {
    Diagnostic::error(INVALID, INVALID_MESSAGE).for_field("domain")
}
