use crate::Diagnostic;
use std::collections::HashSet;
use std::path::Path;

const UTF8: &str = "KBV2-NOTE-UTF8";
const FRONTMATTER_MISSING: &str = "KBV2-NOTE-FRONTMATTER-MISSING";
const FRONTMATTER_UNTERMINATED: &str = "KBV2-NOTE-FRONTMATTER-UNTERMINATED";
const FRONTMATTER_SYNTAX: &str = "KBV2-NOTE-FRONTMATTER-SYNTAX";
const DUPLICATE_FIELD: &str = "KBV2-NOTE-DUPLICATE-FIELD";
const MISSING_FIELD: &str = "KBV2-NOTE-MISSING-FIELD";
const EMPTY_FIELD: &str = "KBV2-NOTE-EMPTY-FIELD";
const WRONG_TYPE: &str = "KBV2-NOTE-WRONG-TYPE";
const UNKNOWN_FIELD: &str = "KBV2-NOTE-UNKNOWN-FIELD";
const REMOVED_FIELD: &str = "KBV2-NOTE-REMOVED-FIELD";
const INVALID_KIND: &str = "KBV2-NOTE-INVALID-KIND";
const INVALID_STATUS: &str = "KBV2-NOTE-INVALID-STATUS";
const FIELD_NOT_ALLOWED: &str = "KBV2-NOTE-FIELD-NOT-ALLOWED";
const INVALID_VALUE: &str = "KBV2-NOTE-INVALID-VALUE";

const UTF8_MESSAGE: &str = "note is not valid UTF-8";
const FRONTMATTER_MISSING_MESSAGE: &str = "note must begin with a frontmatter fence";
const FRONTMATTER_UNTERMINATED_MESSAGE: &str = "frontmatter opening fence has no closing fence";
const FRONTMATTER_SYNTAX_MESSAGE: &str = "frontmatter does not match the controlled flat grammar";
const DUPLICATE_FIELD_MESSAGE: &str = "frontmatter field is duplicated";
const MISSING_FIELD_MESSAGE: &str = "required note field is missing";
const EMPTY_FIELD_MESSAGE: &str = "note field must not be empty";
const WRONG_TYPE_MESSAGE: &str = "note field has the wrong type";
const UNKNOWN_FIELD_MESSAGE: &str = "note field is not allowed by source-contract-v2";
const REMOVED_FIELD_MESSAGE: &str = "note field was removed in source-contract-v2";
const INVALID_KIND_MESSAGE: &str =
    "note kind must be spec, reference, runbook, decision, research, note, or index";
const INVALID_STATUS_MESSAGE: &str = "note status must be `frozen`";
const FIELD_NOT_ALLOWED_MESSAGE: &str = "note field is allowed only for kind `decision`";
const INVALID_VALUE_MESSAGE: &str = "note field has a value not allowed by source-contract-v2";

const MAX_FRONTMATTER_BYTES: usize = 1024 * 1024;

/// The source-owned semantic class of a note.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoteKind {
    Spec,
    Reference,
    Runbook,
    Decision,
    Research,
    Note,
    Index,
}

impl NoteKind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "spec" => Some(Self::Spec),
            "reference" => Some(Self::Reference),
            "runbook" => Some(Self::Runbook),
            "decision" => Some(Self::Decision),
            "research" => Some(Self::Research),
            "note" => Some(Self::Note),
            "index" => Some(Self::Index),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Spec => "spec",
            Self::Reference => "reference",
            Self::Runbook => "runbook",
            Self::Decision => "decision",
            Self::Research => "research",
            Self::Note => "note",
            Self::Index => "index",
        }
    }
}

/// The only source-owned lifecycle marker retained by source-contract-v2.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoteStatus {
    Frozen,
}

/// Validated source-owned note metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteFrontmatter {
    pub description: String,
    pub keywords: Vec<String>,
    pub kind: NoteKind,
    pub links: Vec<String>,
    pub code: Vec<String>,
    pub assets: Vec<String>,
    pub supersedes: Option<String>,
    pub status: Option<NoteStatus>,
}

/// A note whose controlled frontmatter has been parsed and validated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedNote {
    pub frontmatter: NoteFrontmatter,
    pub kind_explicit: bool,
    pub body: Vec<u8>,
    pub original: Vec<u8>,
}

#[derive(Clone, Copy)]
enum EnvelopeFailure {
    Missing,
    Unterminated,
    TooLarge,
}

#[derive(Clone, Copy)]
enum ParseFailure<'a> {
    Syntax,
    Duplicate(&'a str),
}

enum ParsedValue {
    Empty,
    Null,
    Scalar(String),
    List(Vec<ParsedItem>),
}

enum ParsedItem {
    Null,
    Scalar(String),
}

struct ParsedField<'a> {
    key: &'a str,
    value: ParsedValue,
}

#[derive(Clone, Copy)]
enum ResolvedKind {
    Valid(NoteKind),
    Invalid,
}

struct LogicalLine<'a> {
    text: &'a str,
    next: usize,
}

/// Parse and validate one note without reading from the filesystem.
pub fn parse_and_validate_note(
    path: &Path,
    bytes: &[u8],
) -> Result<ValidatedNote, Vec<Diagnostic>> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return Err(vec![diagnostic(path, UTF8, UTF8_MESSAGE, None)]),
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (frontmatter_start, closing_start, body_start) = match locate_envelope(text) {
        Ok(envelope) => envelope,
        Err(EnvelopeFailure::Missing) => {
            return Err(vec![diagnostic(
                path,
                FRONTMATTER_MISSING,
                FRONTMATTER_MISSING_MESSAGE,
                None,
            )]);
        }
        Err(EnvelopeFailure::Unterminated) => {
            return Err(vec![diagnostic(
                path,
                FRONTMATTER_UNTERMINATED,
                FRONTMATTER_UNTERMINATED_MESSAGE,
                None,
            )]);
        }
        Err(EnvelopeFailure::TooLarge) => {
            return Err(vec![diagnostic(
                path,
                FRONTMATTER_SYNTAX,
                FRONTMATTER_SYNTAX_MESSAGE,
                None,
            )]);
        }
    };

    let lines = collect_frontmatter_lines(&text[frontmatter_start..closing_start]);

    let fields = match parse_fields(&lines) {
        Ok(fields) => fields,
        Err(ParseFailure::Syntax) => {
            return Err(vec![diagnostic(
                path,
                FRONTMATTER_SYNTAX,
                FRONTMATTER_SYNTAX_MESSAGE,
                None,
            )]);
        }
        Err(ParseFailure::Duplicate(field)) => {
            return Err(vec![diagnostic(
                path,
                DUPLICATE_FIELD,
                DUPLICATE_FIELD_MESSAGE,
                Some(field),
            )]);
        }
    };

    validate_fields(
        path,
        fields,
        text.as_bytes()[body_start..].to_vec(),
        bytes.to_vec(),
    )
}

/// Render source-owned metadata and a UTF-8 body in canonical source-contract-v2 form.
#[must_use]
pub fn render_note(frontmatter: &NoteFrontmatter, body: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(body.len().saturating_add(128));
    output.extend_from_slice(b"---\ndescription: ");
    push_quoted(&mut output, &frontmatter.description);
    output.extend_from_slice(b"\nkeywords: ");
    push_list(&mut output, &frontmatter.keywords);
    output.extend_from_slice(b"\nkind: ");
    output.extend_from_slice(frontmatter.kind.as_str().as_bytes());

    push_optional_list(&mut output, "links", &frontmatter.links);
    push_optional_list(&mut output, "code", &frontmatter.code);
    push_optional_list(&mut output, "assets", &frontmatter.assets);
    if let Some(supersedes) = &frontmatter.supersedes {
        output.extend_from_slice(b"\nsupersedes: ");
        push_scalar(&mut output, supersedes);
    }
    if let Some(NoteStatus::Frozen) = frontmatter.status {
        output.extend_from_slice(b"\nstatus: frozen");
    }
    output.extend_from_slice(b"\n---\n\n");
    push_normalized_body(&mut output, body);
    output
}

fn locate_envelope(text: &str) -> Result<(usize, usize, usize), EnvelopeFailure> {
    let opening = logical_line_at(text, 0);
    if opening.text != "---" {
        return Err(EnvelopeFailure::Missing);
    }

    let frontmatter_start = opening.next;
    let mut position = frontmatter_start;
    while position < text.len() {
        if position - frontmatter_start > MAX_FRONTMATTER_BYTES {
            return Err(EnvelopeFailure::TooLarge);
        }
        let line = logical_line_at(text, position);
        if line.text == "---" {
            return Ok((frontmatter_start, position, line.next));
        }
        position = line.next;
        if position - frontmatter_start > MAX_FRONTMATTER_BYTES {
            return Err(EnvelopeFailure::TooLarge);
        }
    }
    Err(EnvelopeFailure::Unterminated)
}

fn collect_frontmatter_lines(frontmatter: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut position = 0;
    while position < frontmatter.len() {
        let line = logical_line_at(frontmatter, position);
        lines.push(line.text);
        position = line.next;
    }
    lines
}

fn logical_line_at(text: &str, start: usize) -> LogicalLine<'_> {
    let remaining = &text[start..];
    if let Some(relative_end) = remaining.as_bytes().iter().position(|byte| *byte == b'\n') {
        let raw = &remaining[..relative_end];
        LogicalLine {
            text: raw.strip_suffix('\r').unwrap_or(raw),
            next: start + relative_end + 1,
        }
    } else {
        LogicalLine {
            text: remaining,
            next: text.len(),
        }
    }
}

fn parse_fields<'a>(lines: &[&'a str]) -> Result<Vec<ParsedField<'a>>, ParseFailure<'a>> {
    if lines
        .iter()
        .any(|line| line.contains('\r') || line.contains('\u{feff}'))
    {
        return Err(ParseFailure::Syntax);
    }

    let mut fields: Vec<ParsedField<'a>> = Vec::new();
    let mut seen = HashSet::new();
    let mut line_index = 0;
    while line_index < lines.len() {
        let line = lines[line_index];
        if is_ascii_blank(line) || is_comment_line(line) {
            line_index += 1;
            continue;
        }
        if starts_with_ascii_indent(line) {
            return Err(ParseFailure::Syntax);
        }

        let Some(colon) = line.as_bytes().iter().position(|byte| *byte == b':') else {
            return Err(ParseFailure::Syntax);
        };
        let key = &line[..colon];
        if !is_key(key) {
            return Err(ParseFailure::Syntax);
        }
        if !seen.insert(key) {
            return Err(ParseFailure::Duplicate(key));
        }

        let (value, next_line) =
            parse_field_value(lines, line_index, colon + 1).map_err(|()| ParseFailure::Syntax)?;
        fields.push(ParsedField { key, value });
        line_index = next_line;
    }
    Ok(fields)
}

fn parse_field_value(
    lines: &[&str],
    line_index: usize,
    value_column: usize,
) -> Result<(ParsedValue, usize), ()> {
    let line = lines[line_index];
    let raw = &line[value_column..];
    let start = ascii_trim_start_index(raw);
    if start == raw.len() || raw.as_bytes()[start] == b'#' {
        if lines
            .get(line_index + 1)
            .is_some_and(|next| next.starts_with("  - "))
        {
            return parse_block_list(lines, line_index + 1);
        }
        return Ok((ParsedValue::Empty, line_index + 1));
    }

    match raw.as_bytes()[start] {
        b'[' => parse_flow_list(lines, line_index, value_column + start),
        b'|' | b'>' => {
            if !trailing_is_whitespace_or_comment(raw, start + 1) {
                return Err(());
            }
            parse_block_scalar(lines, line_index + 1, raw.as_bytes()[start])
        }
        _ => {
            let item = parse_scalar_item(raw)?;
            let value = match item {
                ParsedItem::Null => ParsedValue::Null,
                ParsedItem::Scalar(value) => ParsedValue::Scalar(value),
            };
            Ok((value, line_index + 1))
        }
    }
}

fn parse_block_list(lines: &[&str], mut line_index: usize) -> Result<(ParsedValue, usize), ()> {
    let mut items = Vec::new();
    while let Some(line) = lines.get(line_index) {
        let Some(raw) = line.strip_prefix("  - ") else {
            break;
        };
        items.push(parse_scalar_item(raw)?);
        line_index += 1;
    }
    Ok((ParsedValue::List(items), line_index))
}

fn parse_block_scalar(
    lines: &[&str],
    mut line_index: usize,
    indicator: u8,
) -> Result<(ParsedValue, usize), ()> {
    let mut content = Vec::new();
    while let Some(line) = lines.get(line_index) {
        if is_ascii_blank(line) {
            content.push("");
            line_index += 1;
            continue;
        }
        if let Some(value) = line.strip_prefix("  ") {
            content.push(value);
            line_index += 1;
            continue;
        }
        if starts_with_ascii_indent(line) {
            return Err(());
        }
        break;
    }
    while content.last().is_some_and(|line| line.is_empty()) {
        content.pop();
    }

    let value = if indicator == b'|' {
        join_literal_lines(&content)
    } else {
        join_folded_lines(&content)
    };
    Ok((ParsedValue::Scalar(value), line_index))
}

fn join_literal_lines(lines: &[&str]) -> String {
    let capacity =
        lines.iter().map(|line| line.len()).sum::<usize>() + lines.len().saturating_sub(1);
    let mut output = String::with_capacity(capacity);
    for (index, line) in lines.iter().enumerate() {
        if index != 0 {
            output.push('\n');
        }
        output.push_str(line);
    }
    output
}

fn join_folded_lines(lines: &[&str]) -> String {
    let capacity =
        lines.iter().map(|line| line.len()).sum::<usize>() + lines.len().saturating_sub(1);
    let mut output = String::with_capacity(capacity);
    let mut index = 0;
    while index < lines.len() {
        if lines[index].is_empty() {
            output.push('\n');
            while index < lines.len() && lines[index].is_empty() {
                index += 1;
            }
            continue;
        }

        output.push_str(lines[index]);
        if lines.get(index + 1).is_some_and(|next| !next.is_empty()) {
            output.push(' ');
        }
        index += 1;
    }
    output
}

fn parse_scalar_item(raw: &str) -> Result<ParsedItem, ()> {
    let start = ascii_trim_start_index(raw);
    if start == raw.len() || raw.as_bytes()[start] == b'#' {
        return Err(());
    }

    match raw.as_bytes()[start] {
        b'\'' => {
            let (value, end) = parse_single_quoted(raw, start)?;
            if trailing_is_whitespace_or_comment(raw, end) {
                Ok(ParsedItem::Scalar(value))
            } else {
                Err(())
            }
        }
        b'"' => {
            let (value, end) = parse_double_quoted(raw, start)?;
            if trailing_is_whitespace_or_comment(raw, end) {
                Ok(ParsedItem::Scalar(value))
            } else {
                Err(())
            }
        }
        _ => parse_bare_item(raw, start),
    }
}

fn parse_bare_item(raw: &str, start: usize) -> Result<ParsedItem, ()> {
    let mut end = raw.len();
    for (offset, byte) in raw.as_bytes()[start..].iter().enumerate() {
        let index = start + offset;
        if *byte == b'#' && (index == start || matches!(raw.as_bytes()[index - 1], b' ' | b'\t')) {
            end = index;
            break;
        }
    }
    end = ascii_trim_end_index(raw, end);
    if end == start {
        return Err(());
    }
    parsed_bare_token(&raw[start..end])
}

fn parsed_bare_token(value: &str) -> Result<ParsedItem, ()> {
    if !is_legal_bare(value) {
        return Err(());
    }
    if value == "~" || value.eq_ignore_ascii_case("null") {
        Ok(ParsedItem::Null)
    } else {
        Ok(ParsedItem::Scalar(value.to_owned()))
    }
}

fn parse_single_quoted(raw: &str, start: usize) -> Result<(String, usize), ()> {
    let mut output = String::new();
    let mut index = start + 1;
    while index < raw.len() {
        if raw.as_bytes()[index] == b'\'' {
            if raw.as_bytes().get(index + 1) == Some(&b'\'') {
                output.push('\'');
                index += 2;
            } else {
                return Ok((output, index + 1));
            }
        } else {
            let character = raw[index..].chars().next().ok_or(())?;
            output.push(character);
            index += character.len_utf8();
        }
    }
    Err(())
}

fn parse_double_quoted(raw: &str, start: usize) -> Result<(String, usize), ()> {
    let mut output = String::new();
    let mut index = start + 1;
    while index < raw.len() {
        match raw.as_bytes()[index] {
            b'"' => return Ok((output, index + 1)),
            b'\\' => {
                let Some(escape) = raw.as_bytes().get(index + 1).copied() else {
                    return Err(());
                };
                match escape {
                    b'\\' => output.push('\\'),
                    b'"' => output.push('"'),
                    b'n' => output.push('\n'),
                    b'r' => output.push('\r'),
                    b't' => output.push('\t'),
                    b'u' => {
                        let first = parse_hex_quad(raw, index + 2)?;
                        let scalar = match first {
                            0xd800..=0xdbff => {
                                if raw.as_bytes().get(index + 6) != Some(&b'\\')
                                    || raw.as_bytes().get(index + 7) != Some(&b'u')
                                {
                                    return Err(());
                                }
                                let second = parse_hex_quad(raw, index + 8)?;
                                if !(0xdc00..=0xdfff).contains(&second) {
                                    return Err(());
                                }
                                index += 12;
                                0x1_0000 + ((first - 0xd800) << 10) + (second - 0xdc00)
                            }
                            0xdc00..=0xdfff => return Err(()),
                            scalar => {
                                index += 6;
                                scalar
                            }
                        };
                        output.push(char::from_u32(scalar).ok_or(())?);
                        continue;
                    }
                    _ => return Err(()),
                }
                index += 2;
            }
            _ => {
                let character = raw[index..].chars().next().ok_or(())?;
                output.push(character);
                index += character.len_utf8();
            }
        }
    }
    Err(())
}

fn parse_hex_quad(raw: &str, start: usize) -> Result<u32, ()> {
    let digits = raw.as_bytes().get(start..start + 4).ok_or(())?;
    let mut value = 0_u32;
    for digit in digits {
        value = (value << 4) | u32::from(hex_value(*digit).ok_or(())?);
    }
    Ok(value)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

struct FlowCursor<'lines, 'text> {
    lines: &'lines [&'text str],
    line: usize,
    column: usize,
}

impl FlowCursor<'_, '_> {
    fn peek(&self) -> Option<u8> {
        self.lines
            .get(self.line)
            .and_then(|line| line.as_bytes().get(self.column))
            .copied()
    }

    fn advance(&mut self) {
        self.column += 1;
    }

    fn skip_gap(&mut self, initial_comment_allowed: bool) {
        let mut comment_allowed = initial_comment_allowed
            || self.column == 0
            || self
                .lines
                .get(self.line)
                .and_then(|line| self.column.checked_sub(1).map(|index| (line, index)))
                .and_then(|(line, index)| line.as_bytes().get(index))
                .is_some_and(|byte| matches!(byte, b' ' | b'\t'));

        loop {
            let Some(&line) = self.lines.get(self.line) else {
                return;
            };
            while self.peek().is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
                comment_allowed = true;
                self.advance();
            }
            if self.column == line.len() {
                self.line += 1;
                self.column = 0;
                comment_allowed = true;
                continue;
            }
            if self.peek() == Some(b'#') && comment_allowed {
                self.line += 1;
                self.column = 0;
                comment_allowed = true;
                continue;
            }
            return;
        }
    }

    fn parse_item(&mut self) -> Result<ParsedItem, ()> {
        let line = *self.lines.get(self.line).ok_or(())?;
        let start = self.column;
        match self.peek().ok_or(())? {
            b'\'' => {
                let (value, end) = parse_single_quoted(line, start)?;
                self.column = end;
                Ok(ParsedItem::Scalar(value))
            }
            b'"' => {
                let (value, end) = parse_double_quoted(line, start)?;
                self.column = end;
                Ok(ParsedItem::Scalar(value))
            }
            _ => {
                while let Some(byte) = self.peek() {
                    if matches!(byte, b',' | b']')
                        || (byte == b'#'
                            && (self.column == start
                                || matches!(line.as_bytes()[self.column - 1], b' ' | b'\t')))
                    {
                        break;
                    }
                    self.advance();
                }
                let end = ascii_trim_end_index(line, self.column);
                if end == start {
                    return Err(());
                }
                parsed_bare_token(&line[start..end])
            }
        }
    }
}

fn parse_flow_list(
    lines: &[&str],
    line_index: usize,
    open_column: usize,
) -> Result<(ParsedValue, usize), ()> {
    let mut cursor = FlowCursor {
        lines,
        line: line_index,
        column: open_column + 1,
    };
    let mut items = Vec::new();
    cursor.skip_gap(true);
    if cursor.peek() == Some(b']') {
        return finish_flow_list(cursor, items);
    }

    loop {
        if matches!(cursor.peek(), None | Some(b',') | Some(b']')) {
            return Err(());
        }
        items.push(cursor.parse_item()?);
        cursor.skip_gap(false);
        match cursor.peek() {
            Some(b']') => return finish_flow_list(cursor, items),
            Some(b',') => {
                cursor.advance();
                cursor.skip_gap(true);
                if matches!(cursor.peek(), None | Some(b',') | Some(b']')) {
                    return Err(());
                }
            }
            _ => return Err(()),
        }
    }
}

fn finish_flow_list(
    mut cursor: FlowCursor<'_, '_>,
    items: Vec<ParsedItem>,
) -> Result<(ParsedValue, usize), ()> {
    let line_index = cursor.line;
    cursor.advance();
    let line = cursor.lines[line_index];
    if !trailing_is_whitespace_or_comment(line, cursor.column) {
        return Err(());
    }
    Ok((ParsedValue::List(items), line_index + 1))
}

fn trailing_is_whitespace_or_comment(value: &str, start: usize) -> bool {
    if start == value.len() {
        return true;
    }
    let suffix = &value[start..];
    let whitespace = ascii_trim_start_index(suffix);
    if whitespace == suffix.len() {
        return true;
    }
    whitespace != 0 && suffix.as_bytes()[whitespace] == b'#'
}

fn is_legal_bare(value: &str) -> bool {
    if value == "..." || value.is_empty() {
        return false;
    }
    let first = value.as_bytes()[0];
    if matches!(
        first,
        b'-' | b'?'
            | b':'
            | b','
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'#'
            | b'&'
            | b'*'
            | b'!'
            | b'|'
            | b'>'
            | b'\''
            | b'"'
            | b'%'
            | b'@'
            | b'`'
    ) {
        return false;
    }

    let bytes = value.as_bytes();
    if bytes.last() == Some(&b':')
        || bytes
            .windows(2)
            .any(|pair| pair[0] == b':' && matches!(pair[1], b' ' | b'\t'))
    {
        return false;
    }
    !bytes.iter().enumerate().any(|(index, byte)| {
        matches!(byte, b'&' | b'*' | b'!')
            && (index == 0 || matches!(bytes[index - 1], b' ' | b'\t'))
    })
}

fn validate_fields(
    path: &Path,
    fields: Vec<ParsedField<'_>>,
    body: Vec<u8>,
    original: Vec<u8>,
) -> Result<ValidatedNote, Vec<Diagnostic>> {
    let kind_explicit = fields.iter().any(|field| field.key == "kind");
    let resolved_kind = resolve_kind(&fields);
    let mut diagnostics = Vec::new();
    let mut description = None;
    let mut keywords = None;
    let mut links = Vec::new();
    let mut code = Vec::new();
    let mut assets = Vec::new();
    let mut supersedes = None;
    let mut status = None;
    let mut description_present = false;
    let mut keywords_present = false;

    for field in fields {
        match field.key {
            "description" => {
                description_present = true;
                if let Some(value) =
                    take_nonempty_scalar(path, "description", field.value, &mut diagnostics)
                {
                    if value.contains('\r') || value.contains('\n') {
                        diagnostics.push(diagnostic(
                            path,
                            INVALID_VALUE,
                            INVALID_VALUE_MESSAGE,
                            Some("description"),
                        ));
                    } else {
                        description = Some(value);
                    }
                }
            }
            "keywords" => {
                keywords_present = true;
                keywords = take_string_list(path, "keywords", field.value, true, &mut diagnostics);
            }
            "kind" => validate_kind(path, field.value, &mut diagnostics),
            "links" => {
                if let Some(value) =
                    take_string_list(path, "links", field.value, false, &mut diagnostics)
                {
                    links = value;
                }
            }
            "code" => {
                if let Some(value) =
                    take_string_list(path, "code", field.value, false, &mut diagnostics)
                {
                    code = value;
                }
            }
            "assets" => {
                if let Some(value) =
                    take_string_list(path, "assets", field.value, false, &mut diagnostics)
                {
                    if !matches!(resolved_kind, ResolvedKind::Invalid)
                        && !matches!(resolved_kind, ResolvedKind::Valid(NoteKind::Decision))
                    {
                        diagnostics.push(diagnostic(
                            path,
                            FIELD_NOT_ALLOWED,
                            FIELD_NOT_ALLOWED_MESSAGE,
                            Some("assets"),
                        ));
                    }
                    assets = value;
                }
            }
            "supersedes" => {
                if let Some(value) =
                    take_nonempty_scalar(path, "supersedes", field.value, &mut diagnostics)
                {
                    if !matches!(resolved_kind, ResolvedKind::Invalid)
                        && !matches!(resolved_kind, ResolvedKind::Valid(NoteKind::Decision))
                    {
                        diagnostics.push(diagnostic(
                            path,
                            FIELD_NOT_ALLOWED,
                            FIELD_NOT_ALLOWED_MESSAGE,
                            Some("supersedes"),
                        ));
                    }
                    supersedes = Some(value);
                }
            }
            "status" => {
                if let Some(value) =
                    take_nonempty_scalar(path, "status", field.value, &mut diagnostics)
                {
                    if value == "frozen" {
                        status = Some(NoteStatus::Frozen);
                    } else {
                        diagnostics.push(diagnostic(
                            path,
                            INVALID_STATUS,
                            INVALID_STATUS_MESSAGE,
                            Some("status"),
                        ));
                    }
                }
            }
            key if is_removed_field(key) => diagnostics.push(diagnostic(
                path,
                REMOVED_FIELD,
                REMOVED_FIELD_MESSAGE,
                Some(key),
            )),
            key => diagnostics.push(diagnostic(
                path,
                UNKNOWN_FIELD,
                UNKNOWN_FIELD_MESSAGE,
                Some(key),
            )),
        }
    }

    if !description_present {
        diagnostics.push(diagnostic(
            path,
            MISSING_FIELD,
            MISSING_FIELD_MESSAGE,
            Some("description"),
        ));
    }
    if !keywords_present {
        diagnostics.push(diagnostic(
            path,
            MISSING_FIELD,
            MISSING_FIELD_MESSAGE,
            Some("keywords"),
        ));
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let kind = match resolved_kind {
        ResolvedKind::Valid(kind) => kind,
        ResolvedKind::Invalid => NoteKind::Note,
    };
    Ok(ValidatedNote {
        frontmatter: NoteFrontmatter {
            description: description.unwrap_or_default(),
            keywords: keywords.unwrap_or_default(),
            kind,
            links,
            code,
            assets,
            supersedes,
            status,
        },
        kind_explicit,
        body,
        original,
    })
}

fn resolve_kind(fields: &[ParsedField<'_>]) -> ResolvedKind {
    let Some(field) = fields.iter().find(|field| field.key == "kind") else {
        return ResolvedKind::Valid(NoteKind::Note);
    };
    match &field.value {
        ParsedValue::Scalar(value) if !is_empty_string(value) => NoteKind::parse(value)
            .map(ResolvedKind::Valid)
            .unwrap_or(ResolvedKind::Invalid),
        _ => ResolvedKind::Invalid,
    }
}

fn validate_kind(path: &Path, value: ParsedValue, diagnostics: &mut Vec<Diagnostic>) {
    match value {
        ParsedValue::Empty => diagnostics.push(diagnostic(
            path,
            EMPTY_FIELD,
            EMPTY_FIELD_MESSAGE,
            Some("kind"),
        )),
        ParsedValue::Null | ParsedValue::List(_) => diagnostics.push(diagnostic(
            path,
            WRONG_TYPE,
            WRONG_TYPE_MESSAGE,
            Some("kind"),
        )),
        ParsedValue::Scalar(value) if is_empty_string(&value) => diagnostics.push(diagnostic(
            path,
            EMPTY_FIELD,
            EMPTY_FIELD_MESSAGE,
            Some("kind"),
        )),
        ParsedValue::Scalar(value) if NoteKind::parse(&value).is_none() => diagnostics.push(
            diagnostic(path, INVALID_KIND, INVALID_KIND_MESSAGE, Some("kind")),
        ),
        ParsedValue::Scalar(_) => {}
    }
}

fn take_nonempty_scalar(
    path: &Path,
    field: &str,
    value: ParsedValue,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<String> {
    match value {
        ParsedValue::Empty => {
            diagnostics.push(diagnostic(
                path,
                EMPTY_FIELD,
                EMPTY_FIELD_MESSAGE,
                Some(field),
            ));
            None
        }
        ParsedValue::Null | ParsedValue::List(_) => {
            diagnostics.push(diagnostic(
                path,
                WRONG_TYPE,
                WRONG_TYPE_MESSAGE,
                Some(field),
            ));
            None
        }
        ParsedValue::Scalar(value) if is_empty_string(&value) => {
            diagnostics.push(diagnostic(
                path,
                EMPTY_FIELD,
                EMPTY_FIELD_MESSAGE,
                Some(field),
            ));
            None
        }
        ParsedValue::Scalar(value) => Some(value),
    }
}

fn take_string_list(
    path: &Path,
    field: &str,
    value: ParsedValue,
    require_item: bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<String>> {
    let items = match value {
        ParsedValue::Empty => {
            diagnostics.push(diagnostic(
                path,
                EMPTY_FIELD,
                EMPTY_FIELD_MESSAGE,
                Some(field),
            ));
            return None;
        }
        ParsedValue::Null | ParsedValue::Scalar(_) => {
            diagnostics.push(diagnostic(
                path,
                WRONG_TYPE,
                WRONG_TYPE_MESSAGE,
                Some(field),
            ));
            return None;
        }
        ParsedValue::List(items) => items,
    };
    if require_item && items.is_empty() {
        diagnostics.push(diagnostic(
            path,
            EMPTY_FIELD,
            EMPTY_FIELD_MESSAGE,
            Some(field),
        ));
        return None;
    }

    let diagnostic_count = diagnostics.len();
    let mut values = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate() {
        let item_field = format!("{field}[{index}]");
        match item {
            ParsedItem::Null => diagnostics.push(diagnostic(
                path,
                WRONG_TYPE,
                WRONG_TYPE_MESSAGE,
                Some(&item_field),
            )),
            ParsedItem::Scalar(value) if is_empty_string(&value) => {
                diagnostics.push(diagnostic(
                    path,
                    EMPTY_FIELD,
                    EMPTY_FIELD_MESSAGE,
                    Some(&item_field),
                ));
            }
            ParsedItem::Scalar(value) => values.push(value),
        }
    }
    (diagnostics.len() == diagnostic_count).then_some(values)
}

fn is_empty_string(value: &str) -> bool {
    !value.chars().any(|character| !character.is_whitespace())
}

fn is_removed_field(key: &str) -> bool {
    matches!(
        key,
        "object_id"
            | "object_key"
            | "topic"
            | "workset"
            | "schema_version"
            | "updated_at"
            | "created_at"
            | "authored_from"
            | "retrieval_hint"
            | "domain"
            | "title"
            | "identity"
            | "verified"
            | "hash"
    ) || key.ends_with("_hash")
}

fn diagnostic(path: &Path, code: &'static str, message: &str, field: Option<&str>) -> Diagnostic {
    let diagnostic = Diagnostic::error(code, message).at_path(path);
    match field {
        Some(field) => diagnostic.for_field(field),
        None => diagnostic,
    }
}

fn is_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn is_ascii_blank(value: &str) -> bool {
    value.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
}
fn starts_with_ascii_indent(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
}

fn is_comment_line(value: &str) -> bool {
    value.as_bytes().get(ascii_trim_start_index(value)) == Some(&b'#')
}

fn ascii_trim_start_index(value: &str) -> usize {
    value
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

fn ascii_trim_end_index(value: &str, mut end: usize) -> usize {
    while end != 0 && matches!(value.as_bytes()[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    end
}

fn push_optional_list(output: &mut Vec<u8>, name: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    output.push(b'\n');
    output.extend_from_slice(name.as_bytes());
    output.extend_from_slice(b": ");
    push_list(output, values);
}

fn push_list(output: &mut Vec<u8>, values: &[String]) {
    output.push(b'[');
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            output.extend_from_slice(b", ");
        }
        push_scalar(output, value);
    }
    output.push(b']');
}

fn push_scalar(output: &mut Vec<u8>, value: &str) {
    if is_safe_bare(value) {
        output.extend_from_slice(value.as_bytes());
    } else {
        push_quoted(output, value);
    }
}

fn is_safe_bare(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic()
        || !bytes
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'-'))
    {
        return false;
    }
    ![
        "true", "false", "yes", "no", "on", "off", "null", "none", "nan", "inf",
    ]
    .iter()
    .any(|reserved| value.eq_ignore_ascii_case(reserved))
}

fn push_quoted(output: &mut Vec<u8>, value: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    output.push(b'"');
    for character in value.chars() {
        match character {
            '\\' => output.extend_from_slice(b"\\\\"),
            '"' => output.extend_from_slice(b"\\\""),
            '\n' => output.extend_from_slice(b"\\n"),
            '\r' => output.extend_from_slice(b"\\r"),
            '\t' => output.extend_from_slice(b"\\t"),
            '\u{feff}' => output.extend_from_slice(b"\\ufeff"),
            '\0'..='\u{001f}' | '\u{007f}' => {
                let value = character as u32;
                output.extend_from_slice(&[
                    b'\\',
                    b'u',
                    HEX[((value >> 12) & 0x0f) as usize],
                    HEX[((value >> 8) & 0x0f) as usize],
                    HEX[((value >> 4) & 0x0f) as usize],
                    HEX[(value & 0x0f) as usize],
                ]);
            }
            character => {
                let mut encoded = [0_u8; 4];
                output.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            }
        }
    }
    output.push(b'"');
}

fn push_normalized_body(output: &mut Vec<u8>, body: &[u8]) {
    let body_start = output.len();
    let mut input = 0;
    let mut content_started = false;
    while input < body.len() {
        match body[input] {
            b'\r' => {
                if body.get(input + 1) == Some(&b'\n') {
                    input += 1;
                }
                if content_started {
                    output.push(b'\n');
                }
            }
            b'\n' => {
                if content_started {
                    output.push(b'\n');
                }
            }
            byte => {
                output.push(byte);
                content_started = true;
            }
        }
        input += 1;
    }

    while output.len() > body_start && output.last() == Some(&b'\n') {
        output.pop();
    }
    if output.len() > body_start {
        output.push(b'\n');
    }
}
