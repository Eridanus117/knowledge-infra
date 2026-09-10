use std::process::Command;

fn rhizome() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rhizome"))
}

#[test]
fn version_is_consumer_visible() {
    let output = rhizome()
        .arg("--version")
        .output()
        .expect("rhizome binary should run");

    assert!(output.status.success(), "--version should succeed");
    assert_eq!(
        String::from_utf8(output.stdout).expect("version output should be UTF-8"),
        concat!("rhizome ", env!("CARGO_PKG_VERSION"), "\n")
    );
    assert!(output.stderr.is_empty(), "--version should not use stderr");
}

#[test]
fn help_describes_the_source_plane_boundary() {
    let output = rhizome()
        .arg("--help")
        .output()
        .expect("rhizome binary should run");

    assert!(output.status.success(), "--help should succeed");
    let stdout = String::from_utf8(output.stdout).expect("help output should be UTF-8");
    assert!(
        stdout.contains("Operate on Git-backed Markdown knowledge sources"),
        "help should state the CLI purpose"
    );
    assert!(
        stdout.contains("Usage: rhizome"),
        "help should expose clap usage"
    );
    assert!(output.stderr.is_empty(), "--help should not use stderr");
}
