#![forbid(unsafe_code)]

mod args;
mod commands;
mod output;
mod telemetry;

fn main() -> std::process::ExitCode {
    let parser = args::command();
    let matches = match parser.try_get_matches() {
        Ok(matches) => matches,
        Err(error) => {
            use clap::error::ErrorKind;
            let code = match error.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => 0,
                _ => 64,
            };
            if code == 0 {
                print!("{error}");
            } else if json_requested_from_process_args() {
                let value = output::envelope(
                    false,
                    serde_json::Value::Null,
                    &[kb_contract::Diagnostic::error(
                        "KBV2-CLI-USAGE",
                        "invalid command-line arguments",
                    )],
                );
                if let Err(output_error) = output::emit(&value) {
                    eprintln!("could not write CLI output: {output_error}");
                    return std::process::ExitCode::from(1);
                }
            } else {
                eprint!("{error}");
            }
            return std::process::ExitCode::from(code as u8);
        }
    };
    let result = commands::execute(&matches);
    let emit_result = if json_requested(&matches) {
        output::emit(&result.value)
    } else {
        output::emit_human(&result.value)
    };
    if let Err(error) = emit_result {
        eprintln!("could not write CLI output: {error}");
        return std::process::ExitCode::from(1);
    }
    std::process::ExitCode::from(result.exit_code as u8)
}

fn json_requested_from_process_args() -> bool {
    std::env::args()
        .skip(1)
        .any(|argument| argument == "--json")
}

fn json_requested(matches: &clap::ArgMatches) -> bool {
    let Some((_, subcommand)) = matches.subcommand() else {
        return false;
    };
    subcommand
        .try_get_one::<bool>("json")
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
        || subcommand.subcommand().is_some_and(|(_, nested)| {
            nested
                .try_get_one::<bool>("json")
                .ok()
                .flatten()
                .copied()
                .unwrap_or(false)
        })
}
