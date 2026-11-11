//! Filename-template values (`FormatInfo`) and file metadata (`MetaInfo`).

use chrono::{DateTime, Local, Utc};

use crate::types::{FormatInfo, MetaInfo, PlayerResponse, YtdlpExtraction, new_format_info};
use crate::util::format_python_map;

/// Default metadata; values are output templates.
pub fn default_metadata(overrides: &[(String, String)]) -> MetaInfo {
    let mut metadata: MetaInfo = [
        ("title", "%(title)s"),
        ("artist", "%(channel)s"),
        ("date", "%(upload_date)s"),
        ("comment", "%(url)s\n\n%(description)s"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    for (key, value) in overrides {
        metadata.insert(key.clone(), value.clone());
    }
    metadata
}

/// Expands each metadata template; values that fail to expand stay as-is.
pub fn apply_metadata(meta: &MetaInfo, format_info: &FormatInfo) -> MetaInfo {
    meta.iter()
        .map(|(key, value)| (key.clone(), format_python_map(value, format_info).unwrap_or_else(|_| value.clone())))
        .collect()
}

fn digits(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).collect()
}

fn set_date_parts(format_info: &mut FormatInfo, date8: &str) {
    if date8.len() >= 8 {
        format_info.insert("year".into(), date8[..4].into());
        format_info.insert("month".into(), date8[4..6].into());
        format_info.insert("day".into(), date8[6..8].into());
    }
}

fn set_time_parts(format_info: &mut FormatInfo, time6: &str) {
    if time6.len() >= 6 {
        format_info.insert("start_time".into(), time6[..6].into());
        format_info.insert("hours".into(), time6[..2].into());
        format_info.insert("minutes".into(), time6[2..4].into());
        format_info.insert("seconds".into(), time6[4..6].into());
    }
}

/// From the watch page player response. Dates come from the broadcast start
/// timestamp as written by YouTube (no timezone conversion, as in Go).
pub fn format_info_from_player_response(sample_response: &PlayerResponse) -> FormatInfo {
    let mut format_info = new_format_info();
    let details = &sample_response.video_details;
    let pmfr = &sample_response.microformat.player_microformat_renderer;
    let start = &pmfr.live_broadcast_details.start_timestamp;

    // "2026-11-07T01:02:03+00:00" -> date 20261107, time 010203
    let start_digits = digits(start);
    let start_date = start_digits.get(..8).unwrap_or("").to_string();
    set_date_parts(&mut format_info, &start_date);
    if let Some(time) = start_digits.get(8..14) {
        set_time_parts(&mut format_info, time);
    }

    format_info.insert("id".into(), details.video_id.clone());
    format_info.insert("url".into(), format!("https://www.youtube.com/watch?v={}", details.video_id));
    format_info.insert("title".into(), details.title.trim().into());
    format_info.insert("channel_id".into(), details.channel_id.clone());
    format_info.insert("channel".into(), details.author.clone());
    format_info.insert("upload_date".into(), start_date.clone());
    format_info.insert("start_date".into(), start_date);
    format_info.insert("publish_date".into(), digits(&pmfr.publish_date).get(..8).unwrap_or("").into());
    format_info.insert("description".into(), details.short_description.trim().into());
    format_info
}

/// From yt-dlp. Start date/time are derived from the unix start time in UTC.
pub fn format_info_from_ytdlp(info: &YtdlpExtraction, page_url: &str) -> FormatInfo {
    let mut format_info = new_format_info();
    let start = info.start_unix.and_then(|time| DateTime::<Utc>::from_timestamp(time, 0));
    let start_date = match start {
        Some(date_time) => {
            let date = date_time.format("%Y%m%d").to_string();
            set_time_parts(&mut format_info, &date_time.format("%H%M%S").to_string());
            date
        }
        None => String::new(),
    };
    set_date_parts(&mut format_info, &start_date);

    format_info.insert("id".into(), info.id.clone());
    format_info.insert("url".into(), page_url.into());
    format_info.insert("title".into(), info.title.trim().into());
    format_info.insert("channel_id".into(), info.channel_id.clone());
    format_info.insert("channel".into(), info.channel.clone());
    format_info.insert("upload_date".into(), info.upload_date.clone());
    format_info.insert("start_date".into(), start_date);
    format_info.insert("publish_date".into(), info.upload_date.clone());
    format_info.insert("description".into(), info.description.trim().into());
    format_info
}

/// RFC 3339 timestamp converted to local time; unparsable input is returned as-is.
pub fn local_timestamp(rfc3339: &str) -> String {
    let text = rfc3339.trim();
    DateTime::parse_from_rfc3339(text)
        .map(|time| time.with_timezone(&Local).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|_| text.to_string())
}

pub fn local_unix_timestamp(unix: i64) -> String {
    DateTime::<Utc>::from_timestamp(unix, 0)
        .map(|time| time.with_timezone(&Local).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_response() -> PlayerResponse {
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
        let format_info = format_info_from_player_response(&sample_response());
        assert_eq!(format_info["id"], "vid");
        assert_eq!(format_info["url"], "https://www.youtube.com/watch?v=vid");
        assert_eq!(format_info["title"], "T");
        assert_eq!(format_info["channel"], "Chan");
        assert_eq!(format_info["upload_date"], "20261107");
        assert_eq!(
            (format_info["year"].as_str(), format_info["month"].as_str(), format_info["day"].as_str()),
            ("2026", "11", "07")
        );
        assert_eq!(format_info["start_time"], "010203");
        assert_eq!(
            (format_info["hours"].as_str(), format_info["minutes"].as_str(), format_info["seconds"].as_str()),
            ("01", "02", "03")
        );
        assert_eq!(format_info["publish_date"], "20261106");
        assert_eq!(format_info["description"], "d");
    }

    #[test]
    fn missing_start_timestamp_does_not_panic() {
        let format_info = format_info_from_player_response(&PlayerResponse::default());
        assert_eq!(format_info["upload_date"], "");
        assert_eq!(format_info["start_time"], "");
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
        let format_info = format_info_from_ytdlp(&info, "https://www.youtube.com/watch?v=vid");
        assert_eq!(format_info["start_date"], "20251107");
        assert_eq!(format_info["start_time"], "010203");
        assert_eq!(format_info["upload_date"], "20261106");
        assert_eq!(format_info["publish_date"], "20261106");
    }

    #[test]
    fn metadata() {
        let format_info = format_info_from_player_response(&sample_response());
        let meta = default_metadata(&[("artist".into(), "Someone".into()), ("bad".into(), "%(nope)s".into())]);
        let metadata = apply_metadata(&meta, &format_info);
        assert_eq!(metadata["title"], "T");
        assert_eq!(metadata["artist"], "Someone");
        assert_eq!(metadata["comment"], "https://www.youtube.com/watch?v=vid\n\nd");
        assert_eq!(metadata["bad"], "%(nope)s");
    }
}
