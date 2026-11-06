//! Filename-template values (`FormatInfo`) and file metadata (`MetaInfo`).

use chrono::{DateTime, Local, Utc};

use crate::types::{new_format_info, FormatInfo, MetaInfo, PlayerResponse, YtdlpExtraction};
use crate::util::format_python_map;

/// Default metadata; values are output templates.
pub fn default_metadata(overrides: &[(String, String)]) -> MetaInfo {
    let mut m: MetaInfo = [
        ("title", "%(title)s"),
        ("artist", "%(channel)s"),
        ("date", "%(upload_date)s"),
        ("comment", "%(url)s\n\n%(description)s"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    for (k, v) in overrides {
        m.insert(k.clone(), v.clone());
    }
    m
}

/// Expands each metadata template; values that fail to expand stay as-is.
pub fn apply_metadata(meta: &MetaInfo, fi: &FormatInfo) -> MetaInfo {
    meta.iter()
        .map(|(k, v)| (k.clone(), format_python_map(v, fi).unwrap_or_else(|_| v.clone())))
        .collect()
}

fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

fn set_date_parts(fi: &mut FormatInfo, date8: &str) {
    if date8.len() >= 8 {
        fi.insert("year".into(), date8[..4].into());
        fi.insert("month".into(), date8[4..6].into());
        fi.insert("day".into(), date8[6..8].into());
    }
}

fn set_time_parts(fi: &mut FormatInfo, time6: &str) {
    if time6.len() >= 6 {
        fi.insert("start_time".into(), time6[..6].into());
        fi.insert("hours".into(), time6[..2].into());
        fi.insert("minutes".into(), time6[2..4].into());
        fi.insert("seconds".into(), time6[4..6].into());
    }
}

/// From the watch page player response. Dates come from the broadcast start
/// timestamp as written by YouTube (no timezone conversion, as in Go).
pub fn format_info_from_player_response(pr: &PlayerResponse) -> FormatInfo {
    let mut fi = new_format_info();
    let vd = &pr.video_details;
    let pmfr = &pr.microformat.player_microformat_renderer;
    let start = &pmfr.live_broadcast_details.start_timestamp;

    // "2026-11-07T01:02:03+00:00" -> date 20261107, time 010203
    let start_digits = digits(start);
    let start_date = start_digits.get(..8).unwrap_or("").to_string();
    set_date_parts(&mut fi, &start_date);
    if let Some(t) = start_digits.get(8..14) {
        set_time_parts(&mut fi, t);
    }

    fi.insert("id".into(), vd.video_id.clone());
    fi.insert("url".into(), format!("https://www.youtube.com/watch?v={}", vd.video_id));
    fi.insert("title".into(), vd.title.trim().into());
    fi.insert("channel_id".into(), vd.channel_id.clone());
    fi.insert("channel".into(), vd.author.clone());
    fi.insert("upload_date".into(), start_date.clone());
    fi.insert("start_date".into(), start_date);
    fi.insert(
        "publish_date".into(),
        digits(&pmfr.publish_date).get(..8).unwrap_or("").into(),
    );
    fi.insert("description".into(), vd.short_description.trim().into());
    fi
}

/// From yt-dlp. Start date/time are derived from the unix start time in UTC.
pub fn format_info_from_ytdlp(info: &YtdlpExtraction, page_url: &str) -> FormatInfo {
    let mut fi = new_format_info();
    let start = info
        .start_unix
        .and_then(|t| DateTime::<Utc>::from_timestamp(t, 0));
    let start_date = match start {
        Some(dt) => {
            let d = dt.format("%Y%m%d").to_string();
            set_time_parts(&mut fi, &dt.format("%H%M%S").to_string());
            d
        }
        None => String::new(),
    };
    set_date_parts(&mut fi, &start_date);

    fi.insert("id".into(), info.id.clone());
    fi.insert("url".into(), page_url.into());
    fi.insert("title".into(), info.title.trim().into());
    fi.insert("channel_id".into(), info.channel_id.clone());
    fi.insert("channel".into(), info.channel.clone());
    fi.insert("upload_date".into(), info.upload_date.clone());
    fi.insert("start_date".into(), start_date);
    fi.insert("publish_date".into(), info.upload_date.clone());
    fi.insert("description".into(), info.description.trim().into());
    fi
}

/// RFC 3339 timestamp converted to local time; unparsable input is returned as-is.
pub fn local_timestamp(rfc3339: &str) -> String {
    let s = rfc3339.trim();
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Local).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|_| s.to_string())
}

pub fn local_unix_timestamp(unix: i64) -> String {
    DateTime::<Utc>::from_timestamp(unix, 0)
        .map(|t| t.with_timezone(&Local).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr() -> PlayerResponse {
        serde_json::from_value(serde_json::json!({
            "videoDetails": {"videoId":"vid","title":" T ","channelId":"UC1","author":"Chan","shortDescription":" d "},
            "microformat": {"playerMicroformatRenderer": {
                "liveBroadcastDetails": {"startTimestamp":"2026-11-07T01:02:03+00:00"},
                "publishDate": "2026-11-06T20:00:00-08:00"
            }}
        }))
        .unwrap()
    }

    #[test]
    fn from_player_response() {
        let fi = format_info_from_player_response(&pr());
        assert_eq!(fi["id"], "vid");
        assert_eq!(fi["url"], "https://www.youtube.com/watch?v=vid");
        assert_eq!(fi["title"], "T");
        assert_eq!(fi["channel"], "Chan");
        assert_eq!(fi["upload_date"], "20261107");
        assert_eq!((fi["year"].as_str(), fi["month"].as_str(), fi["day"].as_str()), ("2026", "11", "07"));
        assert_eq!(fi["start_time"], "010203");
        assert_eq!((fi["hours"].as_str(), fi["minutes"].as_str(), fi["seconds"].as_str()), ("01", "02", "03"));
        assert_eq!(fi["publish_date"], "20261106");
        assert_eq!(fi["description"], "d");
    }

    #[test]
    fn missing_start_timestamp_does_not_panic() {
        let fi = format_info_from_player_response(&PlayerResponse::default());
        assert_eq!(fi["upload_date"], "");
        assert_eq!(fi["start_time"], "");
    }

    #[test]
    fn from_ytdlp() {
        let info = YtdlpExtraction {
            id: "vid".into(),
            title: "T".into(),
            upload_date: "20261106".into(),
            start_unix: Some(1_762_477_323), // 2025-11-07T01:02:03Z
            ..Default::default()
        };
        let fi = format_info_from_ytdlp(&info, "https://www.youtube.com/watch?v=vid");
        assert_eq!(fi["start_date"], "20251107");
        assert_eq!(fi["start_time"], "010203");
        assert_eq!(fi["upload_date"], "20261106");
        assert_eq!(fi["publish_date"], "20261106");
    }

    #[test]
    fn metadata() {
        let fi = format_info_from_player_response(&pr());
        let meta = default_metadata(&[("artist".into(), "Someone".into()), ("bad".into(), "%(nope)s".into())]);
        let m = apply_metadata(&meta, &fi);
        assert_eq!(m["title"], "T");
        assert_eq!(m["artist"], "Someone");
        assert_eq!(m["comment"], "https://www.youtube.com/watch?v=vid\n\nd");
        assert_eq!(m["bad"], "%(nope)s");
    }
}
