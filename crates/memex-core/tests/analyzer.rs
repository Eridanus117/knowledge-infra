use memex_core::analyzer::{NATURAL_V2, SLUG_V2, natural_v2, slug_v2};
use tantivy::tokenizer::{TextAnalyzer, TokenStream};

fn collect_tokens(mut analyzer: TextAnalyzer, input: &str) -> Vec<String> {
    let mut stream = analyzer.token_stream(input);
    let mut tokens = Vec::new();
    while stream.advance() {
        tokens.push(stream.token().text.clone());
    }
    tokens
}

fn collect_token_details(
    mut analyzer: TextAnalyzer,
    input: &str,
) -> Vec<(String, usize, usize, usize)> {
    let mut stream = analyzer.token_stream(input);
    let mut tokens = Vec::new();
    while stream.advance() {
        let token = stream.token();
        tokens.push((
            token.text.clone(),
            token.position,
            token.offset_from,
            token.offset_to,
        ));
    }
    tokens
}

#[test]
fn natural_v2_preserves_multibyte_offsets_for_overlapping_tokens() {
    let input = include_str!("../../../fixtures/memex/analyzer/natural.input").trim();

    let details = collect_token_details(natural_v2(), input);

    assert_eq!(
        details,
        vec![
            ("南京".to_owned(), 0, 0, 6),
            ("京市".to_owned(), 1, 3, 9),
            ("南京市".to_owned(), 2, 0, 9),
            ("长江".to_owned(), 3, 9, 15),
            ("大桥".to_owned(), 4, 15, 21),
            ("长江大桥".to_owned(), 5, 9, 21),
            ("mixed".to_owned(), 6, 22, 27),
        ]
    );
}

#[test]
fn natural_v2_uses_pinned_search_mode_and_lowercases() {
    let input = include_str!("../../../fixtures/memex/analyzer/natural.input").trim();
    let expected = include_str!("../../../fixtures/memex/analyzer/natural.tokens")
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();

    let tokens = collect_tokens(natural_v2(), input);

    assert_eq!(tokens, expected);
    assert_eq!(NATURAL_V2, "natural_v2");
    assert!(tokens.windows(2).any(|pair| pair == ["南京", "京市"]));
    assert!(tokens.iter().all(|token| token == &token.to_lowercase()));
}

#[test]
fn natural_v2_drops_tokens_longer_than_40_unicode_scalars() {
    let forty_one = "A".repeat(41);
    let forty = "B".repeat(40);
    let input = format!("{forty_one} {forty} keep");

    let tokens = collect_tokens(natural_v2(), &input);

    assert!(!tokens.contains(&forty_one.to_lowercase()));
    assert!(tokens.contains(&forty.to_lowercase()));
    assert!(tokens.contains(&"keep".to_owned()));
}

#[test]
fn slug_v2_splits_path_identity_delimiters_and_preserves_ascii_runs() {
    let input = include_str!("../../../fixtures/memex/analyzer/slug.input").trim();
    let expected = include_str!("../../../fixtures/memex/analyzer/slug.tokens")
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();

    let tokens = collect_tokens(slug_v2(), input);

    assert_eq!(tokens, expected);
    assert_eq!(SLUG_V2, "slug_v2");
}
#[test]
fn natural_v2_preserves_a_position_gap_when_dropping_a_long_token() {
    let long = "A".repeat(41);
    let input = format!("keep {long} after");

    let details = collect_token_details(natural_v2(), &input);

    assert_eq!(
        details,
        vec![
            ("keep".to_owned(), 0, 0, 4),
            ("after".to_owned(), 2, 47, 52),
        ]
    );
}
