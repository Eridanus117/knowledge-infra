#![forbid(unsafe_code)]

use clap::Command;

fn command() -> Command {
    Command::new("rhizome")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Operate on Git-backed Markdown knowledge sources")
        .long_about(
            "Operate on Git-backed Markdown knowledge sources.\n\n\
             Markdown and Git remain authoritative; this bootstrap exposes only the stable CLI boundary.",
        )
}

fn main() {
    command().get_matches();
}
