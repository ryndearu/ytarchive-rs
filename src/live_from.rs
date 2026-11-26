//! Sequence-number math for `--live-from`, `--start-delay`, and the 7-day
//! seek window.

use std::time::Duration;

use crate::duration::{parse_duration_units, parse_time_string};
use crate::error::{Error, Result};
use crate::types::LIVE_MAXIMUM_SEEKABLE_SECS;
use crate::util::seconds_to_duration_and_time_str;

/// Where to start downloading, and a message describing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveFrom {
    pub start_sq: u64,
    pub message: String,
}

/// Earliest sequence YouTube still serves for a stream at `last_sq`.
pub fn seekable_start_sq(last_sq: u64, target_duration: u32) -> u64 {
    if target_duration == 0 {
        return 0;
    }
    last_sq.saturating_sub(LIVE_MAXIMUM_SEEKABLE_SECS / u64::from(target_duration))
}

/// Number of fragments covering `secs`, rounded up to whole fragments.
fn frags_for(secs: u64, target_duration: u32) -> u64 {
    secs.div_ceil(u64::from(target_duration.max(1)))
}

fn parse_unsigned(value: &str) -> Option<Duration> {
    parse_duration_units(value)
        .filter(|parsed| !parsed.negative)
        .map(|parsed| parsed.duration)
        .or_else(|| parse_time_string(value))
}

fn parse_live_from(value: &str) -> Result<Option<(bool, Duration)>> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("now") {
        return Ok(None);
    }
    let (negative, body) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed),
    };
    let dur = parse_unsigned(body).ok_or_else(|| {
        Error::InvalidArg(format!("--live-from: unable to parse '{value}' as either a duration or a time string"))
    })?;
    if negative && dur.as_secs() > LIVE_MAXIMUM_SEEKABLE_SECS {
        return Err(Error::InvalidArg(format!(
            "invalid duration specified '{value}' (maximum video seek time is {} days)",
            LIVE_MAXIMUM_SEEKABLE_SECS / 86_400
        )));
    }
    Ok(Some((negative, dur)))
}

/// Validates `--live-from` syntax and the 7-day seek bound for negative offsets.
pub fn validate_live_from(value: &str) -> Result<()> {
    parse_live_from(value).map(|_| ())
}

/// Resolves `--live-from` (`now`, `-DURATION`, or `DURATION`/time string)
/// against the current stream position.
pub fn resolve_live_from(value: &str, last_sq: u64, target_duration: u32) -> Result<LiveFrom> {
    let target = u64::from(target_duration.max(1));
    let Some((negative, dur)) = parse_live_from(value)? else {
        return Ok(LiveFrom { start_sq: last_sq, message: "Starting download from current time".into() });
    };
    let frags = frags_for(dur.as_secs(), target_duration);
    let rounded_secs = frags.saturating_mul(target);
    let live_for = || seconds_to_duration_and_time_str(last_sq.saturating_mul(target));

    if negative {
        if frags > last_sq {
            return Err(Error::InvalidArg(format!(
                "invalid duration specified; the stream has not been live for that long [live for {}]",
                live_for()
            )));
        }
        return Ok(LiveFrom {
            start_sq: last_sq - frags,
            message: format!("Jumping back {rounded_secs} seconds from now, and starting to download from that time."),
        });
    }

    if last_sq < frags {
        return Err(Error::InvalidArg(format!(
            "invalid duration specified; the stream has not been live for that long [live for {}]",
            live_for()
        )));
    }
    // Go compared a fragment count against a number of seconds here; compare
    // sequence numbers instead.
    let earliest = seekable_start_sq(last_sq, target_duration);
    if frags < earliest {
        return Err(Error::InvalidArg(format!(
            "YT only retains the livestream 7 days past for seeking; you must specify a --live-from value between {} and {}",
            seconds_to_duration_and_time_str(earliest.saturating_mul(target)),
            live_for()
        )));
    }
    Ok(LiveFrom {
        start_sq: frags,
        message: format!(
            "Starting from stream time '{}' and grabbing '{}' of content (and counting).",
            seconds_to_duration_and_time_str(frags.saturating_mul(target)),
            seconds_to_duration_and_time_str((last_sq - frags).saturating_mul(target))
        ),
    })
}

/// `--start-delay`: the sequence to start at and how long to wait first,
/// both rounded up to whole fragments.
pub fn start_delay(delay: Duration, last_sq: u64, target_duration: u32) -> (u64, Duration) {
    let frags = frags_for(delay.as_secs(), target_duration);
    let wait = frags.saturating_mul(u64::from(target_duration.max(1)));
    (last_sq.saturating_add(frags), Duration::from_secs(wait))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seekable() {
        assert_eq!(seekable_start_sq(100, 5), 0);
        let week_frags = LIVE_MAXIMUM_SEEKABLE_SECS / 5;
        assert_eq!(seekable_start_sq(week_frags + 50, 5), 50);
        assert_eq!(seekable_start_sq(100, 0), 0);
    }

    #[test]
    fn resolves_start() {
        // Older than the 7-day window.
        let past_window = LIVE_MAXIMUM_SEEKABLE_SECS / 5 + 1000;
        let cases = [
            ("NOW", 500, Some(500)),
            // 61s rounds up to 13 fragments of 5s.
            ("-61s", 500, Some(487)),
            ("-00:10:00", 500, Some(380)),
            ("-1h", 100, None),
            ("-8d", 10_000_000, None),
            ("10m", 500, Some(120)),
            ("00:00:05", 500, Some(1)),
            ("1h", 100, None),
            ("1m", past_window, None),
            ("garbage", 500, None),
        ];
        for (input, last, expected) in cases {
            let start = resolve_live_from(input, last, 5).ok().map(|live_from| live_from.start_sq);
            assert_eq!(start, expected, "{input} at {last}");
        }
        assert!(resolve_live_from("-61s", 500, 5).unwrap().message.contains("65 seconds"));
    }

    #[test]
    fn delay() {
        assert_eq!(start_delay(Duration::from_secs(12), 100, 5), (103, Duration::from_secs(15)));
        assert_eq!(start_delay(Duration::ZERO, 100, 5), (100, Duration::ZERO));
    }
}
