#![allow(dead_code, unused_imports)] // scaffold: types defined ahead of the modules that use them

mod cli;
mod duration;
mod error;
mod log;
mod types;
mod util;

use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = cli::Cli::parse_normalized();
    let (options, warnings) = match cli.into_options() {
        Ok(v) => v,
        Err(e) => {
            log_error!("{e}");
            return ExitCode::FAILURE;
        }
    };
    log::set_level(options.log_level);
    for w in warnings {
        log_warn!("{w}");
    }

    // Validate the output template early, as Go does, before any network work.
    if let Err(e) =
        util::format_python_map(&options.output_format, &types::new_format_info())
    {
        log_error!("{e}");
        return ExitCode::FAILURE;
    }

    // Download pipeline not ported yet; show the resolved options.
    log_debug!("{options:#?}");
    log_general!("ytarchive-rs: download pipeline not implemented yet");
    ExitCode::SUCCESS
}
