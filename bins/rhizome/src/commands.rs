use crate::output::{self, envelope};
use clap::ArgMatches;
use kb_contract::{
    Diagnostic, NoteFrontmatter, NoteKind, Registry, RegistryLocator, Severity, resolve_registry,
};
use rhizome_core::adopt::{AdoptError, AdoptRequest, apply_adopt_with_hook, plan_adopt};
use rhizome_core::amend::{apply_amend, plan_amend};
use rhizome_core::author::{AuthorRequest, apply_author, plan_author};
use rhizome_core::capture::{CaptureRequest, apply_capture, plan_capture};
use rhizome_core::check::{CoreError, check_source};
use rhizome_core::doctor::doctor_source;
use rhizome_core::frozen::{ApprovalMarker, check_staged_frozen_for_specs};
use rhizome_core::git::{GitBackend, GitError};
use rhizome_core::human_index::{apply_human_index, check_human_index, plan_human_index};
use rhizome_core::links::check_links_and_code;
use rhizome_core::relocate::{apply_relocate, plan_relocate};
use rhizome_core::source::{
    SourceContext, discover_source, parse_note_file_nofollow, read_regular_file_nofollow_bounded,
    validate_parent_path_nofollow,
};
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct RunResult {
    pub value: Value,
    pub exit_code: i32,
}
impl RunResult {
    fn success(data: Value) -> Self {
        Self {
            value: envelope(true, data, &[]),
            exit_code: 0,
        }
    }
    fn success_with_diagnostics(data: Value, diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            value: envelope(true, data, &diagnostics),
            exit_code: 0,
        }
    }
    fn failure(data: Value, diagnostics: Vec<Diagnostic>, exit_code: i32) -> Self {
        Self {
            value: envelope(false, data, &diagnostics),
            exit_code,
        }
    }
}

pub fn execute(matches: &ArgMatches) -> RunResult {
    match matches.subcommand() {
        Some(("new", args)) => new_note(args),
        Some(("check", args)) => check(args),
        Some(("domains", args)) => domains(args),
        Some(("adopt", args)) => adopt(args),
        Some(("doctor", args)) => doctor(args),
        Some(("amend", args)) => amend(args),
        Some(("relocate", args)) => relocate(args),
        Some(("capture", args)) => capture(args),
        Some(("index", args)) => index(args),
        Some(("stats", args)) => stats(args),
        _ => RunResult::failure(
            Value::Null,
            vec![Diagnostic::error("KBV2-CLI-USAGE", "a command is required")],
            64,
        ),
    }
}

fn new_note(args: &ArgMatches) -> RunResult {
    let source = args
        .get_one::<String>("source")
        .expect("clap required source");
    let domain = match kb_contract::DomainId::new(
        args.get_one::<String>("domain")
            .expect("clap required domain"),
    ) {
        Ok(domain) => domain,
        Err(diagnostic) => return RunResult::failure(Value::Null, vec![diagnostic], 1),
    };
    let slug = args.get_one::<String>("slug").expect("clap required slug");
    let description = args
        .get_one::<String>("description")
        .expect("clap required description")
        .clone();
    let keywords = args
        .get_many::<String>("keywords")
        .map(|items| items.cloned().collect())
        .unwrap_or_default();
    let body = match read_file_arg(args, "body-file") {
        Ok(body) => body,
        Err(diagnostic) => return RunResult::failure(Value::Null, vec![diagnostic], 1),
    };
    let kind = match parse_kind(
        args.get_one::<String>("kind")
            .map(String::as_str)
            .unwrap_or("note"),
    ) {
        Ok(kind) => kind,
        Err(diagnostic) => return RunResult::failure(Value::Null, vec![diagnostic], 1),
    };
    let assets = args
        .get_many::<String>("assets")
        .map(|items| items.cloned().collect())
        .unwrap_or_default();
    let registry = match load_registry() {
        Ok(registry) => registry,
        Err(diagnostics) => return RunResult::failure(Value::Null, diagnostics, 1),
    };
    let context = match context_for(&registry, source) {
        Ok(context) => context,
        Err(diagnostics) => return RunResult::failure(Value::Null, diagnostics, 1),
    };
    let request = AuthorRequest {
        context,
        domain,
        slug: slug.clone(),
        frontmatter: NoteFrontmatter {
            description,
            keywords,
            kind,
            links: Vec::new(),
            code: Vec::new(),
            assets,
            supersedes: None,
            status: None,
        },
        body,
    };
    let plan = match plan_author(&request) {
        Ok(plan) => plan,
        Err(error) => return RunResult::failure(Value::Null, error.into_diagnostics(), 1),
    };
    if let Err(error) = apply_author(&plan) {
        return RunResult::failure(Value::Null, error.into_diagnostics(), 1);
    }
    RunResult::success(json!({"path": plan.path(), "identity": plan.identity().to_string()}))
}

fn check(args: &ArgMatches) -> RunResult {
    let paths = args
        .get_many::<String>("path")
        .map(|paths| paths.map(PathBuf::from).collect::<Vec<_>>())
        .unwrap_or_default();
    let selected = args.get_one::<String>("source").map(String::as_str);
    if args.get_flag("all") && !paths.is_empty() {
        return RunResult::failure(
            Value::Null,
            vec![Diagnostic::error(
                "KBV2-CLI-USAGE",
                "--all cannot be combined with paths",
            )],
            64,
        );
    }
    if !paths.is_empty() {
        if env::var_os("KB_SOURCES").is_some()
            || selected.is_some()
            || args.get_one::<String>("registry").is_some()
        {
            let registry = match load_registry_arg(args) {
                Ok(registry) => registry,
                Err(diagnostics) => return RunResult::failure(Value::Null, diagnostics, 1),
            };
            return check_registered_paths(&registry, selected, paths);
        }
        return check_direct_paths(paths);
    }
    let registry = match load_registry_arg(args) {
        Ok(registry) => registry,
        Err(diagnostics) => return RunResult::failure(Value::Null, diagnostics, 1),
    };
    if let Some(source) = selected {
        return context_for(&registry, source)
            .map(|context| check_context(&context, &registry))
            .unwrap_or_else(|diagnostics| RunResult::failure(Value::Null, diagnostics, 1));
    }
    check_all_sources(&registry)
}
fn check_direct_paths(paths: Vec<PathBuf>) -> RunResult {
    let mut diagnostics = Vec::new();
    let mut contract_failed = false;
    for path in paths {
        let path = match std::path::absolute(&path) {
            Ok(path) => path,
            Err(_) => {
                contract_failed = true;
                diagnostics.push(Diagnostic::error(
                    "KBV2-SOURCE-READ",
                    "source entry could not be read",
                ));
                continue;
            }
        };
        match parse_note_file_nofollow(&path) {
            Ok(note) => diagnostics.extend(rhizome_core::mermaid::check_note_mermaid(&path, &note)),
            Err(mut failures) => {
                contract_failed = true;
                diagnostics.append(&mut failures);
            }
        }
    }
    if contract_failed {
        RunResult::failure(Value::Null, diagnostics, 1)
    } else if diagnostics
        .iter()
        .any(|finding| finding.severity == Severity::Error)
    {
        RunResult::failure(Value::Null, diagnostics, 3)
    } else {
        RunResult::success_with_diagnostics(Value::Null, diagnostics)
    }
}

fn check_all_sources(registry: &Registry) -> RunResult {
    let mut rows = Vec::new();
    let mut diagnostics = Vec::new();
    let mut operation_failed = false;
    for spec in registry.sources.values() {
        let context = match context_for_spec(spec, &registry.origin) {
            Ok(context) => context,
            Err(mut failures) => {
                operation_failed = true;
                diagnostics.append(&mut failures);
                continue;
            }
        };
        match check_source(&context) {
            Ok(report) => {
                diagnostics.extend(report.findings.clone());
                rows.push(json!({"source": report.source.to_string(), "findings": report.findings.iter().map(output::diagnostic_value).collect::<Vec<_>>() }));
            }
            Err(error) => {
                operation_failed = true;
                diagnostics.extend(core_diagnostics(error));
            }
        }
        if let Err((code, diagnostic)) = frozen_gate(&context, registry) {
            if code == 1 {
                operation_failed = true;
            }
            diagnostics.push(diagnostic);
        }
    }
    let data = json!(rows);
    if operation_failed {
        RunResult::failure(data, diagnostics, 1)
    } else if diagnostics
        .iter()
        .any(|finding| finding.severity == Severity::Error)
    {
        RunResult::failure(data, diagnostics, 3)
    } else {
        RunResult::success_with_diagnostics(data, diagnostics)
    }
}

fn check_registered_paths(
    registry: &Registry,
    selected: Option<&str>,
    mut paths: Vec<PathBuf>,
) -> RunResult {
    if let Some(source) = selected {
        if !registry.sources.contains_key(source) {
            return RunResult::failure(
                Value::Null,
                vec![
                    Diagnostic::error(
                        "KBV2-REGISTRY-UNKNOWN-SOURCE",
                        "logical source is not registered",
                    )
                    .for_field("source"),
                ],
                1,
            );
        }
    }
    paths.sort_by_key(|path| path.to_string_lossy().into_owned());
    let mut diagnostics = Vec::new();
    let mut rows = Vec::new();
    let mut checked = std::collections::BTreeSet::new();
    let mut human_checked = std::collections::BTreeSet::new();
    let mut operation_failed = false;
    for path in paths {
        let absolute = match std::path::absolute(&path) {
            Ok(path) => path,
            Err(_) => {
                operation_failed = true;
                diagnostics.push(
                    Diagnostic::error("KBV2-SOURCE-READ", "source entry could not be read")
                        .at_path(path),
                );
                continue;
            }
        };
        let Some(spec) = registry
            .sources
            .values()
            .filter(|spec| {
                selected.is_none_or(|name| name == spec.name.as_str())
                    && path_within(&absolute, &spec.root)
            })
            .max_by_key(|spec| spec.root.components().count())
        else {
            rows.push(json!({"path": absolute, "ignored": true}));
            continue;
        };
        let context = match context_for_spec(spec, &registry.origin) {
            Ok(context) => context,
            Err(mut failures) => {
                operation_failed = true;
                diagnostics.append(&mut failures);
                continue;
            }
        };
        checked.insert(spec.name.to_string());
        let snapshot = match discover_source(&context) {
            Ok(snapshot) => snapshot,
            Err(mut failures) => {
                operation_failed = true;
                diagnostics.append(&mut failures);
                continue;
            }
        };
        if human_checked.insert(spec.name.to_string()) {
            let root_index = context.source.root.join("INDEX.md");
            match check_human_index(&snapshot, &root_index) {
                Ok(findings) => diagnostics.extend(findings),
                Err(error) => {
                    operation_failed = true;
                    diagnostics.extend(core_diagnostics(error));
                }
            }
        }
        if validate_parent_path_nofollow(&absolute).is_err() {
            operation_failed = true;
            diagnostics.push(
                Diagnostic::error("KBV2-SOURCE-READ", "source entry could not be read")
                    .at_path(absolute.clone()),
            );
            continue;
        }
        match std::fs::symlink_metadata(&absolute) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                operation_failed = true;
                diagnostics.push(
                    Diagnostic::error("KBV2-SOURCE-READ", "source entry could not be read")
                        .at_path(absolute.clone()),
                );
                continue;
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let backend = match GitBackend::new(&context.git_root) {
                    Ok(backend) => backend,
                    Err(error) => {
                        operation_failed = true;
                        diagnostics.push(Diagnostic::error("KBV2-GIT", error.to_string()));
                        continue;
                    }
                };
                match backend.is_staged_deletion(&absolute) {
                    Ok(true) => {
                        // A staged deletion has no working-tree file to parse.
                        // The frozen gate below remains responsible for authorizing it.
                        rows.push(json!({"path": absolute, "deleted": true}));
                        continue;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        operation_failed = true;
                        diagnostics.push(Diagnostic::error("KBV2-GIT", error.to_string()));
                        continue;
                    }
                }
                operation_failed = true;
                diagnostics.push(
                    Diagnostic::error("KBV2-SOURCE-READ", "source entry could not be read")
                        .at_path(absolute.clone()),
                );
                continue;
            }
            Err(_) => {
                operation_failed = true;
                diagnostics.push(
                    Diagnostic::error("KBV2-SOURCE-READ", "source entry could not be read")
                        .at_path(absolute.clone()),
                );
                continue;
            }
        }
        let canonical = match std::fs::canonicalize(&absolute) {
            Ok(path) => path,
            Err(_) => {
                operation_failed = true;
                diagnostics.push(
                    Diagnostic::error("KBV2-SOURCE-READ", "source entry could not be read")
                        .at_path(absolute),
                );
                continue;
            }
        };
        if !path_within(&canonical, &context.source.root) {
            diagnostics.push(
                Diagnostic::error(
                    "KBV2-SOURCE-OUTSIDE-GIT",
                    "source path is outside the registered source",
                )
                .at_path(canonical),
            );
            continue;
        }
        let root_index = context.source.root.join("INDEX.md");
        let mut findings = if canonical == root_index {
            match check_human_index(&snapshot, &root_index) {
                Ok(findings) => findings,
                Err(error) => {
                    operation_failed = true;
                    core_diagnostics(error)
                }
            }
        } else if snapshot
            .notes
            .iter()
            .any(|note| note.locator.path == canonical)
        {
            let mut findings = check_links_and_code(&snapshot, &context);
            for snapshot_note in &snapshot.notes {
                findings.extend(rhizome_core::mermaid::check_note_mermaid(
                    &snapshot_note.locator.path,
                    &snapshot_note.note,
                ));
            }
            findings
        } else {
            Vec::new()
        };
        let target_path = canonical.clone();
        findings.retain(|finding| {
            finding
                .path
                .as_ref()
                .is_none_or(|path| path == &target_path)
        });
        diagnostics.extend(findings.clone());
        rows.push(json!({"path": canonical, "findings": findings.iter().map(output::diagnostic_value).collect::<Vec<_>>() }));
    }
    let mut gated_roots = std::collections::BTreeSet::new();
    for spec in registry
        .sources
        .values()
        .filter(|spec| selected.is_none_or(|name| name == spec.name.as_str()))
    {
        if !checked.contains(spec.name.as_str()) {
            continue;
        }
        match context_for_spec(spec, &registry.origin) {
            Ok(context) if gated_roots.insert(context.git_root.clone()) => {
                if let Err((code, diagnostic)) = frozen_gate(&context, registry) {
                    if code == 1 {
                        operation_failed = true;
                    }
                    diagnostics.push(diagnostic);
                }
            }
            Ok(_) => {}
            Err(mut failures) => {
                operation_failed = true;
                diagnostics.append(&mut failures);
            }
        }
    }
    if operation_failed {
        RunResult::failure(json!(rows), diagnostics, 1)
    } else if diagnostics
        .iter()
        .any(|finding| finding.severity == Severity::Error)
    {
        RunResult::failure(json!(rows), diagnostics, 3)
    } else {
        RunResult::success_with_diagnostics(json!(rows), diagnostics)
    }
}

fn frozen_gate(context: &SourceContext, registry: &Registry) -> Result<(), (i32, Diagnostic)> {
    let backend = GitBackend::new(&context.git_root)
        .map_err(|error| (1, Diagnostic::error("KBV2-GIT", error.to_string())))?;
    match backend.head_oid() {
        Ok(_) => {}
        Err(GitError::CommandFailed("rev-parse")) => return Ok(()),
        Err(error) => return Err((1, Diagnostic::error("KBV2-GIT", error.to_string()))),
    }
    let specs = registry
        .sources
        .values()
        .filter(|spec| find_git_root(&spec.root).as_ref() == Some(&context.git_root))
        .cloned()
        .collect::<Vec<_>>();
    check_staged_frozen_for_specs(&backend, &specs)
        .map_err(|error| (3, Diagnostic::error("KBV2-FROZEN-GATE", error.to_string())))
}
fn check_context(context: &SourceContext, registry: &Registry) -> RunResult {
    if let Err((code, diagnostic)) = frozen_gate(context, registry) {
        return RunResult::failure(Value::Null, vec![diagnostic], code);
    }
    match check_source(context) {
        Ok(report) => {
            let findings = report.findings.clone();
            let data = json!({"source": report.source.to_string(), "findings": findings.iter().map(output::diagnostic_value).collect::<Vec<_>>()});
            if findings
                .iter()
                .any(|finding| finding.severity == kb_contract::Severity::Error)
            {
                RunResult::failure(data, findings, 3)
            } else {
                RunResult::success_with_diagnostics(data, findings)
            }
        }
        Err(error) => RunResult::failure(Value::Null, core_diagnostics(error), 1),
    }
}

fn domains(args: &ArgMatches) -> RunResult {
    let registry = match load_registry() {
        Ok(registry) => registry,
        Err(diagnostics) => return RunResult::failure(Value::Null, diagnostics, 1),
    };
    let selected = args.get_one::<String>("source");
    if let Some(name) = selected {
        if !registry.sources.contains_key(name.as_str()) {
            return RunResult::failure(
                Value::Null,
                vec![
                    Diagnostic::error(
                        "KBV2-REGISTRY-UNKNOWN-SOURCE",
                        "logical source is not registered",
                    )
                    .for_field("source"),
                ],
                1,
            );
        }
    }
    let mut rows = Vec::new();
    let mut diagnostics = Vec::new();
    for spec in registry.sources.values() {
        if selected.is_some_and(|name| name != spec.name.as_str()) {
            continue;
        }
        let context = match context_for_spec(spec, &registry.origin) {
            Ok(context) => context,
            Err(mut failures) => {
                diagnostics.append(&mut failures);
                continue;
            }
        };
        match discover_source(&context) {
            Ok(snapshot) => rows.extend(snapshot.domains.into_iter().map(|domain| {
                json!({
                    "source": context.source.name.to_string(),
                    "domain": domain.id.to_string(),
                    "path": domain.physical_dir,
                })
            })),
            Err(mut failures) => diagnostics.append(&mut failures),
        }
    }
    rows.sort_by(|left, right| left.to_string().cmp(&right.to_string()));
    if diagnostics.is_empty() {
        RunResult::success(json!(rows))
    } else {
        RunResult::failure(json!(rows), diagnostics, 1)
    }
}

fn adopt(args: &ArgMatches) -> RunResult {
    let registry = PathBuf::from(
        args.get_one::<String>("registry")
            .expect("clap required registry"),
    );
    let request = AdoptRequest {
        registry,
        logical_source: args
            .get_one::<String>("source")
            .expect("clap required source")
            .clone(),
        repo: PathBuf::from(args.get_one::<String>("repo").expect("clap required repo")),
        description: args
            .get_one::<String>("description")
            .expect("clap required description")
            .clone(),
        keywords: args
            .get_many::<String>("keywords")
            .map(|items| items.cloned().collect())
            .unwrap_or_default(),
    };
    let plan = match plan_adopt(&request) {
        Ok(plan) => plan,
        Err(error) => return RunResult::failure(Value::Null, error.into_diagnostics(), 1),
    };
    let apply_result = apply_adopt_with_hook(&plan, install_lefthook);
    if let Err(error) = apply_result {
        return RunResult::failure(Value::Null, error.into_diagnostics(), 1);
    }
    RunResult::success(
        json!({"registry": plan.registry(), "repo": plan.repo(), "index_created": plan.creates_index(), "gate_created": plan.installs_gate()}),
    )
}

fn doctor(args: &ArgMatches) -> RunResult {
    let registry = match load_registry() {
        Ok(registry) => registry,
        Err(diagnostics) => return RunResult::failure(Value::Null, diagnostics, 1),
    };
    let selected = args.get_one::<String>("source");
    if let Some(name) = selected {
        if !registry.sources.contains_key(name.as_str()) {
            return RunResult::failure(
                Value::Null,
                vec![
                    Diagnostic::error(
                        "KBV2-REGISTRY-UNKNOWN-SOURCE",
                        "logical source is not registered",
                    )
                    .for_field("source"),
                ],
                1,
            );
        }
    }
    let mut rows = Vec::new();
    let mut diagnostics = Vec::new();
    for spec in registry.sources.values() {
        if selected.is_some_and(|name| name != spec.name.as_str()) {
            continue;
        }
        let context = match context_for_spec(spec, &registry.origin) {
            Ok(context) => context,
            Err(mut failures) => {
                diagnostics.append(&mut failures);
                continue;
            }
        };
        let report = doctor_source(&context);
        diagnostics.extend(report.diagnostics.clone());
        rows.push(json!({"source": report.source, "checks": report.checks.iter().map(|check| json!({"name": check.name, "ok": check.ok})).collect::<Vec<_>>() }));
    }
    if diagnostics.is_empty() {
        RunResult::success(json!(rows))
    } else {
        RunResult::failure(json!(rows), diagnostics, 1)
    }
}
fn install_lefthook(repo: &Path) -> Result<(), AdoptError> {
    let hooks_dir = effective_hooks_dir(repo)?;
    let pre_commit = hooks_dir.join("pre-commit");
    if let Some(bytes) = read_existing_hook(&pre_commit)? {
        if hook_invokes_lefthook(&bytes) {
            return Ok(());
        }
        return Err(AdoptError::Diagnostics(vec![
            Diagnostic::error(
                "KBV2-ADOPT-HOOK-CONFLICT",
                "an existing pre-commit hook is managed by another tool",
            )
            .at_path(pre_commit),
        ]));
    }

    let status = match Command::new("lefthook")
        .arg("install")
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) => status,
        Err(_) => {
            return Err(hook_failure(
                "lefthook install could not be run",
                repo,
                &pre_commit,
            ));
        }
    };
    if !status.success() {
        return Err(hook_failure("lefthook install failed", repo, &pre_commit));
    }
    let Some(bytes) = read_existing_hook(&pre_commit)? else {
        return Err(AdoptError::Diagnostics(vec![
            Diagnostic::error(
                "KBV2-ADOPT-LEFTHOOK",
                "lefthook install did not create a pre-commit hook",
            )
            .at_path(pre_commit),
        ]));
    };
    if !hook_invokes_lefthook(&bytes) {
        let cleanup = remove_created_hook(&pre_commit);
        let mut diagnostics = vec![
            Diagnostic::error(
                "KBV2-ADOPT-HOOK-CONFLICT",
                "lefthook install did not install the expected hook",
            )
            .at_path(pre_commit),
        ];
        if let Err(error) = cleanup {
            diagnostics.extend(error.into_diagnostics());
        }
        return Err(AdoptError::Diagnostics(diagnostics));
    }
    Ok(())
}

fn hook_failure(message: &'static str, repo: &Path, pre_commit: &Path) -> AdoptError {
    let mut diagnostics =
        vec![Diagnostic::error("KBV2-ADOPT-LEFTHOOK", message).at_path(repo.to_path_buf())];
    if let Err(error) = remove_created_hook(pre_commit) {
        diagnostics.extend(error.into_diagnostics());
    }
    AdoptError::Diagnostics(diagnostics)
}

fn remove_created_hook(path: &Path) -> Result<(), AdoptError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            std::fs::remove_file(path).map_err(|source| AdoptError::Io {
                path: path.to_path_buf(),
                source,
            })
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(AdoptError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn effective_hooks_dir(repo: &Path) -> Result<PathBuf, AdoptError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["config", "--get", "core.hooksPath"])
        .output()
        .map_err(|_| {
            AdoptError::Diagnostics(vec![
                Diagnostic::error(
                    "KBV2-ADOPT-HOOK",
                    "Git hook configuration could not be read",
                )
                .at_path(repo.to_path_buf()),
            ])
        })?;
    if output.status.success() {
        let raw = std::str::from_utf8(&output.stdout)
            .ok()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AdoptError::Diagnostics(vec![
                    Diagnostic::error(
                        "KBV2-ADOPT-HOOK",
                        "Git hook configuration is not valid UTF-8",
                    )
                    .at_path(repo.to_path_buf()),
                ])
            })?;
        let path = PathBuf::from(raw);
        return Ok(if path.is_absolute() {
            path
        } else {
            repo.join(path)
        });
    }
    if output.status.code() == Some(1) {
        Ok(repo.join(".git/hooks"))
    } else {
        Err(AdoptError::Diagnostics(vec![
            Diagnostic::error(
                "KBV2-ADOPT-HOOK",
                "Git hook configuration could not be read",
            )
            .at_path(repo.to_path_buf()),
        ]))
    }
}

fn read_existing_hook(path: &Path) -> Result<Option<Vec<u8>>, AdoptError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(AdoptError::Diagnostics(vec![
                Diagnostic::error(
                    "KBV2-ADOPT-HOOK-CONFLICT",
                    "the effective pre-commit hook is not a regular file",
                )
                .at_path(path.to_path_buf()),
            ]))
        }
        Ok(_) => read_regular_file_nofollow_bounded(path, 1024 * 1024)
            .map(Some)
            .map_err(|_| {
                AdoptError::Diagnostics(vec![
                    Diagnostic::error(
                        "KBV2-ADOPT-HOOK",
                        "the effective pre-commit hook could not be read",
                    )
                    .at_path(path.to_path_buf()),
                ])
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(AdoptError::Diagnostics(vec![
            Diagnostic::error(
                "KBV2-ADOPT-HOOK",
                "the effective pre-commit hook could not be read",
            )
            .at_path(path.to_path_buf()),
        ])),
    }
}

fn hook_invokes_lefthook(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    text.contains("lefthook") || text.contains("rhizome check")
}

fn index(args: &ArgMatches) -> RunResult {
    match args.subcommand() {
        Some(("check", child)) => check(child),
        Some(("sync", child)) => index_sync(child),
        _ => RunResult::failure(
            Value::Null,
            vec![Diagnostic::error(
                "KBV2-CLI-USAGE",
                "an index command is required",
            )],
            64,
        ),
    }
}
fn index_sync(args: &ArgMatches) -> RunResult {
    if !args.get_flag("force") {
        return RunResult::failure(
            Value::Null,
            vec![Diagnostic::error(
                "KBV2-INDEX-UNSAFE",
                "index sync requires explicit --force",
            )],
            2,
        );
    }
    let registry = match load_registry() {
        Ok(registry) => registry,
        Err(diagnostics) => return RunResult::failure(Value::Null, diagnostics, 1),
    };
    let mut changed = Vec::new();
    let mut diagnostics = Vec::new();
    for spec in registry.sources.values() {
        let context = match context_for_spec(spec, &registry.origin) {
            Ok(context) => context,
            Err(mut failures) => {
                diagnostics.append(&mut failures);
                continue;
            }
        };
        match discover_source(&context) {
            Ok(snapshot) => {
                let index = context.source.root.join("INDEX.md");
                match plan_human_index(&snapshot, &index) {
                    Ok(plan) => {
                        if let Err(error) = apply_human_index(plan) {
                            diagnostics.push(core_error_diagnostic(error));
                        } else {
                            changed.push(index);
                        }
                    }
                    Err(error) => diagnostics.push(core_error_diagnostic(error)),
                }
            }
            Err(mut failures) => diagnostics.append(&mut failures),
        }
    }
    if diagnostics.is_empty() {
        RunResult::success(json!({"changed": changed}))
    } else {
        RunResult::failure(json!({"changed": changed}), diagnostics, 1)
    }
}

fn stats(args: &ArgMatches) -> RunResult {
    let registry = match load_registry() {
        Ok(registry) => registry,
        Err(diagnostics) => return RunResult::failure(Value::Null, diagnostics, 1),
    };
    let selected = args.get_one::<String>("source").map(String::as_str);
    if let Some(name) = selected {
        if !registry.sources.contains_key(name) {
            return RunResult::failure(
                Value::Null,
                vec![
                    Diagnostic::error(
                        "KBV2-REGISTRY-UNKNOWN-SOURCE",
                        "logical source is not registered",
                    )
                    .for_field("source"),
                ],
                1,
            );
        }
    }
    let mut source_count = 0usize;
    let mut domains_count = 0usize;
    let mut notes_count = 0usize;
    let mut diagnostics = Vec::new();
    for spec in registry
        .sources
        .values()
        .filter(|spec| selected.is_none_or(|name| name == spec.name.as_str()))
    {
        source_count += 1;
        match context_for_spec(spec, &registry.origin)
            .and_then(|context| discover_source(&context).map_err(|findings| findings))
        {
            Ok(snapshot) => {
                domains_count += snapshot.domains.len();
                notes_count += snapshot
                    .notes
                    .iter()
                    .filter(|note| !note.locator.is_domain_index)
                    .count();
            }
            Err(mut failures) => diagnostics.append(&mut failures),
        }
    }
    let data = json!({"sources": source_count, "domains": domains_count, "notes": notes_count});
    if diagnostics.is_empty() {
        RunResult::success(data)
    } else {
        RunResult::failure(data, diagnostics, 1)
    }
}

fn capture(args: &ArgMatches) -> RunResult {
    let text = args
        .get_many::<String>("text")
        .map(|parts| parts.map(String::as_str).collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    let inbox = env::var_os("RHIZOME_INBOX")
        .map(PathBuf::from)
        .unwrap_or_else(default_inbox);
    let request = CaptureRequest {
        inbox,
        text,
        timestamp: now_timestamp(),
    };
    let plan = match plan_capture(&request) {
        Ok(plan) => plan,
        Err(error) => return RunResult::failure(Value::Null, error.into_diagnostics(), 1),
    };
    if let Err(error) = apply_capture(&plan) {
        return RunResult::failure(Value::Null, error.into_diagnostics(), 1);
    }
    RunResult::success(json!({"path": plan.path()}))
}

fn amend(args: &ArgMatches) -> RunResult {
    let registry = match load_registry() {
        Ok(registry) => registry,
        Err(diagnostics) => return RunResult::failure(Value::Null, diagnostics, 1),
    };
    let source_name = args
        .get_one::<String>("source")
        .expect("clap required source");
    let Some(spec) = registry.sources.get(source_name.as_str()) else {
        return RunResult::failure(
            Value::Null,
            vec![
                Diagnostic::error(
                    "KBV2-REGISTRY-UNKNOWN-SOURCE",
                    "logical source is not registered",
                )
                .for_field("source"),
            ],
            1,
        );
    };
    let context = match context_for_spec(spec, &registry.origin) {
        Ok(context) => context,
        Err(diagnostics) => return RunResult::failure(Value::Null, diagnostics, 1),
    };
    let requested_path = PathBuf::from(args.get_one::<String>("path").expect("clap required path"));
    let path = if requested_path.is_absolute() {
        requested_path
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(requested_path)
    };
    let path = match std::fs::canonicalize(&path) {
        Ok(path) if path_within(&path, &spec.root) => path,
        _ => {
            return RunResult::failure(
                Value::Null,
                vec![
                    Diagnostic::error(
                        "KBV2-SOURCE-OUTSIDE-GIT",
                        "source path is outside the registered source",
                    )
                    .at_path(path),
                ],
                1,
            );
        }
    };
    let replacement = match read_file_arg(args, "body-file") {
        Ok(bytes) => bytes,
        Err(diagnostic) => return RunResult::failure(Value::Null, vec![diagnostic], 1),
    };
    let reason = args
        .get_one::<String>("reason")
        .expect("clap required reason");
    let marker = ApprovalMarker::for_one_file(path.clone(), reason.clone());
    let backend = match GitBackend::new(&context.git_root) {
        Ok(backend) => backend,
        Err(error) => {
            return RunResult::failure(
                Value::Null,
                vec![Diagnostic::error("KBV2-GIT", error.to_string())],
                1,
            );
        }
    };
    let plan = match plan_amend(&context, &path, &replacement, reason, &marker) {
        Ok(plan) => plan,
        Err(error) => {
            return RunResult::failure(
                Value::Null,
                vec![Diagnostic::error("KBV2-AMEND", error.to_string())],
                1,
            );
        }
    };
    let _ = backend;
    if let Err(error) = apply_amend(&plan) {
        return RunResult::failure(
            Value::Null,
            vec![Diagnostic::error("KBV2-AMEND", error.to_string())],
            1,
        );
    }
    RunResult::success(json!({"path": path}))
}

fn relocate(args: &ArgMatches) -> RunResult {
    let registry = match load_registry() {
        Ok(registry) => registry,
        Err(diagnostics) => return RunResult::failure(Value::Null, diagnostics, 1),
    };
    let source_name = args
        .get_one::<String>("source")
        .expect("clap required source");
    let context = match context_for(&registry, source_name) {
        Ok(context) => context,
        Err(diagnostics) => return RunResult::failure(Value::Null, diagnostics, 1),
    };
    let requested_path = PathBuf::from(args.get_one::<String>("path").expect("clap required path"));
    let path = if requested_path.is_absolute() {
        requested_path
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(requested_path)
    };
    let path = match std::fs::canonicalize(&path) {
        Ok(path) if path_within(&path, &context.source.root) => path,
        _ => {
            return RunResult::failure(
                Value::Null,
                vec![
                    Diagnostic::error(
                        "KBV2-SOURCE-OUTSIDE-GIT",
                        "source path is outside the registered source",
                    )
                    .at_path(path),
                ],
                1,
            );
        }
    };
    let target = args
        .get_one::<String>("target")
        .expect("clap required target");
    let plan = match plan_relocate(&registry, &path, target) {
        Ok(plan) => plan,
        Err(error) => {
            return RunResult::failure(
                Value::Null,
                vec![Diagnostic::error("KBV2-RELOCATE", error.to_string())],
                1,
            );
        }
    };
    if let Err(error) = apply_relocate(&plan) {
        return RunResult::failure(
            Value::Null,
            vec![Diagnostic::error("KBV2-RELOCATE", error.to_string())],
            1,
        );
    }
    RunResult::success(json!({"path": path, "target": target}))
}

fn load_registry() -> Result<Registry, Vec<Diagnostic>> {
    load_registry_with(None)
}

fn load_registry_arg(args: &ArgMatches) -> Result<Registry, Vec<Diagnostic>> {
    load_registry_with(args.get_one::<String>("registry").map(PathBuf::from))
}

fn load_registry_with(explicit: Option<PathBuf>) -> Result<Registry, Vec<Diagnostic>> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let absolutize = |path: PathBuf| {
        if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        }
    };
    let explicit = explicit.map(&absolutize);
    let env_path = env::var_os("KB_SOURCES")
        .map(PathBuf::from)
        .map(&absolutize);
    let workspace_root = env::var_os("KB_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .map(&absolutize);
    let user_config = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.clone())
        .join(".config/knowledge-infra/sources.toml");
    resolve_registry(&RegistryLocator {
        explicit,
        cwd,
        env_path,
        workspace_root,
        user_config,
    })
}

fn context_for(registry: &Registry, source: &str) -> Result<SourceContext, Vec<Diagnostic>> {
    let Some(spec) = registry.sources.get(source) else {
        return Err(vec![
            Diagnostic::error(
                "KBV2-REGISTRY-UNKNOWN-SOURCE",
                "logical source is not registered",
            )
            .for_field("source"),
        ]);
    };
    context_for_spec(spec, &registry.origin)
}
fn context_for_spec(
    spec: &kb_contract::SourceSpec,
    origin: &Path,
) -> Result<SourceContext, Vec<Diagnostic>> {
    let git_root = find_git_root(&spec.root).ok_or_else(|| {
        vec![
            Diagnostic::error(
                "KBV2-SOURCE-GIT-ROOT",
                "Git root must be an existing directory containing .git",
            )
            .at_path(spec.root.clone()),
        ]
    })?;
    Ok(SourceContext {
        source: spec.clone(),
        git_root,
        registry_origin: origin.to_path_buf(),
    })
}
fn find_git_root(path: &Path) -> Option<PathBuf> {
    let mut current = fs::canonicalize(path).ok()?;
    loop {
        if let Ok(backend) = GitBackend::new(&current) {
            return Some(backend.root().to_path_buf());
        }
        if !current.pop() {
            return None;
        }
    }
}
fn path_within(path: &Path, root: &Path) -> bool {
    let path = normalize_compare_path(path);
    let root = normalize_compare_path(root);
    path == root || path.starts_with(root)
}
fn normalize_compare_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}
fn read_file_arg(args: &ArgMatches, name: &str) -> Result<Vec<u8>, Diagnostic> {
    const MAX_INPUT: usize = 64 * 1024 * 1024;
    let path = args
        .get_one::<String>(name)
        .expect("clap required file argument");
    if path == "-" {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take((MAX_INPUT + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| Diagnostic::error("KBV2-CLI-IO", "could not read standard input"))?;
        if bytes.len() > MAX_INPUT {
            return Err(Diagnostic::error("KBV2-CLI-IO", "input file is too large"));
        }
        return Ok(bytes);
    }
    let input = PathBuf::from(path);
    let input = if input.is_absolute() {
        input
    } else {
        std::path::absolute(&input)
            .map_err(|_| Diagnostic::error("KBV2-CLI-IO", "could not read input file"))?
    };
    rhizome_core::source::read_regular_file_nofollow_bounded(&input, MAX_INPUT)
        .map_err(|_| Diagnostic::error("KBV2-CLI-IO", "could not read input file").at_path(input))
}
fn parse_kind(value: &str) -> Result<NoteKind, Diagnostic> {
    match value {
        "spec" => Ok(NoteKind::Spec),
        "reference" => Ok(NoteKind::Reference),
        "runbook" => Ok(NoteKind::Runbook),
        "decision" => Ok(NoteKind::Decision),
        "research" => Ok(NoteKind::Research),
        "note" => Ok(NoteKind::Note),
        "index" => Ok(NoteKind::Index),
        _ => Err(Diagnostic::error(
            "KBV2-NOTE-INVALID-KIND",
            "note kind must be spec, reference, runbook, decision, research, note, or index",
        )
        .for_field("kind")),
    }
}
fn core_diagnostics(error: CoreError) -> Vec<Diagnostic> {
    match error {
        CoreError::Discovery(diagnostics) => diagnostics,
        other => vec![core_error_diagnostic(other)],
    }
}
fn core_error_diagnostic<E: std::fmt::Display>(error: E) -> Diagnostic {
    Diagnostic::error("KBV2-OPERATION", error.to_string())
}
fn default_inbox() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/rhizome/inbox.md")
}
fn now_timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = seconds / 86_400;
    let rem = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}
