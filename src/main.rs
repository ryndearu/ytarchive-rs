mod cli;
mod cookies;
mod download;
mod duration;
mod error;
mod finish;
mod formats;
mod gvideo;
mod http;
mod input_url;
mod interrupt;
mod live_from;
mod log;
mod monitor;
mod mp4_atoms;
mod mux;
mod output;
mod prepare;
mod progress;
mod prompt;
mod run;
mod session;
mod stream_meta;
mod types;
mod util;
mod video_info;
mod watch_page;
mod ytdlp;

use std::process::ExitCode;
use std::sync::Arc;

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
    log::set_newlines(options.status_newlines);
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

    let http = match http::Http::new(&options, cookie_set) {
        Ok(h) => h,
        Err(e) => {
            log_error!("Failed to create HTTP client: {e}");
            return ExitCode::FAILURE;
        }
    };

    log_debug!("{options:#?}");
    let interrupt = interrupt::Interrupt::install();
    monitor::start(Arc::new(options), http, interrupt).await
}
