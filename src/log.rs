//! Leveled, colored logging to stderr, matching Go ytarchive's output format:
//! `2006/01/02 15:04:05 LEVEL: message`.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::types::LogLevel;

static LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Warning as u8);

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
    let ts = chrono::Local::now().format("%Y/%m/%d %H:%M:%S");
    eprintln!("{ts} {}", format_line(at, general, &args.to_string()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_roundtrip_and_filtering() {
        for l in [
            LogLevel::Quiet,
            LogLevel::Error,
            LogLevel::Warning,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            set_level(l);
            assert_eq!(level(), l);
        }
        set_level(LogLevel::Warning);
        assert!(enabled(LogLevel::Error));
        assert!(enabled(LogLevel::Warning));
        assert!(!enabled(LogLevel::Info));
    }

    #[test]
    fn line_format() {
        assert_eq!(
            format_line(LogLevel::Error, false, "boom"),
            "ERROR: \x1b[31mboom\x1b[0m\x1b[K"
        );
        assert_eq!(format_line(LogLevel::Error, true, "plain"), "plain");
    }
}
