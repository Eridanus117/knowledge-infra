use jieba_rs::Jieba;
use std::sync::{Arc, LazyLock};
use tantivy::tokenizer::{
    LowerCaser, SimpleTokenizer, TextAnalyzer, Token, TokenStream, Tokenizer,
};

/// Stable registration name for the natural-language analyzer.
pub const NATURAL_V2: &str = "natural_v2";
/// Stable registration name for the identifier/path analyzer.
pub const SLUG_V2: &str = "slug_v2";

const MAX_NATURAL_TOKEN_SCALARS: usize = 40;

/// Construct the pinned Chinese search analyzer used for natural-language fields.
///
/// The embedded dictionary comes from the exact `jieba-rs` dependency selected by
/// this workspace. Search mode emits the overlapping two- and three-character
/// dictionary terms used by lexical recall, and the final filter applies the
/// contract's Unicode-scalar length limit.
#[must_use]
pub fn natural_v2() -> TextAnalyzer {
    TextAnalyzer::builder(JiebaSearchTokenizer::default()).build()
}

/// Construct the slug analyzer used for identities and source-relative paths.
///
/// Tantivy's `SimpleTokenizer` treats the configured path/identity separators as
/// punctuation, preserving contiguous ASCII letters and digits as one run. The
/// lower-casing filter keeps matching independent of source capitalization.
#[must_use]
pub fn slug_v2() -> TextAnalyzer {
    TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(LowerCaser)
        .build()
}

/// The tokenizer implementation is intentionally private: callers select the
/// versioned analyzer through [`natural_v2`] and its registered name.
#[derive(Clone)]
struct JiebaSearchTokenizer {
    worker: Arc<Jieba>,
}

impl Default for JiebaSearchTokenizer {
    fn default() -> Self {
        Self {
            worker: pinned_jieba(),
        }
    }
}

fn pinned_jieba() -> Arc<Jieba> {
    static JIEBA: LazyLock<Arc<Jieba>> = LazyLock::new(|| Arc::new(Jieba::new()));
    JIEBA.clone()
}

impl Tokenizer for JiebaSearchTokenizer {
    type TokenStream<'a> = JiebaSearchTokenStream;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        let mut position = 0usize;
        let tokens = self
            .worker
            .cut_for_search(text, false)
            .into_iter()
            .filter_map(|token| {
                let normalized = token.word.to_lowercase();
                if normalized.trim().is_empty() {
                    return None;
                }
                let token_position = position;
                position += 1;
                if normalized.chars().count() > MAX_NATURAL_TOKEN_SCALARS {
                    return None;
                }
                Some(Token {
                    offset_from: token.byte_start,
                    offset_to: token.byte_end,
                    position: token_position,
                    text: normalized,
                    position_length: 1,
                })
            })
            .collect();
        JiebaSearchTokenStream { tokens, index: 0 }
    }
}

struct JiebaSearchTokenStream {
    tokens: Vec<Token>,
    index: usize,
}

impl TokenStream for JiebaSearchTokenStream {
    fn advance(&mut self) -> bool {
        if self.index < self.tokens.len() {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn token(&self) -> &Token {
        &self.tokens[self.index - 1]
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.tokens[self.index - 1]
    }
}
