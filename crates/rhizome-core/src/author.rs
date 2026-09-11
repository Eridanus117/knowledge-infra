use crate::source::{
    SourceContext, create_regular_file_nofollow, discover_source, open_absolute_dir_nofollow,
};
use kb_contract::{
    Diagnostic, DomainId, Identity, NoteFrontmatter, derive_identity, parse_and_validate_note,
    render_note,
};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

const DOMAIN_MISSING: &str = "KBV2-AUTHOR-DOMAIN-MISSING";
const DOMAIN_MISSING_MESSAGE: &str = "author domain must contain an exact INDEX.md";
const BODY_EMPTY: &str = "KBV2-AUTHOR-BODY-EMPTY";
const BODY_EMPTY_MESSAGE: &str = "authored note body must not be empty";
const BODY_UTF8: &str = "KBV2-AUTHOR-BODY-UTF8";
const BODY_UTF8_MESSAGE: &str = "authored note body must be valid UTF-8";
const DESTINATION_EXISTS: &str = "KBV2-AUTHOR-DESTINATION-EXISTS";
const DESTINATION_EXISTS_MESSAGE: &str = "authored note destination already exists";

/// Input to a side-effect-free authoring plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorRequest {
    pub context: SourceContext,
    pub domain: DomainId,
    pub slug: String,
    pub frontmatter: NoteFrontmatter,
    pub body: Vec<u8>,
}

/// An authoring plan captures canonical destination bytes and never overwrites an existing file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorPlan {
    path: PathBuf,
    identity: Identity,
    bytes: Vec<u8>,
}

impl AuthorPlan {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
}

#[derive(Debug)]
pub enum AuthorError {
    Diagnostics(Vec<Diagnostic>),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for AuthorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Diagnostics(diagnostics) => match diagnostics.first() {
                Some(diagnostic) => formatter.write_str(&diagnostic.message),
                None => formatter.write_str("authoring failed"),
            },
            Self::Io { path, .. } => write!(
                formatter,
                "could not write authored note {}",
                path.display()
            ),
        }
    }
}
impl std::error::Error for AuthorError {}

/// Validate a logical destination and render one canonical source-contract note.
impl AuthorError {
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        match self {
            Self::Diagnostics(diagnostics) => diagnostics,
            Self::Io { path, .. } => vec![
                Diagnostic::error("KBV2-AUTHOR-IO", "could not write authored note").at_path(path),
            ],
        }
    }
}
pub fn plan_author(request: &AuthorRequest) -> Result<AuthorPlan, AuthorError> {
    let body = std::str::from_utf8(&request.body)
        .map_err(|_| diagnostic(BODY_UTF8, BODY_UTF8_MESSAGE, "body"))?;
    if body.trim().is_empty() {
        return Err(diagnostic(BODY_EMPTY, BODY_EMPTY_MESSAGE, "body"));
    }

    let snapshot = discover_source(&request.context).map_err(AuthorError::Diagnostics)?;
    let Some(domain_node) = snapshot
        .domains
        .iter()
        .find(|node| node.id == request.domain)
    else {
        return Err(diagnostic(DOMAIN_MISSING, DOMAIN_MISSING_MESSAGE, "domain"));
    };
    let domain_dir = domain_node.physical_dir.clone();

    let normalized_slug = request.slug.nfc().collect::<String>();
    let identity = derive_identity(
        &request.context.source.name,
        &request.domain,
        &normalized_slug,
    )
    .map_err(|diagnostic| AuthorError::Diagnostics(vec![diagnostic]))?;
    let path = domain_dir.join(format!("{normalized_slug}.md"));
    if path.exists() {
        return Err(diagnostic(
            DESTINATION_EXISTS,
            DESTINATION_EXISTS_MESSAGE,
            "slug",
        ));
    }
    open_absolute_dir_nofollow(&domain_dir).map_err(|source| AuthorError::Io {
        path: domain_dir.clone(),
        source,
    })?;

    let bytes = render_note(&request.frontmatter, &request.body);
    parse_and_validate_note(&path, &bytes).map_err(AuthorError::Diagnostics)?;
    Ok(AuthorPlan {
        path,
        identity,
        bytes,
    })
}

/// Apply an authoring plan with no-follow create-new semantics.
pub fn apply_author(plan: &AuthorPlan) -> Result<(), AuthorError> {
    let mut file = create_regular_file_nofollow(&plan.path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::AlreadyExists {
            diagnostic(DESTINATION_EXISTS, DESTINATION_EXISTS_MESSAGE, "slug")
        } else {
            AuthorError::Io {
                path: plan.path.clone(),
                source,
            }
        }
    })?;
    file.write_all(&plan.bytes)
        .and_then(|_| file.sync_all())
        .map_err(|source| AuthorError::Io {
            path: plan.path.clone(),
            source,
        })
}

fn diagnostic(code: &'static str, message: &'static str, field: &'static str) -> AuthorError {
    AuthorError::Diagnostics(vec![Diagnostic::error(code, message).for_field(field)])
}
