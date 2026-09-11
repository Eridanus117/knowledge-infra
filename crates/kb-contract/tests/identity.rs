use kb_contract::{
    Diagnostic, DomainId, Identity, Registry, RegistryLocator, Severity, SourceName,
    derive_identity, resolve_registry,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

const INVALID_SLUG_MESSAGE: &str = "note slug must be one safe non-empty path segment";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/source-contract-v2/identity")
}

fn logical_registry() -> Registry {
    let root = fixtures();
    let registry_path = root.join("logical-source.toml");
    let locator = RegistryLocator {
        explicit: Some(registry_path),
        cwd: root.clone(),
        env_path: None,
        workspace_root: None,
        user_config: root.join("unused-user-registry.toml"),
    };

    match resolve_registry(&locator) {
        Ok(registry) => registry,
        Err(diagnostics) => panic!(
            "logical-source fixture should resolve, but returned {} diagnostics",
            diagnostics.len()
        ),
    }
}

fn logical_source_name() -> SourceName {
    logical_registry()
        .sources
        .into_values()
        .next()
        .expect("logical-source fixture should contain one source")
        .name
}

fn assert_invalid_slug(raw: &str) {
    let source = logical_source_name();
    let domain = DomainId::new("10-知识笔记").expect("fixture domain should be valid");
    let diagnostic = match derive_identity(&source, &domain, raw) {
        Ok(identity) => panic!("{raw:?} should be rejected, got {identity}"),
        Err(diagnostic) => diagnostic,
    };

    assert_slug_diagnostic(&diagnostic);
}

fn assert_slug_diagnostic(diagnostic: &Diagnostic) {
    assert_eq!(diagnostic.code, "KBV2-IDENTITY-INVALID-SLUG");
    assert_eq!(diagnostic.severity, Severity::Error);
    assert_eq!(diagnostic.path, None);
    assert_eq!(diagnostic.field.as_deref(), Some("slug"));
    assert_eq!(diagnostic.message, INVALID_SLUG_MESSAGE);
}

#[test]
fn identity_uses_the_logical_source_not_the_physical_root_name() {
    let registry = logical_registry();
    let source = registry
        .sources
        .values()
        .next()
        .expect("logical-source fixture should contain one source");
    assert_eq!(
        source.root.file_name().and_then(|name| name.to_str()),
        Some("desk")
    );
    let domain = DomainId::new("widgets/blue").expect("fixture domain should be valid");

    let identity = derive_identity(&source.name, &domain, "ap-gap")
        .expect("safe slug should derive an identity");

    assert_eq!(identity.as_str(), "knowledge:widgets/blue:ap-gap");
    assert!(!identity.as_str().starts_with("desk:"));
}

#[test]
fn identity_accepts_ascii_unicode_two_digit_cjk_and_dotted_slugs() {
    let source = logical_source_name();
    let cases = [
        ("widgets", "ap-gap", "knowledge:widgets:ap-gap"),
        (
            "10-知识笔记/20-系统设计",
            "10-缓存一致性",
            "knowledge:10-知识笔记/20-系统设计:10-缓存一致性",
        ),
        (
            "Référence",
            "v1.2-Straße",
            "knowledge:Référence:v1.2-Straße",
        ),
    ];

    for (domain, slug, expected) in cases {
        let domain = DomainId::new(domain).expect("fixture domain should be valid");
        let identity =
            derive_identity(&source, &domain, slug).expect("safe slug should derive an identity");
        assert_eq!(identity.as_str(), expected, "slug: {slug:?}");
    }
}

#[test]
fn identity_normalizes_the_slug_to_nfc_and_preserves_display_case() {
    let source = logical_source_name();
    let domain = DomainId::new("Cafe\u{301}/Re\u{301}sume\u{301}")
        .expect("decomposed domain should be valid");

    let identity = derive_identity(&source, &domain, "Straße-Cafe\u{301}")
        .expect("decomposed slug should be valid");

    assert_eq!(identity.as_str(), "knowledge:Café/Résumé:Straße-Café");
    assert_eq!(identity.to_string(), "knowledge:Café/Résumé:Straße-Café");
}

#[test]
fn identity_rejects_empty_escape_delimiter_and_control_slugs() {
    let invalid = [
        "",
        ".",
        "..",
        "/note",
        "note/",
        "note/child",
        "note//child",
        "note\\child",
        "note:child",
        "\u{0000}",
        "line\nfeed",
        "\u{007f}",
        "\u{0085}control",
    ];

    for raw in invalid {
        assert_invalid_slug(raw);
    }
}

#[test]
fn opaque_values_support_display_and_borrowed_string_lookup() {
    let registry = logical_registry();
    let source = registry
        .sources
        .get("knowledge")
        .expect("SourceName should support borrowed str lookup");
    assert_eq!(source.name.as_str(), "knowledge");
    assert_eq!(format!("{}", source.name), "knowledge");

    let domain = DomainId::new("10-知识笔记").expect("fixture domain should be valid");
    let identity = derive_identity(&source.name, &domain, "20-系统设计")
        .expect("fixture identity should derive");
    let mut by_identity: BTreeMap<Identity, &str> = BTreeMap::new();
    by_identity.insert(identity, "found");

    let stored = by_identity
        .keys()
        .next()
        .expect("one identity should be stored");
    assert_eq!(stored.as_str(), "knowledge:10-知识笔记:20-系统设计");
    assert_eq!(format!("{stored}"), "knowledge:10-知识笔记:20-系统设计");
    assert_eq!(
        by_identity.get("knowledge:10-知识笔记:20-系统设计"),
        Some(&"found")
    );
}
