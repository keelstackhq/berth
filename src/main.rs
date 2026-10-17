#![forbid(unsafe_code)]

use berth::{Cli, Report, render_json, render_table};
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
