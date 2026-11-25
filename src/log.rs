//! Leveled, colored logging to stderr, matching Go ytarchive's output format:
//! `2006/01/02 15:04:05 LEVEL: message`.

use std::env;
use std::fmt::Arguments;
use std::io::{self, IsTerminal};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use tokio::sync::mpsc;

use crate::types::{DataType, LogLevel, PerType, ProgressInfo};
use crate::util::format_size;

/// The "Video Fragments: N; Audio Fragments: M; ..." status line.
#[derive(Debug, Default, Clone, Copy)]
pub struct Totals {
    pub audio_frags: u64,
    pub video_frags: u64,
    pub bytes: u64,
    pub max_seq: Option<u64>,
    pub start_frag: PerType<Option<u64>>,
}

impl Totals {
    pub fn add(&mut self, progress: &ProgressInfo) {
        match progress.data_type {
            DataType::Audio => self.audio_frags += progress.fragments,
            DataType::Video => self.video_frags += progress.fragments,
        }
        self.bytes += progress.byte_count;
        if progress.max_seq > self.max_seq {
            self.max_seq = progress.max_seq;
        }
        *self.start_frag.get_mut(progress.data_type) = Some(progress.start_frag);
    }

    pub fn line(&self, verbose: bool) -> String {
        let mut text = format!("Video Fragments: {}; Audio Fragments: {}; ", self.video_frags, self.audio_frags);
        if verbose && let Some(max) = self.max_seq {
            let start_frag = [self.start_frag.audio, self.start_frag.video].into_iter().flatten().min().unwrap_or(0);
            text.push_str(&format!("Max Fragments: {}; Max Sequence: {max}; ", (max + 1).saturating_sub(start_frag)));
        }
        text.push_str(&format!("Total Downloaded: {}", format_size(self.bytes)));
        text
    }
}

/// Prints a status line per progress message until the channel closes.
pub async fn report_progress(mut receiver: mpsc::UnboundedReceiver<ProgressInfo>, newlines: bool) -> Totals {
    let mut totals = Totals::default();
    let verbose = enabled(LogLevel::Info);
    while let Some(progress) = receiver.recv().await {
        totals.add(&progress);
        if level() > LogLevel::Quiet {
            if newlines || !is_tty() {
                INLINE_ACTIVE.store(false, Ordering::Relaxed);
                eprintln!("{}", totals.line(verbose));
            } else {
                INLINE_ACTIVE.store(true, Ordering::Relaxed);
                eprint!("\r{}\x1b[K", totals.line(verbose));
            }
        }
    }
    totals
}

static LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Warning as u8);
/// When false (default), log lines start with `\r` to overwrite the status line.
static NEWLINES: AtomicBool = AtomicBool::new(false);
/// Whether an inline `\r` status line is currently sitting on stderr without a trailing newline.
static INLINE_ACTIVE: AtomicBool = AtomicBool::new(false);

fn is_tty() -> bool {
    static TTY: OnceLock<bool> = OnceLock::new();
    *TTY.get_or_init(|| io::stderr().is_terminal())
}

fn use_color() -> bool {
    static COLOR: OnceLock<bool> = OnceLock::new();
    *COLOR.get_or_init(|| is_tty() && env::var_os("NO_COLOR").is_none_or(|value| value.is_empty()))
}

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
fn format_line_styled(at: LogLevel, general: bool, msg: &str, color_on: bool, clear_line: bool) -> String {
    let clear = if clear_line { "\x1b[K" } else { "" };
    match style(at, general) {
        (Some(label), color) if color_on => format!("{label}: {color}{msg}\x1b[0m{clear}"),
        (Some(label), _) => format!("{label}: {msg}{clear}"),
        (None, _) => format!("{msg}{clear}"),
    }
}

#[doc(hidden)]
pub fn write(at: LogLevel, general: bool, args: Arguments<'_>) {
    if !enabled(at) {
        return;
    }
    let timestamp = chrono::Local::now().format("%Y/%m/%d %H:%M:%S");
    let inline = is_tty() && !NEWLINES.load(Ordering::Relaxed);
    let line_start = if inline { "\r" } else { "" };
    INLINE_ACTIVE.store(false, Ordering::Relaxed);
    eprintln!("{line_start}{timestamp} {}", format_line_styled(at, general, &args.to_string(), use_color(), inline));
}

/// Terminates an active inline `\r` status line with a newline. No-op under
/// `--quiet`, `--newline`, non-TTY stderr, or when no inline line is active.
pub fn finish_inline() {
    if INLINE_ACTIVE.swap(false, Ordering::Relaxed) && level() > LogLevel::Quiet {
        eprintln!();
    }
}

/// Status line while polling for a stream. Rewrites the same line unless
/// `newlines` (`--newline`) is set or stderr is not a TTY.
pub fn retry_status(retries: u32, total_waited_secs: u64, newlines: bool) {
    if level() <= LogLevel::Quiet {
        return;
    }
    let now = chrono::Local::now().format("%Y/%m/%d %H:%M:%S");
    let msg = format!("Retries: {retries} (Last retry: {now}), Total time waited: {total_waited_secs} seconds");
    if newlines || !is_tty() {
        INLINE_ACTIVE.store(false, Ordering::Relaxed);
        eprintln!("{msg}");
    } else {
        INLINE_ACTIVE.store(true, Ordering::Relaxed);
        eprint!("\r{msg}\x1b[K");
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
