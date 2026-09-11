use kb_contract::{Diagnostic, DomainId, Severity};
use std::collections::BTreeMap;

const INVALID_DOMAIN_MESSAGE: &str = "domain must contain only safe non-empty path segments";

fn assert_invalid_domain(raw: &str) {
    let diagnostic = match DomainId::new(raw) {
        Ok(domain) => panic!("{raw:?} should be rejected, got {domain}"),
        Err(diagnostic) => diagnostic,
    };

    assert_domain_diagnostic(&diagnostic);
}

fn assert_domain_diagnostic(diagnostic: &Diagnostic) {
    assert_eq!(diagnostic.code, "KBV2-DOMAIN-INVALID");
    assert_eq!(diagnostic.severity, Severity::Error);
    assert_eq!(diagnostic.path, None);
    assert_eq!(diagnostic.field.as_deref(), Some("domain"));
    assert_eq!(diagnostic.message, INVALID_DOMAIN_MESSAGE);
}

#[test]
fn domain_accepts_ascii_unicode_and_two_digit_cjk_segments() {
    let cases = [
        ("widgets/blue", "widgets/blue"),
        ("api.v2/reference", "api.v2/reference"),
        ("10-知识笔记/20-系统设计", "10-知识笔记/20-系统设计"),
        ("Straße/Συστήματα", "Straße/Συστήματα"),
    ];

    for (raw, expected) in cases {
        let domain = DomainId::new(raw).expect("safe domain should be accepted");
        assert_eq!(domain.as_str(), expected, "raw domain: {raw:?}");
    }
}

#[test]
fn domain_normalizes_each_segment_to_nfc_without_changing_display_case() {
    let domain = DomainId::new("Cafe\u{301}/Re\u{301}sume\u{301}/Straße")
        .expect("decomposed Unicode should be accepted");

    assert_eq!(domain.as_str(), "Café/Résumé/Straße");
    assert_eq!(domain.to_string(), "Café/Résumé/Straße");
}

#[test]
fn domain_rejects_empty_escape_delimiter_and_control_segments() {
    let invalid = [
        "",
        ".",
        "..",
        "/topic",
        "topic/",
        "topic//child",
        "topic/./child",
        "topic/../child",
        "topic\\child",
        "topic:child",
        "topic/child:leaf",
        "\u{0000}",
        "topic/line\nfeed",
        "topic/\u{007f}",
        "topic/\u{0085}control",
    ];

    for raw in invalid {
        assert_invalid_domain(raw);
    }
}

#[test]
fn domain_supports_display_and_borrowed_string_lookup() {
    let domain =
        DomainId::new("10-知识笔记/20-系统设计").expect("consumer fixture domain should be valid");
    let mut by_domain = BTreeMap::new();
    by_domain.insert(domain, "found");

    let stored = by_domain
        .keys()
        .next()
        .expect("one domain should be stored");
    assert_eq!(stored.as_str(), "10-知识笔记/20-系统设计");
    assert_eq!(format!("{stored}"), "10-知识笔记/20-系统设计");
    assert_eq!(by_domain.get("10-知识笔记/20-系统设计"), Some(&"found"));
}
