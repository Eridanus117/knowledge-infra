use kb_contract::{Diagnostic, ValidatedNote};
use serde_json::Value;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
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
    let deadline = Instant::now() + ADAPTER_TIMEOUT;
    let payload = request.to_string();
    let (stdin_path, mut stdin) = match create_temp_file("stdin") {
        Ok(file) => file,
        Err(_) => return vec![adapter_diagnostic(path)],
    };
    let _stdin_guard = TempFilePath(stdin_path);
    if stdin.write_all(payload.as_bytes()).is_err()
        || stdin.flush().is_err()
        || stdin.sync_all().is_err()
        || stdin.seek(SeekFrom::Start(0)).is_err()
        || Instant::now() >= deadline
    {
        return vec![adapter_diagnostic(path)];
    }
    if Instant::now() >= deadline {
        return vec![adapter_diagnostic(path)];
    }
    let (stdout_path, stdout) = match create_temp_file("stdout") {
        Ok(file) => file,
        Err(_) => return vec![adapter_diagnostic(path)],
    };
    let _stdout_guard = TempFilePath(stdout_path.clone());
    if Instant::now() >= deadline {
        return vec![adapter_diagnostic(path)];
    }
    let mut child = match Command::new(adapter)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return vec![adapter_diagnostic(path)],
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) | Err(_) => {
                kill_and_reap(&mut child);
                return vec![adapter_diagnostic(path)];
            }
        }
    };
    if !matches!(status.code(), Some(0) | Some(1)) {
        return vec![adapter_diagnostic(path)];
    }
    let (output, overflow) = match read_stdout_bounded(&stdout_path, deadline) {
        Ok(result) => result,
        Err(()) => return vec![adapter_diagnostic(path)],
    };
    if overflow {
        return vec![adapter_diagnostic(path)];
    }
    parse_findings(path, &output)
}

fn create_temp_file(suffix: &str) -> std::io::Result<(PathBuf, File)> {
    static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);
    let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let directory = env::temp_dir();
    let process = std::process::id();
    for attempt in 0..16 {
        let path = directory.join(format!(
            "rhizome-mermaid-{process}-{sequence}-{attempt}-{suffix}.tmp"
        ));
        match OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique Mermaid adapter temporary file",
    ))
}

struct TempFilePath(PathBuf);

impl Drop for TempFilePath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn read_stdout_bounded(path: &Path, deadline: Instant) -> Result<(Vec<u8>, bool), ()> {
    const MAX_OUTPUT: usize = 8 * 1024 * 1024;
    let mut stdout = File::open(path).map_err(|_| ())?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut overflow = false;
    loop {
        if Instant::now() >= deadline {
            return Err(());
        }
        let size = stdout.read(&mut buffer).map_err(|_| ())?;
        if size == 0 {
            break;
        }
        let keep = size.min(MAX_OUTPUT.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..keep]);
        overflow |= keep < size;
        if overflow {
            break;
        }
    }
    Ok((bytes, overflow))
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
