use kb_contract::{Diagnostic, ValidatedNote};
use serde_json::Value;
use std::env;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
pub const MERMAID_INVALID_CODE: &str = "KBV2-MERMAID-INVALID";
pub const MERMAID_INVALID_MESSAGE: &str = "Mermaid diagram could not be parsed";
pub const MERMAID_ADAPTER_CODE: &str = "KBV2-MERMAID-ADAPTER";
pub const MERMAID_ADAPTER_MESSAGE: &str = "Mermaid adapter failed";
const ADAPTER_ENV: &str = "RHIZOME_MERMAID_ADAPTER";
const ADAPTER_TIMEOUT: Duration = Duration::from_secs(5);

/// Validate Mermaid fenced blocks through the configured sidecar boundary.
/// The source parser remains authoritative: adapter failures become findings, never core errors.
pub fn check_mermaid(path: &Path, bytes: &[u8]) -> Vec<Diagnostic> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let blocks = collect_blocks(text);
    if blocks.is_empty() {
        return Vec::new();
    }
    let Some(adapter) = env::var_os(ADAPTER_ENV).filter(|value| !value.is_empty()) else {
        return vec![adapter_diagnostic(path)];
    };
    let request = serde_json::json!(
        blocks
            .iter()
            .map(|(line, code)| serde_json::json!({"line": line, "code": code}))
            .collect::<Vec<_>>()
    );
    let mut child = match Command::new(adapter)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return vec![adapter_diagnostic(path)],
    };
    let write_ok = child
        .stdin
        .take()
        .and_then(|mut stdin| stdin.write_all(request.to_string().as_bytes()).ok());
    if write_ok.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        return vec![adapter_diagnostic(path)];
    }
    let deadline = Instant::now() + ADAPTER_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = match child.wait_with_output() {
                    Ok(output) => output,
                    Err(_) => return vec![adapter_diagnostic(path)],
                };
                if !matches!(status.code(), Some(0) | Some(1)) {
                    return vec![adapter_diagnostic(path)];
                }
                return parse_findings(path, &output.stdout);
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return vec![adapter_diagnostic(path)];
            }
        }
    }
}

fn parse_findings(path: &Path, bytes: &[u8]) -> Vec<Diagnostic> {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return vec![adapter_diagnostic(path)];
    };
    let Some(findings) = value.get("findings").and_then(Value::as_array) else {
        return vec![adapter_diagnostic(path)];
    };
    let mut diagnostics = Vec::new();
    for finding in findings {
        let line = finding.get("line").and_then(Value::as_u64).unwrap_or(0);
        let message = finding
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or(MERMAID_INVALID_MESSAGE);
        diagnostics.push(
            Diagnostic::error(MERMAID_INVALID_CODE, message.to_owned())
                .at_path(path.to_path_buf())
                .for_field(format!("mermaid[{line}]")),
        );
    }
    diagnostics
}

pub fn check_note_mermaid(path: &Path, note: &ValidatedNote) -> Vec<Diagnostic> {
    check_mermaid(path, &note.body)
}

fn adapter_diagnostic(path: &Path) -> Diagnostic {
    Diagnostic::error(MERMAID_ADAPTER_CODE, MERMAID_ADAPTER_MESSAGE).at_path(path.to_path_buf())
}

fn collect_blocks(text: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut active: Option<(usize, u8, usize, String)> = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if let Some((start, marker, width, code)) = &mut active {
            let closes = trimmed.bytes().all(|byte| byte == *marker) && trimmed.len() >= *width;
            if closes {
                blocks.push((*start, std::mem::take(code)));
                active = None;
            } else {
                code.push_str(line);
                code.push('\n');
            }
            continue;
        }
        let bytes = trimmed.as_bytes();
        let Some(&marker) = bytes.first() else {
            continue;
        };
        if marker != b'`' && marker != b'~' {
            continue;
        }
        let width = bytes.iter().take_while(|byte| **byte == marker).count();
        if width >= 3 && trimmed[width..].trim().eq_ignore_ascii_case("mermaid") {
            active = Some((index + 1, marker, width, String::new()));
        }
    }
    if let Some((start, _, _, _)) = active {
        blocks.push((start, String::new()));
    }
    blocks
}
