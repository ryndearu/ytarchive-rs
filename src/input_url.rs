//! Classifies the URL given on the command line.

use crate::error::{Error, Result};
use crate::types::{AUDIO_ITAG, DataType, FragmentUrl};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputUrl {
    /// A specific video: watch, youtu.be, /live/ID, /shorts/ID.
    Video { id: String },
    /// A channel; `live_url` is the channel's `/live` page, which resolves to
    /// whatever the channel is currently streaming or has scheduled.
    Channel { live_url: String },
    /// A direct googlevideo fragment URL.
    GoogleVideo { id: String, itag: u32, url: FragmentUrl },
}

impl InputUrl {
    /// URL to hand to yt-dlp / fetch the watch page with.
    pub fn page_url(&self) -> String {
        match self {
            InputUrl::Video { id } => format!("https://www.youtube.com/watch?v={id}"),
            InputUrl::Channel { live_url } => live_url.clone(),
            InputUrl::GoogleVideo { id, .. } => format!("https://www.youtube.com/watch?v={id}"),
        }
    }
}

const CHANNEL_PREFIXES: &[&str] = &["/channel/", "/c/", "/user/", "/@"];

pub fn parse_input_url(raw: &str) -> Result<InputUrl> {
    let raw = raw.trim();
    let parsed = url::Url::parse(raw).map_err(|e| Error::InvalidArg(format!("invalid URL '{raw}': {e}")))?;
    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    let host = host.strip_prefix("www.").or_else(|| host.strip_prefix("m.")).unwrap_or(&host);
    let path = parsed.path();
    let lower_path = path.to_ascii_lowercase();

    if host == "youtube.com" {
        if lower_path.starts_with("/watch") {
            let id = query_param(&parsed, "v")
                .filter(|v| !v.is_empty())
                .ok_or_else(|| Error::InvalidArg("youtube URL missing video ID".into()))?;
            return Ok(InputUrl::Video { id });
        }
        if CHANNEL_PREFIXES.iter().any(|p| lower_path.starts_with(p)) {
            // Keep only the channel part (/@name, /channel/ID, ...) and append /live,
            // dropping sub pages like /streams or /videos.
            let mut segs = path.trim_matches('/').split('/');
            let first = segs.next().unwrap_or("");
            let channel_path = if first.starts_with('@') {
                format!("/{first}")
            } else {
                format!("/{first}/{}", segs.next().unwrap_or(""))
            };
            let origin = format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap_or(""));
            return Ok(InputUrl::Channel { live_url: format!("{origin}{channel_path}/live") });
        }
        for prefix in ["/live/", "/shorts/"] {
            if lower_path.starts_with(prefix) {
                let id = path[prefix.len()..].trim_matches('/').to_string();
                if id.is_empty() {
                    break;
                }
                return Ok(InputUrl::Video { id });
            }
        }
    } else if host == "youtu.be" {
        let id = path.trim_matches('/').to_string();
        if !id.is_empty() {
            return Ok(InputUrl::Video { id });
        }
    } else if host.ends_with(".googlevideo.com") {
        let itag = parse_itag(&parsed)?;
        let data_type = if itag == AUDIO_ITAG { DataType::Audio } else { DataType::Video };
        let (url, itag) = parse_gvideo_url(raw, data_type)?;
        let id = query_param(&parsed, "id").unwrap_or_default();
        // The id parameter looks like `VIDEOID.N`.
        let id = id.rsplit_once('.').map_or(id.as_str(), |(v, _)| v).to_string();
        return Ok(InputUrl::GoogleVideo { id, itag, url });
    }

    Err(Error::InvalidArg("the provided URL is not a known valid YouTube URL".into()))
}

/// Validates a googlevideo fragment URL for `data_type` and returns it as a
/// fragment template plus its itag.
pub fn parse_gvideo_url(raw: &str, data_type: DataType) -> Result<(FragmentUrl, u32)> {
    let parsed =
        url::Url::parse(raw.trim()).map_err(|e| Error::InvalidArg(format!("error parsing Google Video URL: {e}")))?;
    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    if !host.ends_with(".googlevideo.com") {
        return Err(Error::InvalidArg("not a Google Video URL".into()));
    }
    let itag = parse_itag(&parsed)?;
    if query_param(&parsed, "noclen").is_none() {
        return Err(Error::InvalidArg("given Google Video URL is not for a fragmented stream".into()));
    }
    match data_type {
        DataType::Audio if itag != AUDIO_ITAG => Err(Error::InvalidArg(
            "given audio URL does not have the audio itag; make sure you set the correct URL(s)".into(),
        )),
        DataType::Video if itag == AUDIO_ITAG => Err(Error::InvalidArg(
            "given video URL has the audio itag set; make sure you set the correct URL(s)".into(),
        )),
        _ => Ok((FragmentUrl::with_sq_param(raw.trim()), itag)),
    }
}

fn parse_itag(u: &url::Url) -> Result<u32> {
    query_param(u, "itag")
        .and_then(|s| s.parse().ok())
        .filter(|i| *i > 0)
        .ok_or_else(|| Error::InvalidArg("error parsing itag parameter of Google Video URL".into()))
}

fn query_param(u: &url::Url, key: &str) -> Option<String> {
    u.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(id: &str) -> InputUrl {
        InputUrl::Video { id: id.into() }
    }

    fn channel(u: &str) -> InputUrl {
        InputUrl::Channel { live_url: u.into() }
    }

    #[test]
    fn video_urls() {
        assert_eq!(parse_input_url("https://www.youtube.com/watch?v=abc&t=1").unwrap(), video("abc"));
        assert_eq!(parse_input_url("https://m.youtube.com/watch?v=abc").unwrap(), video("abc"));
        assert_eq!(parse_input_url("https://youtu.be/abc?si=x").unwrap(), video("abc"));
        assert_eq!(parse_input_url("https://youtube.com/live/abc?feature=share").unwrap(), video("abc"));
        assert_eq!(parse_input_url("https://youtube.com/shorts/abc").unwrap(), video("abc"));
        assert!(parse_input_url("https://youtube.com/watch?x=1").is_err());
        assert!(parse_input_url("https://example.com/watch?v=abc").is_err());
        assert!(parse_input_url("not a url").is_err());
    }

    #[test]
    fn channel_urls() {
        assert_eq!(
            parse_input_url("https://www.youtube.com/@LofiGirl").unwrap(),
            channel("https://www.youtube.com/@LofiGirl/live")
        );
        assert_eq!(
            parse_input_url("https://www.youtube.com/@LofiGirl/streams").unwrap(),
            channel("https://www.youtube.com/@LofiGirl/live")
        );
        assert_eq!(
            parse_input_url("https://www.youtube.com/channel/UC123/videos?x=1").unwrap(),
            channel("https://www.youtube.com/channel/UC123/live")
        );
        assert_eq!(parse_input_url("https://youtube.com/c/Name/").unwrap(), channel("https://youtube.com/c/Name/live"));
    }

    #[test]
    fn googlevideo_urls() {
        let raw = "https://rr1---sn-x.googlevideo.com/videoplayback?id=abc.1&itag=299&noclen=1&sq=40&x=%2F";
        match parse_input_url(raw).unwrap() {
            InputUrl::GoogleVideo { id, itag, url } => {
                assert_eq!(id, "abc");
                assert_eq!(itag, 299);
                assert_eq!(
                    url.for_seq(1),
                    "https://rr1---sn-x.googlevideo.com/videoplayback?id=abc.1&itag=299&noclen=1&x=%2F&sq=1"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn gvideo_validation() {
        let audio = "https://r.googlevideo.com/videoplayback?itag=140&noclen=1";
        let video = "https://r.googlevideo.com/videoplayback?itag=299&noclen=1";
        assert!(parse_gvideo_url(audio, DataType::Audio).is_ok());
        assert!(parse_gvideo_url(audio, DataType::Video).is_err());
        assert!(parse_gvideo_url(video, DataType::Audio).is_err());
        assert!(parse_gvideo_url("https://r.googlevideo.com/videoplayback?itag=299&clen=5", DataType::Video).is_err());
        assert!(parse_gvideo_url("https://example.com/?itag=299&noclen=1", DataType::Video).is_err());
    }
}
