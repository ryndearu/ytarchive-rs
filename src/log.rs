//! Leveled, colored logging to stderr, matching Go ytarchive's output format:
//! `2006/01/02 15:04:05 LEVEL: message`.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use tokio::sync::mpsc;

use crate::types::{DataType, LogLevel, ProgressInfo};
use crate::util::format_size;

/// The "Video Fragments: N; Audio Fragments: M; ..." status line.
#[derive(Debug, Default, Clone, Copy)]
pub struct Totals {
    pub audio_frags: u64,
    pub video_frags: u64,
    pub bytes: u64,
    pub max_seq: Option<u64>,
    pub start_frag: u64,
}

impl Totals {
    pub fn add(&mut self, progress: &ProgressInfo) {
        match progress.data_type {
            DataType::Audio => self.audio_frags += 1,
            DataType::Video => self.video_frags += 1,
        }
        self.bytes += progress.byte_count;
        if progress.max_seq > self.max_seq {
            self.max_seq = progress.max_seq;
        }
        self.start_frag = progress.start_frag;
    }

    pub fn line(&self, verbose: bool) -> String {
        let mut text = format!("Video Fragments: {}; Audio Fragments: {}; ", self.video_frags, self.audio_frags);
        if verbose && let Some(max) = self.max_seq {
            text.push_str(&format!(
                "Max Fragments: {}; Max Sequence: {max}; ",
                (max + 1).saturating_sub(self.start_frag)
            ));
        }
        text.push_str(&format!("Total Downloaded: {}", format_size(self.bytes)));
        text
    }
}

/// Prints a status line per progress message until the channel closes.
/// Starts from `initial` (resumed fragment counts).
pub async fn report_progress(
    mut receiver: mpsc::UnboundedReceiver<ProgressInfo>,
    initial: Totals,
    newlines: bool,
) -> Totals {
    let mut totals = initial;
    let verbose = enabled(LogLevel::Info);
    while let Some(progress) = receiver.recv().await {
        totals.add(&progress);
        if level() > LogLevel::Quiet {
            if newlines {
                eprintln!("{}", totals.line(verbose));
            } else {
                eprint!("\r{}\x1b[K", totals.line(verbose));
            }
        }
    }
    totals
}

static LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Warning as u8);
/// When false (default), log lines start with `\r` to overwrite the status line.
static NEWLINES: AtomicBool = AtomicBool::new(false);

pub fn set_newlines(on: bool) {
    NEWLINES.store(on, Ordering::Relaxed);
}

pub fn set_level(level: LogLevel) {
    LEVEL.store(level as u8, Ordering::Relaxed);
}

pub fn level() -> LogLevel {
    match LEVEL.load(Ordering::Relaxed) {
        0 => LogLevel::Quiet,
        1 => LogLevel::Error,
        2 => LogLevel::Warning,
        3 => LogLevel::Info,
        4 => LogLevel::Debug,
        _ => LogLevel::Trace,
    }
}

pub fn enabled(at: LogLevel) -> bool {
    level() >= at
}

/// Label and ANSI color for each level. `None` label = general message.
fn style(at: LogLevel, general: bool) -> (Option<&'static str>, &'static str) {
    if general {
        return (None, "");
    }
    match at {
        LogLevel::Error => (Some("ERROR"), "\x1b[31m"),
        LogLevel::Warning => (Some("WARNING"), "\x1b[33m"),
        LogLevel::Info => (Some("INFO"), "\x1b[32m"),
        LogLevel::Debug => (Some("DEBUG"), "\x1b[36m"),
        LogLevel::Trace => (Some("TRACE"), "\x1b[35m"),
        LogLevel::Quiet => (None, ""),
    }
}

/// Builds the line without the timestamp; split out for testing.
pub fn format_line(at: LogLevel, general: bool, msg: &str) -> String {
    match style(at, general) {
        (Some(label), color) => format!("{label}: {color}{msg}\x1b[0m\x1b[K"),
        (None, _) => msg.to_string(),
    }
}

#[doc(hidden)]
pub fn write(at: LogLevel, general: bool, args: std::fmt::Arguments<'_>) {
    if !enabled(at) {
        return;
    }
    let timestamp = chrono::Local::now().format("%Y/%m/%d %H:%M:%S");
    let line_start = if NEWLINES.load(Ordering::Relaxed) { "" } else { "\r" };
    eprintln!("{line_start}{timestamp} {}", format_line(at, general, &args.to_string()));
}

/// Status line while polling for a stream. Rewrites the same line unless
/// `newlines` (`--newline`) is set.
pub fn retry_status(retries: u32, total_waited_secs: u64, newlines: bool) {
    if level() <= LogLevel::Quiet {
        return;
    }
    let now = chrono::Local::now().format("%Y/%m/%d %H:%M:%S");
    let msg = format!("Retries: {retries} (Last retry: {now}), Total time waited: {total_waited_secs} seconds");
    if newlines {
        eprintln!("{msg}");
    } else {
        eprint!("\r{msg}");
    }
}

/// Shown at every level except quiet, without a label.
#[macro_export]
macro_rules! log_general {
    ($($arg:tt)*) => { $crate::log::write($crate::types::LogLevel::Error, true, format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => { $crate::log::write($crate::types::LogLevel::Error, false, format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::log::write($crate::types::LogLevel::Warning, false, format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::log::write($crate::types::LogLevel::Info, false, format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => { $crate::log::write($crate::types::LogLevel::Debug, false, format_args!($($arg)*)) };
}
#[macro_export]
macro_rules! log_trace {
    ($($arg:tt)*) => { $crate::log::write($crate::types::LogLevel::Trace, false, format_args!($($arg)*)) };
}
