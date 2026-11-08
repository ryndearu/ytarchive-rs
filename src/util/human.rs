//! Human-readable sizes and durations for status output.

const KIB: f64 = 1024.0;
const MIB: f64 = KIB * 1024.0;
const GIB: f64 = MIB * 1024.0;

pub fn format_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.2}GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.2}MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.2}KiB", b / KIB)
    } else {
        format!("{bytes}B")
    }
}

/// `1d2h3m4s`; zero-valued leading units are omitted, seconds always shown.
pub fn seconds_to_duration_str(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3600;
    let minutes = seconds % 3600 / 60;
    let secs = seconds % 60;

    let mut out = String::new();
    if days > 0 {
        out.push_str(&format!("{days}d"));
    }
    if hours > 0 {
        out.push_str(&format!("{hours}h"));
    }
    if minutes > 0 {
        out.push_str(&format!("{minutes}m"));
    }
    out.push_str(&format!("{secs}s"));
    out
}

/// `H:MM:SS` when an hour or more, otherwise `MM:SS`. Hours are not wrapped into days.
pub fn seconds_to_time_str(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    let secs = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes:02}:{secs:02}")
    }
}

/// `1h1m1s (1:01:01)`
pub fn seconds_to_duration_and_time_str(seconds: u64) -> String {
    format!(
        "{} ({})",
        seconds_to_duration_str(seconds),
        seconds_to_time_str(seconds)
    )
}
