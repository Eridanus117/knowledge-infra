use crate::{Diagnostic, DomainId, SourceName};
use std::borrow::Borrow;
use std::fmt;
use unicode_normalization::UnicodeNormalization;

const INVALID_SLUG: &str = "KBV2-IDENTITY-INVALID-SLUG";
const INVALID_SLUG_MESSAGE: &str = "note slug must be one safe non-empty path segment";

/// A logical source, C2 domain, and filename-stem identity in NFC display form.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Identity(String);

impl Identity {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for Identity {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Derive a position-owned note identity from validated logical values.
pub fn derive_identity(
    source: &SourceName,
    domain: &DomainId,
    slug: &str,
) -> Result<Identity, Diagnostic> {
    if !valid_slug(slug) {
        return Err(Diagnostic::error(INVALID_SLUG, INVALID_SLUG_MESSAGE).for_field("slug"));
    }

    let mut identity =
        String::with_capacity(source.as_str().len() + domain.as_str().len() + slug.len() + 2);
    identity.push_str(source.as_str());
    identity.push(':');
    identity.push_str(domain.as_str());
    identity.push(':');
    identity.extend(slug.nfc());
    Ok(Identity(identity))
}

fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug != "."
        && slug != ".."
        && !slug.chars().any(|character| {
            character == ':' || character == '/' || character == '\\' || character.is_control()
        })
}
