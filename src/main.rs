#![allow(dead_code, unused_imports)] // scaffold: types defined ahead of the modules that use them

mod cli;
mod cookies;
mod duration;
mod error;
mod http;
mod log;
mod types;
mod util;
mod ytdlp;

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
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

    let cookie_set = match &options.cookies_file {
        Some(path) => match cookies::load_file(path) {
            Ok(set) => {
                log_info!("Loaded cookie file {}", path.display());
                set
            }
            Err(e) => {
                log_error!("Failed to load cookies file: {e}");
                return ExitCode::FAILURE;
            }
        },
        None => types::CookieSet::default(),
    };

    let _http = match http::Http::new(&options, cookie_set) {
        Ok(h) => h,
        Err(e) => {
            log_error!("Failed to create HTTP client: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Download pipeline not ported yet; show the resolved options.
    log_debug!("{options:#?}");
    log_general!("ytarchive-rs: download pipeline not implemented yet");
    ExitCode::SUCCESS
}
