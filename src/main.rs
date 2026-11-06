#![allow(dead_code, unused_imports)] // scaffold: types defined ahead of the modules that use them

mod cli;
mod duration;
mod error;
mod types;

use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = cli::Cli::parse_normalized();
    let (options, warnings) = match cli.into_options() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("ERROR: {e}");
            return ExitCode::FAILURE;
        }
    };
    for w in warnings {
        eprintln!("WARNING: {w}");
    }

    // Download pipeline not ported yet; show the resolved options.
    println!("{options:#?}");
    ExitCode::SUCCESS
}
