use kb_contract::{Diagnostic, Severity};
use serde_json::{Value, json};
use std::io::{self, Write};

pub const CLI_SCHEMA: &str = "rhizome-cli-v2";

#[must_use]
pub fn diagnostic_value(diagnostic: &Diagnostic) -> Value {
    json!({
        "code": diagnostic.code,
        "severity": match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        },
        "path": diagnostic.path.as_ref().map(|path| path.to_string_lossy().into_owned()),
        "field": diagnostic.field,
        "message": diagnostic.message,
    })
}

#[must_use]
pub fn envelope(ok: bool, data: Value, diagnostics: &[Diagnostic]) -> Value {
    json!({
        "schema": CLI_SCHEMA,
        "ok": ok,
        "data": data,
        "diagnostics": diagnostics.iter().map(diagnostic_value).collect::<Vec<_>>(),
    })
}

pub fn emit(value: &Value) -> io::Result<()> {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    serde_json::to_writer(&mut lock, value).map_err(io::Error::other)?;
    lock.write_all(b"\n")?;
    lock.flush()
}

pub fn emit_human(value: &Value) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if let Some(data) = value.get("data") {
        let encoded = serde_json::to_string(data).map_err(io::Error::other)?;
        writeln!(stdout, "data: {encoded}")?;
    }
    if let Some(diagnostics) = value.get("diagnostics").and_then(Value::as_array) {
        for diagnostic in diagnostics {
            let code = diagnostic
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("diagnostic");
            let message = diagnostic
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("");
            writeln!(stdout, "{code}: {message}")?;
        }
    }
    if !ok && value.get("data").is_none() {
        stdout.write_all(b"failure\n")?;
    }
    stdout.flush()
}
