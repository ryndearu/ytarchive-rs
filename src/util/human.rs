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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(format_size(0), "0B");
        assert_eq!(format_size(1023), "1023B");
        assert_eq!(format_size(1024), "1.00KiB");
        assert_eq!(format_size(1536 * 1024), "1.50MiB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.00GiB");
    }

    #[test]
    fn durations() {
        assert_eq!(seconds_to_duration_str(0), "0s");
        assert_eq!(seconds_to_duration_str(3600), "1h0s");
        assert_eq!(seconds_to_duration_str(90_061), "1d1h1m1s");
        assert_eq!(seconds_to_time_str(65), "01:05");
        assert_eq!(seconds_to_time_str(90_061), "25:01:01");
        assert_eq!(seconds_to_duration_and_time_str(3661), "1h1m1s (1:01:01)");
    }
}
