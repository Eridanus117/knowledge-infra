use kb_contract::{
    Diagnostic, Registry, RegistryLocator, Severity, SourceName, SourceSpec, Surface,
    resolve_registry,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

struct ScratchDirectory {
    path: PathBuf,
}

impl ScratchDirectory {
    fn new() -> Self {
        let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the system clock should be after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "kb-contract-registry-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("scratch directory should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/source-contract-v2/registry")
}

fn make_dir(path: impl AsRef<Path>) {
    fs::create_dir_all(path).expect("fixture directory should be created");
}

fn install_fixture(
    scratch: &ScratchDirectory,
    destination: impl AsRef<Path>,
    fixture: &str,
) -> PathBuf {
    let destination = scratch.path().join(destination);
    make_dir(
        destination
            .parent()
            .expect("fixture destination should have a parent"),
    );
    fs::copy(fixtures().join(fixture), &destination).expect("fixture should be installed");
    destination
}

fn install_candidate(scratch: &ScratchDirectory, destination: impl AsRef<Path>) -> PathBuf {
    let registry = install_fixture(scratch, destination, "valid-default.toml");
    make_dir(
        registry
            .parent()
            .expect("registry should have a parent")
            .join("alpha"),
    );
    registry
}

fn locator_for(scratch: &ScratchDirectory) -> RegistryLocator {
    let cwd = scratch.path().join("cwd");
    make_dir(&cwd);
    RegistryLocator {
        explicit: None,
        cwd,
        env_path: None,
        workspace_root: None,
        user_config: scratch.path().join("user/sources.toml"),
    }
}

fn resolve_ok(locator: &RegistryLocator) -> Registry {
    match resolve_registry(locator) {
        Ok(registry) => registry,
        Err(diagnostics) => panic!(
            "registry should resolve, but returned {} diagnostics",
            diagnostics.len()
        ),
    }
}

fn one_diagnostic(locator: &RegistryLocator) -> Diagnostic {
    let diagnostics = match resolve_registry(locator) {
        Ok(_) => panic!("registry resolution should fail"),
        Err(diagnostics) => diagnostics,
    };
    assert_eq!(
        diagnostics.len(),
        1,
        "the fixture should isolate one diagnostic"
    );
    diagnostics
        .into_iter()
        .next()
        .expect("one diagnostic should be present")
}

fn assert_diagnostic(
    diagnostic: &Diagnostic,
    code: &'static str,
    path: Option<&Path>,
    field: Option<&str>,
    message: &str,
) {
    assert_eq!(diagnostic.code, code);
    assert!(
        matches!(&diagnostic.severity, Severity::Error),
        "registry diagnostics are errors"
    );
    assert_eq!(diagnostic.path.as_deref(), path);
    assert_eq!(diagnostic.field.as_deref(), field);
    assert_eq!(diagnostic.message, message);
}

fn only_source(registry: &Registry) -> &SourceSpec {
    assert_eq!(registry.sources.len(), 1);
    registry
        .sources
        .values()
        .next()
        .expect("one source should be present")
}

fn canonical(path: impl AsRef<Path>) -> PathBuf {
    fs::canonicalize(path).expect("fixture path should canonicalize")
}

fn source_at_root<'a>(registry: &'a Registry, root: &Path) -> &'a SourceSpec {
    registry
        .sources
        .values()
        .find(|source| source.root.as_path() == root)
        .expect("source with expected root should be present")
}

fn resolved_relative_registry() -> (ScratchDirectory, PathBuf, Registry) {
    let scratch = ScratchDirectory::new();
    let registry_path = install_fixture(&scratch, "config/sources.toml", "valid-relative.toml");
    let origin = registry_path
        .parent()
        .expect("registry should have a parent");
    make_dir(origin.join("workspace/defaulted"));
    make_dir(origin.join("repos/custom-root"));
    make_dir(origin.join("repos/alias"));
    make_dir(origin.join("repos/vertical-root"));

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path.clone());
    let registry = resolve_ok(&locator);
    (scratch, registry_path, registry)
}

fn assert_main_fixture_diagnostic(
    fixture: &str,
    code: &'static str,
    field: Option<&str>,
    message: &str,
) {
    let scratch = ScratchDirectory::new();
    let registry_path = install_fixture(&scratch, "config/sources.toml", fixture);
    make_dir(scratch.path().join("config/root"));

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path.clone());
    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        code,
        Some(registry_path.as_path()),
        field,
        message,
    );
}

fn install_overlay_pair(
    scratch: &ScratchDirectory,
    main_fixture: &str,
    overlay_fixture: &str,
) -> (PathBuf, PathBuf) {
    let registry_path = install_fixture(scratch, "config/sources.toml", main_fixture);
    let overlay_path = install_fixture(scratch, "config/sources.local.toml", overlay_fixture);
    make_dir(scratch.path().join("config/roots/original"));
    make_dir(scratch.path().join("config/workspace/alpha"));
    make_dir(scratch.path().join("config/local/alpha"));
    make_dir(scratch.path().join("config/local/ghost"));
    (registry_path, overlay_path)
}

#[test]
fn explicit_registry_has_highest_precedence() {
    let scratch = ScratchDirectory::new();
    let explicit = install_candidate(&scratch, "explicit/registry.toml");
    let env = install_candidate(&scratch, "env/registry.toml");
    let workspace = install_candidate(&scratch, "workspace/kb-sources.toml");
    let user = install_candidate(&scratch, "user/sources.toml");

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(explicit.clone());
    locator.env_path = Some(env);
    locator.workspace_root = Some(
        workspace
            .parent()
            .expect("workspace registry should have a parent")
            .to_path_buf(),
    );
    locator.user_config = user;

    let registry = resolve_ok(&locator);
    assert_eq!(registry.origin, explicit);
}

#[test]
fn kb_sources_registry_precedes_workspace_and_user_candidates() {
    let scratch = ScratchDirectory::new();
    let env = install_candidate(&scratch, "env/registry.toml");
    let workspace = install_candidate(&scratch, "workspace/kb-sources.toml");
    let user = install_candidate(&scratch, "user/sources.toml");

    let mut locator = locator_for(&scratch);
    locator.env_path = Some(env.clone());
    locator.workspace_root = Some(
        workspace
            .parent()
            .expect("workspace registry should have a parent")
            .to_path_buf(),
    );
    locator.user_config = user;

    let registry = resolve_ok(&locator);
    assert_eq!(registry.origin, env);
}

#[test]
fn workspace_registry_precedes_the_user_candidate() {
    let scratch = ScratchDirectory::new();
    let workspace = install_candidate(&scratch, "workspace/kb-sources.toml");
    let user = install_candidate(&scratch, "user/sources.toml");

    let mut locator = locator_for(&scratch);
    locator.workspace_root = Some(
        workspace
            .parent()
            .expect("workspace registry should have a parent")
            .to_path_buf(),
    );
    locator.user_config = user;

    let registry = resolve_ok(&locator);
    assert_eq!(registry.origin, workspace);
}

#[test]
fn a_selected_workspace_registry_failure_does_not_fall_through_to_user_config() {
    let scratch = ScratchDirectory::new();
    let workspace = install_fixture(&scratch, "workspace/kb-sources.toml", "corrupt.toml");
    let user = install_candidate(&scratch, "user/sources.toml");

    let mut locator = locator_for(&scratch);
    locator.workspace_root = Some(
        workspace
            .parent()
            .expect("workspace registry should have a parent")
            .to_path_buf(),
    );
    locator.user_config = user;

    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-TOML",
        Some(workspace.as_path()),
        None,
        "source registry is not valid TOML",
    );
}

#[test]
fn user_registry_is_selected_when_the_workspace_candidate_is_absent() {
    let scratch = ScratchDirectory::new();
    let workspace_root = scratch.path().join("workspace");
    make_dir(&workspace_root);
    let user = install_candidate(&scratch, "user/sources.toml");

    let mut locator = locator_for(&scratch);
    locator.workspace_root = Some(workspace_root);
    locator.user_config = user.clone();

    let registry = resolve_ok(&locator);
    assert_eq!(registry.origin, user);
}

#[test]
fn a_missing_explicit_registry_fails_without_trying_lower_tiers() {
    let scratch = ScratchDirectory::new();
    let missing = scratch.path().join("explicit/missing.toml");
    let env = install_candidate(&scratch, "env/registry.toml");
    let workspace = install_candidate(&scratch, "workspace/kb-sources.toml");
    let user = install_candidate(&scratch, "user/sources.toml");

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(missing.clone());
    locator.env_path = Some(env);
    locator.workspace_root = Some(
        workspace
            .parent()
            .expect("workspace registry should have a parent")
            .to_path_buf(),
    );
    locator.user_config = user;

    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-NOT-FOUND",
        Some(missing.as_path()),
        None,
        "explicit registry does not exist",
    );
}

#[test]
fn a_missing_kb_sources_registry_fails_without_trying_lower_tiers() {
    let scratch = ScratchDirectory::new();
    let missing = scratch.path().join("env/missing.toml");
    let workspace = install_candidate(&scratch, "workspace/kb-sources.toml");
    let user = install_candidate(&scratch, "user/sources.toml");

    let mut locator = locator_for(&scratch);
    locator.env_path = Some(missing.clone());
    locator.workspace_root = Some(
        workspace
            .parent()
            .expect("workspace registry should have a parent")
            .to_path_buf(),
    );
    locator.user_config = user;

    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-NOT-FOUND",
        Some(missing.as_path()),
        None,
        "KB_SOURCES registry does not exist",
    );
}

#[test]
fn a_malformed_explicit_registry_fails_without_trying_kb_sources() {
    let scratch = ScratchDirectory::new();
    let corrupt = install_fixture(&scratch, "explicit/sources.toml", "corrupt.toml");
    let env = install_candidate(&scratch, "env/registry.toml");

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(corrupt.clone());
    locator.env_path = Some(env);

    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-TOML",
        Some(corrupt.as_path()),
        None,
        "source registry is not valid TOML",
    );
}

#[test]
fn a_malformed_kb_sources_registry_fails_without_trying_workspace() {
    let scratch = ScratchDirectory::new();
    let corrupt = install_fixture(&scratch, "env/sources.toml", "corrupt.toml");
    let workspace = install_candidate(&scratch, "workspace/kb-sources.toml");

    let mut locator = locator_for(&scratch);
    locator.env_path = Some(corrupt.clone());
    locator.workspace_root = Some(
        workspace
            .parent()
            .expect("workspace registry should have a parent")
            .to_path_buf(),
    );

    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-TOML",
        Some(corrupt.as_path()),
        None,
        "source registry is not valid TOML",
    );
}

#[test]
fn no_configured_candidate_does_not_search_cwd_ancestors() {
    let scratch = ScratchDirectory::new();
    let ancestor_registry = install_candidate(&scratch, "kb-sources.toml");
    assert_eq!(ancestor_registry, scratch.path().join("kb-sources.toml"));
    let cwd_registry = install_candidate(&scratch, "project/nested/kb-sources.toml");
    assert_eq!(
        cwd_registry,
        scratch.path().join("project/nested/kb-sources.toml")
    );

    let cwd = scratch.path().join("project/nested");
    make_dir(&cwd);
    let workspace_root = scratch.path().join("workspace");
    make_dir(&workspace_root);

    let mut locator = locator_for(&scratch);
    locator.cwd = cwd;
    locator.workspace_root = Some(workspace_root);
    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-NOT-FOUND",
        None,
        None,
        "no source registry exists in the configured workspace or user locations",
    );
}

#[test]
fn no_configured_candidate_does_not_use_a_builtin_docs_source() {
    let scratch = ScratchDirectory::new();
    let cwd = scratch.path().join("project/nested");
    make_dir(cwd.join("docs"));
    let workspace_root = scratch.path().join("workspace");
    make_dir(workspace_root.join("docs"));

    let mut locator = locator_for(&scratch);
    locator.cwd = cwd;
    locator.workspace_root = Some(workspace_root);
    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-NOT-FOUND",
        None,
        None,
        "no source registry exists in the configured workspace or user locations",
    );
}

#[test]
fn an_existing_candidate_that_is_not_readable_as_a_file_is_a_read_error() {
    let scratch = ScratchDirectory::new();
    let registry_path = scratch.path().join("explicit/sources.toml");
    make_dir(&registry_path);

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path.clone());
    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-READ",
        Some(registry_path.as_path()),
        None,
        "source registry could not be read",
    );
}

#[cfg(unix)]
#[test]
fn a_unix_special_file_is_a_read_error_without_parsing() {
    let scratch = ScratchDirectory::new();
    let registry_path = PathBuf::from("/dev/null");

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path.clone());
    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-READ",
        Some(registry_path.as_path()),
        None,
        "source registry could not be read",
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_to_a_regular_registry_is_accepted() {
    let scratch = ScratchDirectory::new();
    let target_path = install_candidate(&scratch, "explicit/target.toml");
    let registry_path = scratch.path().join("explicit/sources.toml");
    std::os::unix::fs::symlink(&target_path, &registry_path)
        .expect("registry symlink should be created");

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path.clone());
    let registry = resolve_ok(&locator);

    assert_eq!(registry.origin, registry_path);
    assert_eq!(
        only_source(&registry).root,
        canonical(scratch.path().join("explicit/alpha"))
    );
}

#[test]
fn relative_workspace_root_is_resolved_from_the_registry_origin() {
    let (_scratch, registry_path, registry) = resolved_relative_registry();
    let expected = canonical(
        registry_path
            .parent()
            .expect("registry should have a parent")
            .join("workspace/defaulted"),
    );

    assert_eq!(registry.origin, registry_path);
    assert_eq!(source_at_root(&registry, &expected).root, expected);
}

#[test]
fn an_explicit_relative_source_path_is_resolved_from_the_registry_origin() {
    let (_scratch, registry_path, registry) = resolved_relative_registry();
    let expected = canonical(
        registry_path
            .parent()
            .expect("registry should have a parent")
            .join("repos/custom-root"),
    );

    assert_eq!(source_at_root(&registry, &expected).root, expected);
}

#[test]
fn an_omitted_source_path_defaults_to_registry_origin_plus_logical_name() {
    let scratch = ScratchDirectory::new();
    let registry_path = install_candidate(&scratch, "config/sources.toml");
    let expected = canonical(scratch.path().join("config/alpha"));
    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path);
    let registry = resolve_ok(&locator);
    assert_eq!(only_source(&registry).root, expected);
}

#[test]
fn surface_defaults_to_vertical() {
    let (_scratch, registry_path, registry) = resolved_relative_registry();
    let root = canonical(
        registry_path
            .parent()
            .expect("registry should have a parent")
            .join("workspace/defaulted"),
    );

    assert!(matches!(
        &source_at_root(&registry, &root).surface,
        Surface::Vertical
    ));
}

#[test]
fn surface_accepts_core() {
    let (_scratch, registry_path, registry) = resolved_relative_registry();
    let root = canonical(
        registry_path
            .parent()
            .expect("registry should have a parent")
            .join("repos/custom-root"),
    );

    assert!(matches!(
        &source_at_root(&registry, &root).surface,
        Surface::Core
    ));
}

#[test]
fn surface_accepts_explicit_vertical() {
    let (_scratch, registry_path, registry) = resolved_relative_registry();
    let root = canonical(
        registry_path
            .parent()
            .expect("registry should have a parent")
            .join("repos/vertical-root"),
    );

    assert!(matches!(
        &source_at_root(&registry, &root).surface,
        Surface::Vertical
    ));
}

#[test]
fn legal_name_boundaries_are_accepted() {
    let scratch = ScratchDirectory::new();
    let registry_path = install_fixture(
        &scratch,
        "config/sources.toml",
        "valid-name-boundaries.toml",
    );
    for name in ["a", "a-", "a--b", "a0-b9"] {
        make_dir(scratch.path().join("config/roots").join(name));
    }

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path);
    let registry = resolve_ok(&locator);
    assert_eq!(registry.sources.len(), 4);
    for (name, source) in &registry.sources {
        assert!(name == &source.name, "map key should equal SourceSpec.name");
    }
}

#[test]
fn illegal_name_boundaries_are_rejected() {
    for fixture in [
        "invalid-name-empty.toml",
        "invalid-name-uppercase.toml",
        "invalid-name-digit-first.toml",
        "invalid-name-hyphen-first.toml",
        "invalid-name-underscore.toml",
        "invalid-name-dot.toml",
        "invalid-name-slash.toml",
        "invalid-name-backslash.toml",
        "invalid-name-space.toml",
        "invalid-name-non-ascii.toml",
    ] {
        assert_main_fixture_diagnostic(
            fixture,
            "KBV2-REGISTRY-INVALID-NAME",
            Some("source[0].name"),
            "source name must match ^[a-z][a-z0-9-]*$",
        );
    }
}

#[test]
fn duplicate_logical_names_are_rejected_at_the_later_row() {
    let scratch = ScratchDirectory::new();
    let registry_path = install_fixture(&scratch, "config/sources.toml", "duplicate-name.toml");
    make_dir(scratch.path().join("config/roots/first"));
    make_dir(scratch.path().join("config/roots/second"));

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path.clone());
    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-DUPLICATE-NAME",
        Some(registry_path.as_path()),
        Some("source[1].name"),
        "source name is duplicated",
    );
}

#[test]
fn corrupt_main_toml_has_a_stable_parse_diagnostic() {
    assert_main_fixture_diagnostic(
        "corrupt.toml",
        "KBV2-REGISTRY-TOML",
        None,
        "source registry is not valid TOML",
    );
}

#[test]
fn non_utf8_registry_content_is_a_toml_error() {
    let scratch = ScratchDirectory::new();
    let registry_path = scratch.path().join("config/sources.toml");
    make_dir(
        registry_path
            .parent()
            .expect("registry should have a parent"),
    );
    fs::write(&registry_path, [0xff, 0xfe]).expect("invalid UTF-8 fixture should be written");

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path.clone());
    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-TOML",
        Some(registry_path.as_path()),
        None,
        "source registry is not valid TOML",
    );
}

#[test]
fn an_unknown_main_registry_field_is_rejected() {
    assert_main_fixture_diagnostic(
        "unknown-main-field.toml",
        "KBV2-REGISTRY-UNKNOWN-FIELD",
        Some("version"),
        "registry field is not allowed by source-contract-v2",
    );
}

#[test]
fn an_unknown_source_row_field_is_rejected() {
    assert_main_fixture_diagnostic(
        "unknown-source-field.toml",
        "KBV2-REGISTRY-UNKNOWN-FIELD",
        Some("source[0].owner"),
        "registry field is not allowed by source-contract-v2",
    );
}

#[test]
fn legacy_is_a_removed_field_not_a_generic_unknown_field() {
    let scratch = ScratchDirectory::new();
    let registry_path = install_fixture(&scratch, "config/sources.toml", "removed-legacy.toml");
    make_dir(scratch.path().join("config/root"));

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path.clone());
    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-REMOVED-FIELD",
        Some(registry_path.as_path()),
        Some("source[0].legacy"),
        "registry field was removed in source-contract-v2",
    );
    assert_ne!(diagnostic.code, "KBV2-REGISTRY-UNKNOWN-FIELD");
}

#[test]
fn an_invalid_surface_is_rejected() {
    assert_main_fixture_diagnostic(
        "invalid-surface.toml",
        "KBV2-REGISTRY-INVALID-SURFACE",
        Some("source[0].surface"),
        "source surface must be `core` or `vertical`",
    );
}

#[test]
fn a_missing_source_name_has_a_stable_field_diagnostic() {
    assert_main_fixture_diagnostic(
        "missing-name.toml",
        "KBV2-REGISTRY-MISSING-FIELD",
        Some("source[0].name"),
        "required registry field is missing",
    );
}

#[test]
fn a_registry_without_source_rows_has_a_stable_field_diagnostic() {
    assert_main_fixture_diagnostic(
        "missing-source.toml",
        "KBV2-REGISTRY-MISSING-FIELD",
        Some("source"),
        "required registry field is missing",
    );
}

#[test]
fn known_fields_with_wrong_toml_types_are_rejected() {
    for (fixture, field) in [
        ("wrong-type-workspace-root.toml", "workspace_root"),
        ("wrong-type-source.toml", "source"),
        ("wrong-type-name.toml", "source[0].name"),
        ("wrong-type-path.toml", "source[0].path"),
        ("wrong-type-surface.toml", "source[0].surface"),
    ] {
        assert_main_fixture_diagnostic(
            fixture,
            "KBV2-REGISTRY-WRONG-TYPE",
            Some(field),
            "registry field has the wrong TOML type",
        );
    }
}

#[test]
fn a_local_overlay_can_replace_only_an_existing_sources_path() {
    let scratch = ScratchDirectory::new();
    let (registry_path, _) = install_overlay_pair(
        &scratch,
        "overlay-success-main.toml",
        "overlay-success-local.toml",
    );

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path.clone());
    let registry = resolve_ok(&locator);
    assert_eq!(registry.origin, registry_path);
    let source = only_source(&registry);
    assert_eq!(
        source.root,
        canonical(scratch.path().join("config/local/alpha"))
    );
    assert!(matches!(&source.surface, Surface::Core));
}

#[test]
fn a_local_overlay_row_requires_a_path() {
    let scratch = ScratchDirectory::new();
    let (registry_path, overlay_path) = install_overlay_pair(
        &scratch,
        "overlay-success-main.toml",
        "overlay-missing-path-local.toml",
    );

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path);
    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-MISSING-FIELD",
        Some(overlay_path.as_path()),
        Some("source[0].path"),
        "required registry field is missing",
    );
}

#[test]
fn a_local_overlay_cannot_add_an_unknown_logical_source() {
    let scratch = ScratchDirectory::new();
    let (registry_path, overlay_path) = install_overlay_pair(
        &scratch,
        "overlay-unknown-main.toml",
        "overlay-unknown-local.toml",
    );

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path);
    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-UNKNOWN-OVERLAY",
        Some(overlay_path.as_path()),
        Some("source[0].name"),
        "overlay source does not exist in the main registry",
    );
}

#[test]
fn a_local_overlay_rejects_top_level_known_and_unknown_row_fields() {
    for (overlay_fixture, field) in [
        ("overlay-forbidden-local.toml", "source[0].surface"),
        ("overlay-forbidden-main-field-local.toml", "workspace_root"),
        ("overlay-forbidden-unknown-local.toml", "source[0].owner"),
    ] {
        let scratch = ScratchDirectory::new();
        let (registry_path, overlay_path) =
            install_overlay_pair(&scratch, "overlay-forbidden-main.toml", overlay_fixture);

        let mut locator = locator_for(&scratch);
        locator.explicit = Some(registry_path);
        let diagnostic = one_diagnostic(&locator);
        assert_diagnostic(
            &diagnostic,
            "KBV2-REGISTRY-OVERLAY-FIELD",
            Some(overlay_path.as_path()),
            Some(field),
            "local overlay may override only source.path",
        );
    }
}

#[test]
fn legacy_in_a_local_overlay_is_still_a_removed_field() {
    let scratch = ScratchDirectory::new();
    let (registry_path, overlay_path) = install_overlay_pair(
        &scratch,
        "overlay-forbidden-main.toml",
        "overlay-removed-legacy-local.toml",
    );

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path);
    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-REMOVED-FIELD",
        Some(overlay_path.as_path()),
        Some("source[0].legacy"),
        "registry field was removed in source-contract-v2",
    );
    assert_ne!(diagnostic.code, "KBV2-REGISTRY-OVERLAY-FIELD");
    assert_ne!(diagnostic.code, "KBV2-REGISTRY-UNKNOWN-FIELD");
}

#[test]
fn corrupt_local_overlay_toml_is_not_ignored() {
    let scratch = ScratchDirectory::new();
    let (registry_path, overlay_path) = install_overlay_pair(
        &scratch,
        "overlay-corrupt-main.toml",
        "overlay-corrupt-local.toml",
    );

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path);
    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-TOML",
        Some(overlay_path.as_path()),
        None,
        "source registry is not valid TOML",
    );
}

#[test]
fn an_existing_local_overlay_that_is_not_a_file_is_a_read_error() {
    let scratch = ScratchDirectory::new();
    let registry_path =
        install_fixture(&scratch, "config/sources.toml", "overlay-success-main.toml");
    make_dir(scratch.path().join("config/workspace/alpha"));
    let overlay_path = scratch.path().join("config/sources.local.toml");
    make_dir(&overlay_path);

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path);
    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-READ",
        Some(overlay_path.as_path()),
        None,
        "source registry could not be read",
    );
}

#[test]
fn duplicate_roots_are_checked_after_local_overlay_paths_are_applied() {
    let scratch = ScratchDirectory::new();
    let (registry_path, _) = install_overlay_pair(
        &scratch,
        "overlay-duplicate-root-main.toml",
        "overlay-duplicate-root-local.toml",
    );
    make_dir(scratch.path().join("config/roots/alpha"));
    let duplicate_root = scratch.path().join("config/roots/beta");
    make_dir(&duplicate_root);

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path);
    let diagnostic = one_diagnostic(&locator);
    let canonical_root = canonical(&duplicate_root);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-DUPLICATE-ROOT",
        Some(canonical_root.as_path()),
        Some("source[1].path"),
        "canonical source root is already registered",
    );
}

#[test]
fn a_missing_physical_source_root_is_rejected() {
    let scratch = ScratchDirectory::new();
    let registry_path = install_fixture(&scratch, "config/sources.toml", "missing-root.toml");
    let missing_root = scratch.path().join("config/roots/missing");

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path);
    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-SOURCE-ROOT",
        Some(missing_root.as_path()),
        Some("source[0].path"),
        "source root must be an existing directory",
    );
}

#[test]
fn a_regular_file_cannot_be_a_physical_source_root() {
    let scratch = ScratchDirectory::new();
    let registry_path = install_fixture(&scratch, "config/sources.toml", "file-root.toml");
    let file_root = scratch.path().join("config/roots/file");
    make_dir(
        file_root
            .parent()
            .expect("source-root file should have a parent"),
    );
    fs::write(&file_root, b"not a directory").expect("source-root fixture file should be written");

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path);
    let diagnostic = one_diagnostic(&locator);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-SOURCE-ROOT",
        Some(file_root.as_path()),
        Some("source[0].path"),
        "source root must be an existing directory",
    );
}

#[test]
fn lexical_aliases_of_the_same_canonical_root_are_duplicates() {
    let scratch = ScratchDirectory::new();
    let registry_path = install_fixture(&scratch, "config/sources.toml", "duplicate-root.toml");
    let shared_root = scratch.path().join("config/roots/shared");
    make_dir(&shared_root);

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path);
    let diagnostic = one_diagnostic(&locator);
    let canonical_root = canonical(&shared_root);
    assert_diagnostic(
        &diagnostic,
        "KBV2-REGISTRY-DUPLICATE-ROOT",
        Some(canonical_root.as_path()),
        Some("source[1].path"),
        "canonical source root is already registered",
    );
}

#[test]
fn registry_iteration_is_deterministic_by_logical_source_name() {
    let scratch = ScratchDirectory::new();
    let registry_path =
        install_fixture(&scratch, "config/sources.toml", "deterministic-order.toml");
    for root in ["a-root", "z-root", "m-root"] {
        make_dir(scratch.path().join("config/roots").join(root));
    }

    let mut locator = locator_for(&scratch);
    locator.explicit = Some(registry_path);
    let registry = resolve_ok(&locator);
    let entries: Vec<(&SourceName, &SourceSpec)> = registry.sources.iter().collect();
    let ordered_root_names: Vec<&str> = entries
        .iter()
        .map(|(_, source)| {
            source
                .root
                .file_name()
                .and_then(|name| name.to_str())
                .expect("fixture roots should have UTF-8 names")
        })
        .collect();

    assert_eq!(ordered_root_names, ["z-root", "m-root", "a-root"]);
    for (name, source) in entries {
        assert!(name == &source.name, "map key should equal SourceSpec.name");
    }
}
