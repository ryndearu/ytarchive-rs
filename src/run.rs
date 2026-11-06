//! Top-level flow for one stream: resolve the URL, fetch info, and (later)
//! download and mux.

use std::process::ExitCode;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::http::Http;
use crate::input_url::{parse_input_url, InputUrl};
use crate::live_from::resolve_live_from;
use crate::prompt::get_user_input;
use crate::types::{FormatSelection, Options};
use crate::util::seconds_to_duration_and_time_str;
use crate::video_info::Extractor;
use crate::{log_error, log_general};

pub async fn run(opts: Arc<Options>, http: Http, cancel: CancellationToken) -> ExitCode {
    let raw_url = match &opts.url {
        Some(u) => u.clone(),
        None => get_user_input("Enter a youtube livestream URL: ").await,
    };
    let input = match parse_input_url(&raw_url) {
        Ok(i) => i,
        Err(e) => {
            log_error!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let mut ex = Extractor::new(opts.clone(), http, cancel, &input);
    let Some(selection) = ex.get_video_info().await else {
        return ExitCode::FAILURE;
    };

    let live_from_sq = match (&opts.live_from, ex.last_sq) {
        (Some(v), Some(last)) => match resolve_live_from(v, last, ex.target_duration) {
            Ok(lf) => {
                log_general!("{}", lf.message);
                Some(lf.start_sq)
            }
            Err(e) => {
                log_error!("{e}");
                return ExitCode::FAILURE;
            }
        },
        (Some(_), None) => {
            log_error!("--live-from needs the current stream position, which could not be determined");
            return ExitCode::FAILURE;
        }
        _ => None,
    };

    if opts.info_only {
        print_info(&ex, &selection, &input);
        return ExitCode::SUCCESS;
    }

    let _ = live_from_sq;
    log_general!("ytarchive-rs: downloading is not implemented yet");
    ExitCode::SUCCESS
}

fn print_info(ex: &Extractor, sel: &FormatSelection, input: &InputUrl) {
    let fi = &ex.format_info;
    let quality = match (sel.codec, sel.video_itag) {
        (Some(c), Some(itag)) => format!("{} ({}, itag {itag})", sel.label, c.name()),
        (None, Some(itag)) => format!("itag {itag}"),
        _ => "audio_only".into(),
    };
    println!("Video ID: {}", ex.video_id);
    println!("Title: {}", fi.get("title").map_or("", String::as_str));
    println!("Channel: {}", fi.get("channel").map_or("", String::as_str));
    println!("Selected quality: {quality}");
    println!("Live: {}", ex.live);
    if let Some(last) = ex.last_sq {
        println!(
            "Duration so far: {} ({} fragments of {}s)",
            seconds_to_duration_and_time_str(last * u64::from(ex.target_duration)),
            last,
            ex.target_duration
        );
    }
    if let InputUrl::Channel { live_url } = input {
        println!("Resolved from: {live_url}");
    }
}
