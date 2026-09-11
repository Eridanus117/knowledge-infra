use crate::human_index::check_human_index;
use crate::source::{SourceContext, discover_source, read_regular_file_nofollow_bounded};
use kb_contract::{Diagnostic, Severity};
use std::path::PathBuf;

/// Stable diagnostics produced by the source-plane doctor command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorReport {
    pub schema: &'static str,
    pub source: String,
    pub checks: Vec<DoctorCheck>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub ok: bool,
}

/// Inspect one configured source without repairing it or invoking external tools.
pub fn doctor_source(context: &SourceContext) -> DoctorReport {
    let mut checks = Vec::new();
    let mut diagnostics = Vec::new();
    let git_ok = context.git_root.is_dir()
        && std::fs::symlink_metadata(context.git_root.join(".git"))
            .map(|metadata| metadata.is_dir() || metadata.is_file())
            .unwrap_or(false);
    checks.push(DoctorCheck {
        name: "git-root",
        ok: git_ok,
    });
    if !git_ok {
        diagnostics.push(
            Diagnostic::error(
                "KBV2-SOURCE-GIT-ROOT",
                "Git root must be an existing directory containing .git",
            )
            .at_path(context.git_root.clone()),
        );
    }
    let registry_ok = context.registry_origin.is_file();
    checks.push(DoctorCheck {
        name: "registry",
        ok: registry_ok,
    });
    if !registry_ok {
        diagnostics.push(
            Diagnostic::error("KBV2-DOCTOR-REGISTRY", "source registry could not be read")
                .at_path(context.registry_origin.clone()),
        );
    }
    let gate_path = context.git_root.join("lefthook.yml");
    let gate_ok = read_regular_file_nofollow_bounded(&gate_path, 1024 * 1024)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .is_some_and(|text| {
            text.lines()
                .map(str::trim)
                .any(|line| line == "run: rhizome check -- {staged_files}")
        });
    checks.push(DoctorCheck {
        name: "adoption-gate",
        ok: gate_ok,
    });
    if !gate_ok {
        diagnostics.push(
            Diagnostic::error(
                "KBV2-DOCTOR-GATE",
                "lefthook adoption gate is missing or invalid",
            )
            .at_path(gate_path),
        );
    }
    match discover_source(context) {
        Ok(snapshot) => {
            checks.push(DoctorCheck {
                name: "source-discovery",
                ok: true,
            });
            checks.push(DoctorCheck {
                name: "domains",
                ok: !snapshot.domains.is_empty(),
            });
            if snapshot.domains.is_empty() {
                diagnostics.push(
                    Diagnostic::error("KBV2-DOCTOR-NO-DOMAINS", "source contains no C2 domains")
                        .at_path(context.source.root.clone()),
                );
            }
            let root_index = context.source.root.join("INDEX.md");
            let root_index_ok = match check_human_index(&snapshot, &root_index) {
                Ok(findings) => {
                    let ok = findings.is_empty();
                    diagnostics.extend(findings);
                    ok
                }
                Err(_) => false,
            };
            checks.push(DoctorCheck {
                name: "human-index",
                ok: root_index_ok,
            });
            if !root_index_ok {
                diagnostics.push(
                    Diagnostic::error(
                        "KBV2-DOCTOR-HUMAN-INDEX",
                        "source human index is missing or invalid",
                    )
                    .at_path(root_index),
                );
            }
        }
        Err(mut findings) => {
            checks.push(DoctorCheck {
                name: "source-discovery",
                ok: false,
            });
            diagnostics.append(&mut findings);
        }
    }
    diagnostics.sort_by(|left, right| {
        left.code
            .cmp(right.code)
            .then_with(|| {
                left.path
                    .as_ref()
                    .map(PathBuf::as_path)
                    .cmp(&right.path.as_ref().map(PathBuf::as_path))
            })
            .then_with(|| left.field.cmp(&right.field))
            .then_with(|| left.message.cmp(&right.message))
    });
    DoctorReport {
        schema: "rhizome-doctor-v2",
        source: context.source.name.to_string(),
        checks,
        diagnostics,
    }
}

pub fn doctor(context: &SourceContext) -> DoctorReport {
    doctor_source(context)
}

#[must_use]
pub fn doctor_ok(report: &DoctorReport) -> bool {
    report
        .diagnostics
        .iter()
        .all(|diagnostic| diagnostic.severity == Severity::Warning)
}
