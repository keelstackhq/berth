#![forbid(unsafe_code)]

use berth::{BerthError, Cli, Report, render_json, render_table};
use clap::Parser;
use clap::error::ErrorKind;
use std::io::IsTerminal;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            let code = match err.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => 0,
                _ => 2,
            };
            let _ = err.print();
            if code == 2 {
                print_usage_json(&err);
            }
            return ExitCode::from(code);
        }
    };
    berth::set_verbose(cli.verbose);
    let json = cli.json;
    let color =
        !cli.no_color && std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal();
    let command = cli.command.name();
    match berth::run(cli) {
        Ok(report) => {
            print_report(&report, json, color);
            ExitCode::from(report.exit_code as u8)
        }
        Err(err) => {
            eprintln!("error: {err}");
            if json {
                println!("{}", render_json(&Report::from_error(command, &err)));
            }
            ExitCode::from(err.exit_code() as u8)
        }
    }
}

fn print_report(report: &Report, json: bool, color: bool) {
    if json {
        println!("{}", render_json(report));
    } else if let Some(text) = &report.text {
        print!("{text}");
    } else {
        print!("{}", render_table(report, color));
    }
}

const COMMANDS: [&str; 6] = ["up", "down", "ls", "env", "gc", "doctor"];

/// A clap usage error with `--json` anywhere in argv still prints one Report (exit 2) on stdout;
/// the usage text has already gone to stderr.
fn print_usage_json(err: &clap::Error) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "--json") {
        return;
    }
    let message = err.render().to_string();
    let first = message.lines().next().unwrap_or("usage error");
    let reason = first.strip_prefix("error: ").unwrap_or(first).to_string();
    let report = Report::from_error(command_from_args(&args), &BerthError::Usage(reason));
    println!("{}", render_json(&report));
}

/// The subcommand named by the first positional argument, or `"unknown"`.
fn command_from_args(args: &[String]) -> &'static str {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-C" || arg == "--format" {
            iter.next();
        } else if !arg.starts_with('-') {
            return COMMANDS.into_iter().find(|c| c == arg).unwrap_or("unknown");
        }
    }
    "unknown"
}
