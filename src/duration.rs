//! Parsing for duration strings (`1d8h10m`, `-30m`) and time strings (`12:30:05`).
//! Mirrors Go's str2duration + hhmmss behaviour used by ytarchive.

use std::time::Duration;

use crate::error::{Error, Result};

/// A duration with a sign, for `--live-from` which may seek backwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignedDuration {
    pub negative: bool,
    pub duration: Duration,
}

/// Parses `[-+]<num><unit>...` with units w, d, h, m, s, ms, us/µs, ns.
/// Fractions are allowed (`1.5h`). `"0"` is accepted on its own.
pub fn parse_duration_units(input: &str) -> Option<SignedDuration> {
    let s = input.trim();
    let (negative, mut rest) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    if rest == "0" {
        return Some(SignedDuration { negative, duration: Duration::ZERO });
    }
    if rest.is_empty() {
        return None;
    }

    let mut total_nanos: f64 = 0.0;
    while !rest.is_empty() {
        let num_end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
        if num_end == 0 {
            return None;
        }
        let value: f64 = rest[..num_end].parse().ok()?;
        rest = &rest[num_end..];

        let unit_end = rest.find(|c: char| c.is_ascii_digit() || c == '.').unwrap_or(rest.len());
        let unit = &rest[..unit_end];
        rest = &rest[unit_end..];

        let nanos_per_unit: f64 = match unit {
            "ns" => 1.0,
            "us" | "µs" => 1e3,
            "ms" => 1e6,
            "s" => 1e9,
            "m" => 60e9,
            "h" => 3600e9,
            "d" => 86_400e9,
            "w" => 604_800e9,
            _ => return None,
        };
        total_nanos += value * nanos_per_unit;
    }

    if !total_nanos.is_finite() || total_nanos > u64::MAX as f64 {
        return None;
    }
    Some(SignedDuration { negative, duration: Duration::from_nanos(total_nanos as u64) })
}

/// Parses `HH:MM:SS`, `MM:SS`, or `SS` into a duration.
pub fn parse_time_string(input: &str) -> Option<Duration> {
    let parts: Vec<&str> = input.trim().split(':').collect();
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    let mut secs: u64 = 0;
    for (i, part) in parts.iter().enumerate() {
        let v: u64 = part.parse().ok()?;
        // Minutes/seconds after the first component must be < 60.
        if i > 0 && v >= 60 {
            return None;
        }
        secs = secs.checked_mul(60)?.checked_add(v)?;
    }
    Some(Duration::from_secs(secs))
}

/// Duration string or time string; must be non-negative.
/// Used by `--capture-duration` and `--start-delay`.
pub fn parse_duration_or_time(flag: &str, input: &str) -> Result<Duration> {
    if let Some(d) = parse_duration_units(input) {
        if d.negative && !d.duration.is_zero() {
            return Err(Error::InvalidArg(format!("{flag} cannot be negative: {input}")));
        }
        return Ok(d.duration);
    }
    parse_time_string(input).ok_or_else(|| {
        Error::InvalidArg(format!(
            "{flag}: unable to parse '{input}' as either a duration (e.g. 1h30m) or a time string (e.g. 01:30:00)"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: &str) -> Option<(bool, u64)> {
        parse_duration_units(s).map(|d| (d.negative, d.duration.as_secs()))
    }

    #[test]
    fn units() {
        assert_eq!(secs("1d8h10m"), Some((false, 86_400 + 8 * 3600 + 600)));
        assert_eq!(secs("-30m"), Some((true, 1800)));
        assert_eq!(secs("+1.5h"), Some((false, 5400)));
        assert_eq!(secs("2w"), Some((false, 1_209_600)));
        assert_eq!(secs("0"), Some((false, 0)));
        assert_eq!(parse_duration_units("1s500ms").unwrap().duration, Duration::from_millis(1500));
    }

    #[test]
    fn units_reject() {
        for bad in ["", "-", "10", "h", "1x", "1h-2m", "12:30"] {
            assert_eq!(secs(bad), None, "{bad}");
        }
    }

    #[test]
    fn time_strings() {
        assert_eq!(parse_time_string("12:30:05"), Some(Duration::from_secs(45_005)));
        assert_eq!(parse_time_string("30:05"), Some(Duration::from_secs(1805)));
        assert_eq!(parse_time_string("90"), Some(Duration::from_secs(90)));
        assert_eq!(parse_time_string("100:00:00"), Some(Duration::from_secs(360_000)));
        assert_eq!(parse_time_string("1:60:00"), None);
        assert_eq!(parse_time_string("1::00"), None);
        assert_eq!(parse_time_string("1:2:3:4"), None);
    }

    #[test]
    fn combined() {
        assert_eq!(parse_duration_or_time("x", "1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration_or_time("x", "01:00:00").unwrap(), Duration::from_secs(3600));
        assert!(parse_duration_or_time("x", "-1h").is_err());
        assert!(parse_duration_or_time("x", "abc").is_err());
    }
}
