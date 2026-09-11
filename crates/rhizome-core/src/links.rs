use crate::source::{SourceContext, SourceSnapshot};
use kb_contract::{Diagnostic, NoteStatus, Severity, derive_identity};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

const BROKEN_LINK: &str = "KBV2-LINK-BROKEN";
const BROKEN_CODE: &str = "KBV2-CODE-BROKEN";
const BROKEN_LINK_MESSAGE: &str = "link target does not resolve: ";
const BROKEN_CODE_MESSAGE: &str = "code pointer does not resolve from the Git root";

pub fn check_links_and_code(snapshot: &SourceSnapshot, context: &SourceContext) -> Vec<Diagnostic> {
    let identities = snapshot
        .notes
        .iter()
        .map(|note| note.locator.identity.as_str().to_owned())
        .collect::<HashSet<_>>();
    let git_root = fs::canonicalize(&context.git_root).unwrap_or_else(|_| context.git_root.clone());
    let mut findings = Vec::new();

    for snapshot_note in &snapshot.notes {
        if snapshot_note.note.frontmatter.status == Some(NoteStatus::Frozen) {
            continue;
        }
        let locator = &snapshot_note.locator;
        for link in &snapshot_note.note.frontmatter.links {
            let identity = link_identity(snapshot, locator, link);
            if !identities.contains(&identity) {
                findings.push(
                    Diagnostic::error(BROKEN_LINK, format!("{BROKEN_LINK_MESSAGE}{identity}"))
                        .at_path(locator.path.clone())
                        .for_field("links"),
                );
            }
        }
        for pointer in &snapshot_note.note.frontmatter.code {
            if !is_path_shaped(pointer) || code_pointer_resolves(&git_root, pointer) {
                continue;
            }
            let mut finding = Diagnostic::error(BROKEN_CODE, BROKEN_CODE_MESSAGE)
                .at_path(locator.path.clone())
                .for_field("code");
            finding.severity = Severity::Warning;
            findings.push(finding);
        }
    }
    findings
}

fn link_identity(
    snapshot: &SourceSnapshot,
    locator: &crate::source::NoteLocator,
    link: &str,
) -> String {
    if link.split(':').count() == 3 {
        return link.to_owned();
    }
    match derive_identity(&snapshot.source, &locator.domain, link) {
        Ok(identity) => identity.to_string(),
        Err(_) => format!("{}:{}:{link}", snapshot.source, locator.domain),
    }
}

fn is_path_shaped(pointer: &str) -> bool {
    let path = Path::new(pointer);
    pointer.contains('/')
        || pointer.contains('\\')
        || path.is_absolute()
        || pointer.starts_with("./")
        || pointer.starts_with("../")
        || pointer.starts_with(".\\")
        || pointer.starts_with("..\\")
}

fn code_pointer_resolves(git_root: &Path, pointer: &str) -> bool {
    let candidate = git_root.join(pointer);
    let Ok(canonical) = fs::canonicalize(&candidate) else {
        return false;
    };
    let Ok(root) = fs::canonicalize(git_root) else {
        return false;
    };
    canonical.starts_with(root)
}
