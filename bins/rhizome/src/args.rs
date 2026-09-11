use clap::{Arg, ArgAction, Command};

pub fn command() -> Command {
    Command::new("rhizome")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Operate on Git-backed Markdown knowledge sources")
        .long_about(
            "Operate on Git-backed Markdown knowledge sources.\n\nMarkdown and Git remain authoritative; this CLI exposes only the source-plane contract.",
        )
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(new_command())
        .subcommand(check_command("check"))
        .subcommand(domains_command())
        .subcommand(adopt_command())
        .subcommand(doctor_command())
        .subcommand(amend_command())
        .subcommand(relocate_command())
        .subcommand(capture_command())
        .subcommand(index_command())
        .subcommand(stats_command())
}

fn json_arg() -> Arg {
    Arg::new("json")
        .long("json")
        .action(ArgAction::SetTrue)
        .help("Emit one JSON envelope")
}
fn source_arg() -> Arg {
    Arg::new("source")
        .long("source")
        .value_name("SOURCE")
        .help("Logical source name")
}
fn check_command(name: &'static str) -> Command {
    Command::new(name)
        .about("Check a source or Markdown note")
        .arg(json_arg())
        .arg(source_arg())
        .arg(Arg::new("path").value_name("PATH").num_args(0..))
}
fn new_command() -> Command {
    Command::new("new")
        .about("Author a new source note")
        .arg(json_arg())
        .arg(source_arg().required(true))
        .arg(
            Arg::new("domain")
                .long("domain")
                .value_name("DOMAIN")
                .required(true),
        )
        .arg(Arg::new("slug").value_name("SLUG").required(true))
        .arg(
            Arg::new("description")
                .long("description")
                .value_name("TEXT")
                .required(true),
        )
        .arg(
            Arg::new("keywords")
                .long("keywords")
                .value_name("WORD")
                .action(ArgAction::Append)
                .required(true),
        )
        .arg(
            Arg::new("kind")
                .long("kind")
                .value_name("KIND")
                .default_value("note"),
        )
        .arg(
            Arg::new("assets")
                .long("assets")
                .value_name("ASSET")
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("body-file")
                .long("body-file")
                .value_name("PATH")
                .required(true),
        )
}
fn domains_command() -> Command {
    Command::new("domains")
        .about("List deterministic C2 domains")
        .arg(json_arg())
        .arg(source_arg())
}
fn adopt_command() -> Command {
    Command::new("adopt")
        .about("Adopt an existing Git repository as a logical source")
        .arg(json_arg())
        .arg(
            Arg::new("registry")
                .long("registry")
                .value_name("PATH")
                .required(true),
        )
        .arg(source_arg().required(true))
        .arg(
            Arg::new("repo")
                .long("repo")
                .value_name("PATH")
                .required(true),
        )
        .arg(
            Arg::new("description")
                .long("description")
                .value_name("TEXT")
                .required(true),
        )
        .arg(
            Arg::new("keywords")
                .long("keywords")
                .value_name("WORD")
                .action(ArgAction::Append)
                .required(true),
        )
}
fn doctor_command() -> Command {
    Command::new("doctor")
        .about("Diagnose source-plane configuration")
        .arg(json_arg())
        .arg(source_arg())
}
fn amend_command() -> Command {
    Command::new("amend")
        .about("Amend a frozen note with explicit approval")
        .arg(json_arg())
        .arg(source_arg().required(true))
        .arg(
            Arg::new("path")
                .long("path")
                .value_name("PATH")
                .required(true),
        )
        .arg(
            Arg::new("body-file")
                .long("body-file")
                .value_name("PATH")
                .required(true),
        )
        .arg(
            Arg::new("reason")
                .long("reason")
                .value_name("TEXT")
                .required(true),
        )
}
fn relocate_command() -> Command {
    Command::new("relocate")
        .about("Relocate a frozen note between C2 domains")
        .arg(json_arg())
        .arg(
            Arg::new("source")
                .long("source")
                .value_name("SOURCE")
                .required(true),
        )
        .arg(
            Arg::new("path")
                .long("path")
                .value_name("PATH")
                .required(true),
        )
        .arg(
            Arg::new("target")
                .long("target")
                .value_name("IDENTITY")
                .required(true),
        )
}
fn capture_command() -> Command {
    Command::new("capture")
        .about("Append a raw thought to the configured inbox")
        .arg(json_arg())
        .arg(
            Arg::new("text")
                .value_name("TEXT")
                .num_args(1..)
                .required(true),
        )
}
fn index_command() -> Command {
    Command::new("index")
        .about("Manage generated human indexes")
        .subcommand_required(true)
        .subcommand(check_command("check"))
        .subcommand(
            Command::new("sync")
                .about("Synchronize generated human indexes")
                .arg(json_arg())
                .arg(Arg::new("force").long("force").action(ArgAction::SetTrue)),
        )
}
fn stats_command() -> Command {
    Command::new("stats")
        .about("Show deterministic source statistics")
        .arg(json_arg())
        .arg(source_arg())
}
