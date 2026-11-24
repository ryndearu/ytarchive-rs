// Unsafe code is only allowed where it's truly necessary; there is none now.
#![forbid(unsafe_code)]

/// Version string stamped by build.rs: `v0.1.0 (date)` for releases,
/// `v0.1.0+<commit> (date)` for nightly and other builds.
const VERSION: &str = env!("YTARCHIVE_VERSION_STRING");

mod cli;
mod cookies;
mod download;
mod duration;
mod error;
mod finish;
mod formats;
mod http;
mod input_url;
mod interrupt;
mod live_from;
mod log;
mod monitor;
mod mp4_atoms;
mod mux;
mod output;
mod platform;
mod prepare;
mod prompt;
mod run;
mod session;
mod stream_meta;
mod types;
mod util;
mod video_info;
mod watch_page;
mod ytdlp;
mod ytdlp_cookies;

use std::process::ExitCode;
use std::sync::Arc;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = cli::Cli::parse_normalized();
    let mut options = match cli.into_options() {
        Ok(options) => options,
        Err(error) => {
            log_error!("{error}");
            return ExitCode::FAILURE;
        }
    };
    log::set_level(options.log_level);
    log::set_newlines(options.status_newlines);
    // Restored when main returns; runs after log setup so --debug captures
    // Windows setup messages and --help/--version don't alter console state.
    let _console = platform::setup();
    // Go printed its version at startup too.
    if options.log_level > types::LogLevel::Quiet {
        eprintln!("ytarchive {VERSION}");
    }

    // Validate the output template early, as Go does, before any network work.
    let known_keys = types::new_format_info();
    if let Err(error) = util::format_python_map(&options.output_format, &known_keys) {
        log_error!("{error}");
        return ExitCode::FAILURE;
    }
    for (key, value) in &options.metadata_overrides {
        if let Err(error) = util::format_python_map(value, &known_keys) {
            log_warn!("--metadata {key}: {error}; value will be kept literally");
        }
    }

    let _cookies = ytdlp_cookies::guard();
    let cookie_set = match options.cookies_file.clone() {
        Some(path) => match std::fs::read(&path) {
            Ok(data) => {
                log_info!("Loaded cookie file {}", path.display());
                match ytdlp_cookies::prepare(&path, &data) {
                    Ok(copy) => options.ytdlp_cookies_file = copy,
                    Err(error) => log_warn!("Couldn't write a repaired cookies file for yt-dlp: {error}"),
                }
                cookies::parse_netscape(&data)
            }
            Err(error) => {
                log_error!("Failed to load cookies file: {error}");
                return ExitCode::FAILURE;
            }
        },
        None => types::CookieSet::default(),
    };

    let http = match http::Http::new(&options, cookie_set) {
        Ok(http) => http,
        Err(error) => {
            log_error!("Failed to create HTTP client: {error}");
            return ExitCode::FAILURE;
        }
    };

    if log::enabled(types::LogLevel::Debug) {
        let mut logged = options.clone();
        logged.url = logged.url.as_deref().map(http::redact_url_for_log);
        logged.video_url = logged.video_url.as_deref().map(http::redact_url_for_log);
        logged.audio_url = logged.audio_url.as_deref().map(http::redact_url_for_log);
        logged.proxy = logged.proxy.as_ref().map(http::redact_proxy);
        logged.ytdlp_opts = http::redact_proxy_args(&logged.ytdlp_opts);
        log_debug!("{logged:#?}");
    }
    let interrupt = interrupt::Interrupt::install();
    monitor::start(Arc::new(options), http, interrupt).await
}
