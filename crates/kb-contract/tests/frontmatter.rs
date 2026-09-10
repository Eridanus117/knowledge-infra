use kb_contract::{
    Diagnostic, NoteFrontmatter, NoteKind, NoteStatus, Severity, ValidatedNote,
    parse_and_validate_note, render_note,
};
use std::path::{Path, PathBuf};

const FIXTURE_ROOT: &str = "fixtures/source-contract-v2/frontmatter";

const MESSAGE_UTF8: &str = "note is not valid UTF-8";
const MESSAGE_MISSING_FRONTMATTER: &str = "note must begin with a frontmatter fence";
const MESSAGE_UNTERMINATED_FRONTMATTER: &str = "frontmatter opening fence has no closing fence";
const MESSAGE_SYNTAX: &str = "frontmatter does not match the controlled flat grammar";
const MESSAGE_DUPLICATE_FIELD: &str = "frontmatter field is duplicated";
const MESSAGE_MISSING_FIELD: &str = "required note field is missing";
const MESSAGE_EMPTY_FIELD: &str = "note field must not be empty";
const MESSAGE_WRONG_TYPE: &str = "note field has the wrong type";
const MESSAGE_UNKNOWN_FIELD: &str = "note field is not allowed by source-contract-v2";
const MESSAGE_REMOVED_FIELD: &str = "note field was removed in source-contract-v2";
const MESSAGE_INVALID_KIND: &str =
    "note kind must be spec, reference, runbook, decision, research, note, or index";
const MESSAGE_INVALID_STATUS: &str = "note status must be `frozen`";
const MESSAGE_FIELD_NOT_ALLOWED: &str = "note field is allowed only for kind `decision`";
const MESSAGE_INVALID_VALUE: &str = "note field has a value not allowed by source-contract-v2";

macro_rules! fixture {
    ($name:literal) => {
        include_bytes!(concat!(
            "../../../fixtures/source-contract-v2/frontmatter/",
            $name
        ))
        .as_slice()
    };
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(FIXTURE_ROOT).join(name)
}

fn parse_ok(name: &str, bytes: &[u8]) -> ValidatedNote {
    let path = fixture_path(name);
    let note = match parse_and_validate_note(path.as_path(), bytes) {
        Ok(note) => note,
        Err(diagnostics) => panic!(
            "{name} should be valid, but returned {} diagnostics: {diagnostics:?}",
            diagnostics.len()
        ),
    };
    assert_eq!(
        note.original.as_slice(),
        bytes,
        "{name} must retain every original input byte"
    );
    note
}

fn diagnostics_for(name: &str, bytes: &[u8]) -> Vec<Diagnostic> {
    let path = fixture_path(name);
    match parse_and_validate_note(path.as_path(), bytes) {
        Ok(_) => panic!("{name} should be rejected"),
        Err(diagnostics) => diagnostics,
    }
}

fn assert_diagnostic(
    diagnostic: &Diagnostic,
    path: &Path,
    code: &'static str,
    field: Option<&str>,
    message: &str,
) {
    assert_eq!(diagnostic.code, code);
    assert!(
        matches!(diagnostic.severity, Severity::Error),
        "note contract diagnostics are errors"
    );
    assert_eq!(diagnostic.path.as_deref(), Some(path));
    assert_eq!(diagnostic.field.as_deref(), field);
    assert_eq!(diagnostic.message, message);
}

fn assert_one_diagnostic(
    name: &str,
    bytes: &[u8],
    code: &'static str,
    field: Option<&str>,
    message: &str,
) {
    let path = fixture_path(name);
    let diagnostics = diagnostics_for(name, bytes);
    assert_eq!(
        diagnostics.len(),
        1,
        "{name} isolates one public diagnostic"
    );
    assert_diagnostic(&diagnostics[0], path.as_path(), code, field, message);
}

fn decode_hex_fixture(bytes: &[u8]) -> Vec<u8> {
    let encoded = std::str::from_utf8(bytes).expect("hex fixture should be ASCII");
    let digits: Vec<u8> = encoded
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    assert_eq!(digits.len() % 2, 0, "hex fixture needs complete bytes");
    digits
        .chunks_exact(2)
        .map(|pair| (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]))
        .collect()
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => panic!("hex fixture contains a non-hex digit"),
    }
}

fn kind_name(kind: &NoteKind) -> &'static str {
    match kind {
        NoteKind::Spec => "spec",
        NoteKind::Reference => "reference",
        NoteKind::Runbook => "runbook",
        NoteKind::Decision => "decision",
        NoteKind::Research => "research",
        NoteKind::Note => "note",
        NoteKind::Index => "index",
    }
}

fn frontmatter(description: &str, keywords: &[&str], kind: NoteKind) -> NoteFrontmatter {
    NoteFrontmatter {
        description: description.to_owned(),
        keywords: keywords.iter().map(|value| (*value).to_owned()).collect(),
        kind,
        links: Vec::new(),
        code: Vec::new(),
        assets: Vec::new(),
        supersedes: None,
        status: None,
    }
}

#[test]
fn minimal_lf_note_defaults_kind_and_preserves_original_and_body_bytes() {
    let name = "valid-minimal-lf.md";
    let bytes = fixture!("valid-minimal-lf.md");
    let note = parse_ok(name, bytes);

    assert_eq!(
        note.frontmatter.description,
        "Minimal note body has no heading"
    );
    assert_eq!(note.frontmatter.keywords, ["minimal", "body"]);
    assert_eq!(kind_name(&note.frontmatter.kind), "note");
    assert!(
        !note.kind_explicit,
        "an omitted kind must remain observable"
    );
    assert!(note.frontmatter.links.is_empty());
    assert!(note.frontmatter.code.is_empty());
    assert!(note.frontmatter.assets.is_empty());
    assert_eq!(note.frontmatter.supersedes, None);
    assert!(note.frontmatter.status.is_none());
    assert_eq!(
        note.body,
        b"\nPlain body without a heading.\nSecond line.\n"
    );
}

#[test]
fn utf8_bom_is_semantically_ignored_but_retained_in_original() {
    let name = "valid-bom.md";
    let bytes = fixture!("valid-bom.md");
    assert!(bytes.starts_with(&[0xef, 0xbb, 0xbf]));

    let note = parse_ok(name, bytes);
    assert_eq!(note.frontmatter.description, "BOM-safe note");
    assert_eq!(note.frontmatter.keywords, ["bom", "UTF8"]);
    assert_eq!(kind_name(&note.frontmatter.kind), "note");
    assert!(!note.kind_explicit);
    assert_eq!(note.body, b"\nBody after BOM.\n");
}

#[test]
fn crlf_frontmatter_parses_while_body_and_original_stay_byte_exact() {
    let name = "valid-crlf.md";
    let bytes = fixture!("valid-crlf.md");
    assert!(bytes.windows(2).any(|window| window == b"\r\n"));

    let note = parse_ok(name, bytes);
    assert_eq!(note.frontmatter.description, "CRLF note");
    assert_eq!(note.frontmatter.keywords, ["crlf"]);
    assert_eq!(kind_name(&note.frontmatter.kind), "note");
    assert!(note.kind_explicit);
    assert_eq!(note.body, b"\r\nBody line one.\r\nBody line two.\r\n");
}

#[test]
fn flow_and_block_lists_decode_to_the_same_typed_vectors() {
    let flow = parse_ok("valid-flow-list.md", fixture!("valid-flow-list.md"));
    assert_eq!(flow.frontmatter.keywords, ["alpha", "two words", "owner's"]);
    assert_eq!(flow.frontmatter.links, ["first-note", "second note"]);
    assert_eq!(flow.frontmatter.code, ["repo/src/lib.rs", "module:member"]);

    let block = parse_ok("valid-block-list.md", fixture!("valid-block-list.md"));
    assert_eq!(block.frontmatter.keywords, ["alpha", "two words"]);
    assert_eq!(block.frontmatter.links, ["first-note"]);
    assert_eq!(block.frontmatter.code, ["repo/src/lib.rs"]);
}

#[test]
fn quotes_escapes_hashes_and_inline_comments_have_controlled_meanings() {
    let note = parse_ok(
        "valid-quotes-comments.md",
        fixture!("valid-quotes-comments.md"),
    );

    assert_eq!(note.frontmatter.description, "owner's \"quoted\" # value");
    assert_eq!(
        note.frontmatter.keywords,
        ["slash\\path", "say \"hello\"", "it's", "repo/file#section"]
    );
    assert!(note.kind_explicit);
}

#[test]
fn paired_utf16_surrogates_decode_to_one_unicode_scalar() {
    let note = parse_ok(
        "valid-surrogate-pair.md",
        fixture!("valid-surrogate-pair.md"),
    );
    assert_eq!(note.frontmatter.description, "Paired scalar \u{1d11e}");
}

#[test]
fn escaped_feff_roundtrips_through_its_canonical_ascii_escape() {
    let note = parse_ok("valid-escaped-feff.md", fixture!("valid-escaped-feff.md"));
    assert_eq!(note.frontmatter.description, "Escaped \u{feff} marker");

    let rendered = render_note(&note.frontmatter, &note.body);
    assert!(
        !rendered.windows(3).any(|window| window == b"\xef\xbb\xbf"),
        "canonical frontmatter must not contain raw U+FEFF bytes"
    );
    let rendered_text =
        std::str::from_utf8(&rendered).expect("canonical rendering should remain UTF-8");
    assert!(rendered_text.contains("\\ufeff"));

    let reparsed = parse_and_validate_note(Path::new("roundtrip-feff.md"), &rendered)
        .expect("canonical escaped U+FEFF should parse again");
    assert_eq!(reparsed.frontmatter, note.frontmatter);
}

#[test]
fn multiline_flow_list_consumes_only_its_controlled_continuations() {
    let note = parse_ok(
        "valid-multiline-flow-list.md",
        fixture!("valid-multiline-flow-list.md"),
    );
    assert_eq!(note.frontmatter.keywords, ["alpha", "two words", "owner's"]);
    assert_eq!(kind_name(&note.frontmatter.kind), "note");
}

#[test]
fn literal_and_folded_block_scalars_use_the_documented_folding_rules() {
    let literal = parse_ok("valid-block-literal.md", fixture!("valid-block-literal.md"));
    assert_eq!(literal.frontmatter.description, "one literal line");

    let folded = parse_ok("valid-block-folded.md", fixture!("valid-block-folded.md"));
    assert_eq!(folded.frontmatter.description, "one folded description");
}

#[test]
fn all_seven_explicit_kinds_map_to_the_public_enum() {
    let cases = [
        ("valid-kind-spec.md", fixture!("valid-kind-spec.md"), "spec"),
        (
            "valid-kind-reference.md",
            fixture!("valid-kind-reference.md"),
            "reference",
        ),
        (
            "valid-kind-runbook.md",
            fixture!("valid-kind-runbook.md"),
            "runbook",
        ),
        (
            "valid-kind-decision.md",
            fixture!("valid-kind-decision.md"),
            "decision",
        ),
        (
            "valid-kind-research.md",
            fixture!("valid-kind-research.md"),
            "research",
        ),
        ("valid-kind-note.md", fixture!("valid-kind-note.md"), "note"),
        (
            "valid-kind-index.md",
            fixture!("valid-kind-index.md"),
            "index",
        ),
    ];

    for (name, bytes, expected) in cases {
        let note = parse_ok(name, bytes);
        assert_eq!(kind_name(&note.frontmatter.kind), expected, "{name}");
        assert!(note.kind_explicit, "{name} writes kind explicitly");
    }
}

#[test]
fn links_and_code_require_lists_of_nonempty_strings() {
    let note = parse_ok("valid-links-code.md", fixture!("valid-links-code.md"));
    assert_eq!(note.frontmatter.links, ["alpha", "two words"]);
    assert_eq!(note.frontmatter.code, ["repo/src/lib.rs", "module:member"]);
}

#[test]
fn decision_accepts_assets_and_supersedes() {
    let note = parse_ok(
        "valid-decision-fields.md",
        fixture!("valid-decision-fields.md"),
    );
    assert_eq!(kind_name(&note.frontmatter.kind), "decision");
    assert_eq!(
        note.frontmatter.assets,
        ["repo:service@main", "svc:Example#run"]
    );
    assert_eq!(
        note.frontmatter.supersedes.as_deref(),
        Some("adr-001-old-choice")
    );
}

#[test]
fn exact_frozen_status_maps_to_the_only_status_variant() {
    let note = parse_ok("valid-frozen-status.md", fixture!("valid-frozen-status.md"));
    assert!(matches!(
        note.frontmatter.status.as_ref(),
        Some(NoteStatus::Frozen)
    ));
}

#[test]
fn invalid_utf8_fails_before_frontmatter_parsing() {
    let name = "invalid-utf8.hex";
    let bytes = decode_hex_fixture(fixture!("invalid-utf8.hex"));
    assert!(std::str::from_utf8(&bytes).is_err());
    assert_one_diagnostic(name, &bytes, "KBV2-NOTE-UTF8", None, MESSAGE_UTF8);
}

#[test]
fn missing_and_unterminated_fences_have_distinct_envelope_diagnostics() {
    let cases = [
        (
            "invalid-missing-frontmatter.md",
            fixture!("invalid-missing-frontmatter.md"),
            "KBV2-NOTE-FRONTMATTER-MISSING",
            MESSAGE_MISSING_FRONTMATTER,
        ),
        (
            "invalid-unterminated-frontmatter.md",
            fixture!("invalid-unterminated-frontmatter.md"),
            "KBV2-NOTE-FRONTMATTER-UNTERMINATED",
            MESSAGE_UNTERMINATED_FRONTMATTER,
        ),
    ];

    for (name, bytes, code, message) in cases {
        assert_one_diagnostic(name, bytes, code, None, message);
    }
}

#[test]
fn frontmatter_over_one_mib_fails_before_line_collection() {
    let name = "oversized-frontmatter.md";
    let mut bytes = b"---\ndescription: ".to_vec();
    bytes.resize(bytes.len() + 1024 * 1024, b'x');
    bytes.extend_from_slice(b"\nkeywords: [limit]\n---\nBody is not size-capped.\n");

    assert_one_diagnostic(
        name,
        &bytes,
        "KBV2-NOTE-FRONTMATTER-SYNTAX",
        None,
        MESSAGE_SYNTAX,
    );
}

#[test]
fn unsupported_flat_grammar_constructs_share_one_sanitized_syntax_diagnostic() {
    let cases = [
        (
            "invalid-syntax-stray-line.md",
            fixture!("invalid-syntax-stray-line.md"),
        ),
        (
            "invalid-syntax-indented-line.md",
            fixture!("invalid-syntax-indented-line.md"),
        ),
        (
            "invalid-syntax-nested-mapping.md",
            fixture!("invalid-syntax-nested-mapping.md"),
        ),
        (
            "invalid-syntax-unparseable-line.md",
            fixture!("invalid-syntax-unparseable-line.md"),
        ),
        (
            "invalid-syntax-anchor.md",
            fixture!("invalid-syntax-anchor.md"),
        ),
        (
            "invalid-syntax-alias.md",
            fixture!("invalid-syntax-alias.md"),
        ),
        (
            "invalid-syntax-multidoc.md",
            fixture!("invalid-syntax-multidoc.md"),
        ),
        (
            "invalid-syntax-unclosed-quote.md",
            fixture!("invalid-syntax-unclosed-quote.md"),
        ),
        (
            "invalid-syntax-unclosed-flow-list.md",
            fixture!("invalid-syntax-unclosed-flow-list.md"),
        ),
        (
            "invalid-syntax-lone-high-surrogate.md",
            fixture!("invalid-syntax-lone-high-surrogate.md"),
        ),
        (
            "invalid-syntax-lone-low-surrogate.md",
            fixture!("invalid-syntax-lone-low-surrogate.md"),
        ),
        (
            "invalid-syntax-wrong-low-surrogate.md",
            fixture!("invalid-syntax-wrong-low-surrogate.md"),
        ),
        (
            "invalid-syntax-raw-feff.md",
            fixture!("invalid-syntax-raw-feff.md"),
        ),
    ];

    for (name, bytes) in cases {
        assert_one_diagnostic(
            name,
            bytes,
            "KBV2-NOTE-FRONTMATTER-SYNTAX",
            None,
            MESSAGE_SYNTAX,
        );
    }
}

#[test]
fn duplicate_key_names_the_repeated_field_without_parser_details() {
    assert_one_diagnostic(
        "invalid-duplicate-field.md",
        fixture!("invalid-duplicate-field.md"),
        "KBV2-NOTE-DUPLICATE-FIELD",
        Some("description"),
        MESSAGE_DUPLICATE_FIELD,
    );
}

#[test]
fn required_fields_distinguish_missing_from_empty() {
    let missing = [
        (
            "invalid-missing-description.md",
            fixture!("invalid-missing-description.md"),
            "description",
        ),
        (
            "invalid-missing-keywords.md",
            fixture!("invalid-missing-keywords.md"),
            "keywords",
        ),
    ];
    for (name, bytes, field) in missing {
        assert_one_diagnostic(
            name,
            bytes,
            "KBV2-NOTE-MISSING-FIELD",
            Some(field),
            MESSAGE_MISSING_FIELD,
        );
    }

    let empty = [
        (
            "invalid-empty-description.md",
            fixture!("invalid-empty-description.md"),
            "description",
        ),
        (
            "invalid-empty-keywords.md",
            fixture!("invalid-empty-keywords.md"),
            "keywords",
        ),
    ];
    for (name, bytes, field) in empty {
        assert_one_diagnostic(
            name,
            bytes,
            "KBV2-NOTE-EMPTY-FIELD",
            Some(field),
            MESSAGE_EMPTY_FIELD,
        );
    }
}

#[test]
fn multiline_description_is_a_value_error_after_literal_decoding() {
    assert_one_diagnostic(
        "invalid-multiline-description.md",
        fixture!("invalid-multiline-description.md"),
        "KBV2-NOTE-INVALID-VALUE",
        Some("description"),
        MESSAGE_INVALID_VALUE,
    );
}

#[test]
fn explicit_null_is_not_the_same_as_an_omitted_field() {
    assert_one_diagnostic(
        "invalid-null-description.md",
        fixture!("invalid-null-description.md"),
        "KBV2-NOTE-WRONG-TYPE",
        Some("description"),
        MESSAGE_WRONG_TYPE,
    );
    assert_one_diagnostic(
        "invalid-status-null.md",
        fixture!("invalid-status-null.md"),
        "KBV2-NOTE-WRONG-TYPE",
        Some("status"),
        MESSAGE_WRONG_TYPE,
    );
}

#[test]
fn every_public_field_rejects_the_wrong_container_type() {
    let name = "invalid-wrong-types.md";
    let path = fixture_path(name);
    let diagnostics = diagnostics_for(name, fixture!("invalid-wrong-types.md"));
    let expected_fields = [
        "description",
        "keywords",
        "links",
        "code",
        "assets",
        "supersedes",
        "status",
    ];

    assert_eq!(diagnostics.len(), expected_fields.len());
    for (diagnostic, field) in diagnostics.iter().zip(expected_fields) {
        assert_diagnostic(
            diagnostic,
            path.as_path(),
            "KBV2-NOTE-WRONG-TYPE",
            Some(field),
            MESSAGE_WRONG_TYPE,
        );
    }
}

#[test]
fn null_and_empty_items_are_rejected_in_every_list_field() {
    let name = "invalid-list-items.md";
    let path = fixture_path(name);
    let diagnostics = diagnostics_for(name, fixture!("invalid-list-items.md"));
    let expected = [
        ("KBV2-NOTE-WRONG-TYPE", "keywords[1]", MESSAGE_WRONG_TYPE),
        ("KBV2-NOTE-EMPTY-FIELD", "keywords[2]", MESSAGE_EMPTY_FIELD),
        ("KBV2-NOTE-WRONG-TYPE", "links[1]", MESSAGE_WRONG_TYPE),
        ("KBV2-NOTE-EMPTY-FIELD", "links[2]", MESSAGE_EMPTY_FIELD),
        ("KBV2-NOTE-WRONG-TYPE", "code[1]", MESSAGE_WRONG_TYPE),
        ("KBV2-NOTE-EMPTY-FIELD", "code[2]", MESSAGE_EMPTY_FIELD),
        ("KBV2-NOTE-WRONG-TYPE", "assets[1]", MESSAGE_WRONG_TYPE),
        ("KBV2-NOTE-EMPTY-FIELD", "assets[2]", MESSAGE_EMPTY_FIELD),
    ];

    assert_eq!(diagnostics.len(), expected.len());
    for (diagnostic, (code, field, message)) in diagnostics.iter().zip(expected) {
        assert_diagnostic(diagnostic, path.as_path(), code, Some(field), message);
    }
}

#[test]
fn invalid_kind_value_and_wrong_kind_type_are_distinct() {
    assert_one_diagnostic(
        "invalid-kind.md",
        fixture!("invalid-kind.md"),
        "KBV2-NOTE-INVALID-KIND",
        Some("kind"),
        MESSAGE_INVALID_KIND,
    );
    assert_one_diagnostic(
        "invalid-kind-wrong-type.md",
        fixture!("invalid-kind-wrong-type.md"),
        "KBV2-NOTE-WRONG-TYPE",
        Some("kind"),
        MESSAGE_WRONG_TYPE,
    );
}

#[test]
fn each_decision_only_field_fails_on_a_non_decision() {
    let cases = [
        (
            "invalid-assets-non-decision.md",
            fixture!("invalid-assets-non-decision.md"),
            "assets",
        ),
        (
            "invalid-supersedes-non-decision.md",
            fixture!("invalid-supersedes-non-decision.md"),
            "supersedes",
        ),
    ];

    for (name, bytes, field) in cases {
        assert_one_diagnostic(
            name,
            bytes,
            "KBV2-NOTE-FIELD-NOT-ALLOWED",
            Some(field),
            MESSAGE_FIELD_NOT_ALLOWED,
        );
    }
}

#[test]
fn unknown_field_has_its_own_stable_classification() {
    assert_one_diagnostic(
        "invalid-unknown-field.md",
        fixture!("invalid-unknown-field.md"),
        "KBV2-NOTE-UNKNOWN-FIELD",
        Some("owner"),
        MESSAGE_UNKNOWN_FIELD,
    );
}

#[test]
fn every_killed_v1_field_is_classified_as_removed() {
    let name = "invalid-removed-killed-fields.md";
    let path = fixture_path(name);
    let diagnostics = diagnostics_for(name, fixture!("invalid-removed-killed-fields.md"));
    let expected_fields = [
        "object_id",
        "object_key",
        "topic",
        "workset",
        "schema_version",
        "updated_at",
        "created_at",
        "authored_from",
        "retrieval_hint",
    ];

    assert_eq!(diagnostics.len(), expected_fields.len());
    for (diagnostic, field) in diagnostics.iter().zip(expected_fields) {
        assert_diagnostic(
            diagnostic,
            path.as_path(),
            "KBV2-NOTE-REMOVED-FIELD",
            Some(field),
            MESSAGE_REMOVED_FIELD,
        );
    }
}

#[test]
fn every_source_derived_field_is_classified_as_removed() {
    let name = "invalid-removed-derived-fields.md";
    let path = fixture_path(name);
    let diagnostics = diagnostics_for(name, fixture!("invalid-removed-derived-fields.md"));
    let expected_fields = ["domain", "title", "identity", "verified"];

    assert_eq!(diagnostics.len(), expected_fields.len());
    for (diagnostic, field) in diagnostics.iter().zip(expected_fields) {
        assert_diagnostic(
            diagnostic,
            path.as_path(),
            "KBV2-NOTE-REMOVED-FIELD",
            Some(field),
            MESSAGE_REMOVED_FIELD,
        );
    }
}

#[test]
fn hash_and_projection_hash_fields_are_classified_as_removed() {
    let name = "invalid-removed-hash-fields.md";
    let path = fixture_path(name);
    let diagnostics = diagnostics_for(name, fixture!("invalid-removed-hash-fields.md"));
    let expected_fields = [
        "hash",
        "source_hash",
        "compiled_hash",
        "text_hash",
        "content_hash",
    ];

    assert_eq!(diagnostics.len(), expected_fields.len());
    for (diagnostic, field) in diagnostics.iter().zip(expected_fields) {
        assert_diagnostic(
            diagnostic,
            path.as_path(),
            "KBV2-NOTE-REMOVED-FIELD",
            Some(field),
            MESSAGE_REMOVED_FIELD,
        );
    }
}

#[test]
fn every_representative_removed_status_is_invalid() {
    let cases = [
        ("invalid-status-raw.md", fixture!("invalid-status-raw.md")),
        (
            "invalid-status-derived.md",
            fixture!("invalid-status-derived.md"),
        ),
        (
            "invalid-status-canonical.md",
            fixture!("invalid-status-canonical.md"),
        ),
        (
            "invalid-status-draft.md",
            fixture!("invalid-status-draft.md"),
        ),
        (
            "invalid-status-living.md",
            fixture!("invalid-status-living.md"),
        ),
    ];

    for (name, bytes) in cases {
        assert_one_diagnostic(
            name,
            bytes,
            "KBV2-NOTE-INVALID-STATUS",
            Some("status"),
            MESSAGE_INVALID_STATUS,
        );
    }
}

#[test]
fn renderer_omits_empty_optionals_and_emits_default_kind_explicitly() {
    let frontmatter = frontmatter("Canonical minimal", &["alpha", "beta"], NoteKind::Note);
    let rendered = render_note(&frontmatter, b"Body without heading.");

    assert_eq!(rendered, fixture!("rendered-canonical-minimal.md"));
}

#[test]
fn renderer_uses_the_complete_canonical_field_order() {
    let mut frontmatter = frontmatter(
        "Canonical decision",
        &["alpha", "two words"],
        NoteKind::Decision,
    );
    frontmatter.links = vec!["related-note".to_owned()];
    frontmatter.code = vec!["repo/src/lib.rs".to_owned()];
    frontmatter.assets = vec!["repo:service@main".to_owned()];
    frontmatter.supersedes = Some("adr-001-old-choice".to_owned());
    frontmatter.status = Some(NoteStatus::Frozen);

    let rendered = render_note(&frontmatter, b"\nDecision body.\r\n");
    assert_eq!(rendered, fixture!("rendered-canonical-full.md"));
}

#[test]
fn renderer_quotes_every_ambiguous_string_and_escapes_quote_and_backslash() {
    let frontmatter = NoteFrontmatter {
        description: "quote \" and slash \\".to_owned(),
        keywords: [
            "",
            "true",
            "False",
            "2026",
            "2026-09-10",
            "-leading",
            "?question",
            "two words",
            " padded ",
            "a,b",
            "[bracket]",
            "a:b",
            "召回",
            "path\\with\"quote",
            "alpha",
            "repo/src/lib.rs",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        kind: NoteKind::Reference,
        links: Vec::new(),
        code: Vec::new(),
        assets: Vec::new(),
        supersedes: None,
        status: None,
    };

    let rendered = render_note(&frontmatter, b"");
    assert!(std::str::from_utf8(&rendered).is_ok());
    assert_eq!(rendered, fixture!("rendered-canonical-quoting.md"));
}

#[test]
fn renderer_normalizes_body_endings_and_boundary_blank_lines() {
    let frontmatter = frontmatter("Body normalization", &["body"], NoteKind::Note);
    let rendered = render_note(&frontmatter, b"\r\n\r\nFirst\r\nSecond\rThird\r\n\r\n");

    assert_eq!(rendered, fixture!("rendered-canonical-body.md"));
    assert!(!rendered.contains(&b'\r'));
    assert!(rendered.ends_with(b"Third\n"));
}

#[test]
fn renderer_never_adds_forbidden_fields_or_filters_body_text() {
    let frontmatter = frontmatter("No derived metadata", &["safe"], NoteKind::Note);
    let rendered = render_note(
        &frontmatter,
        b"domain: body-owned\r\nsource_hash: body-owned\r\n",
    );
    let text = std::str::from_utf8(&rendered).expect("renderer output must be UTF-8");
    let without_opening = text
        .strip_prefix("---\n")
        .expect("renderer writes an exact opening fence");
    let (rendered_frontmatter, body) = without_opening
        .split_once("\n---\n\n")
        .expect("renderer writes an exact closing fence and one blank line");

    let forbidden = [
        "object_id",
        "object_key",
        "topic",
        "workset",
        "schema_version",
        "updated_at",
        "created_at",
        "authored_from",
        "retrieval_hint",
        "domain",
        "title",
        "identity",
        "verified",
        "hash",
        "source_hash",
        "compiled_hash",
        "text_hash",
        "content_hash",
    ];
    for field in forbidden {
        assert!(
            !rendered_frontmatter
                .lines()
                .any(|line| line.starts_with(field) && line[field.len()..].starts_with(':')),
            "renderer leaked {field} into frontmatter"
        );
    }
    assert_eq!(body, "domain: body-owned\nsource_hash: body-owned\n");
    assert!(!body.starts_with('#'), "renderer must not inject an H1");
}
