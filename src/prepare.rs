//! Getting from an input URL to selected formats and stream facts, in one of
//! three ways: extraction (the usual case), direct googlevideo URLs from the
//! command line, or a kept download.
//!
//! A kept download is run again after its stream can no longer be looked up
//! (it went private, or its live download is gone). With --recover-private,
//! when a download ends with fragments missing, its database is kept with the
//! stream info and the download URLs (see `Finish::keep_for_retry`); running
//! the same command again finds it through the state files and retries only
//! the missing fragments, while the URLs still work.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::download::{KeptStream, load_state, read_kept};
use crate::error::Error;
use crate::http::Http;
use crate::input_url::{InputUrl, parse_gvideo_url};
use crate::output::state_file;
use crate::prompt::get_user_input;
use crate::types::{
    AUDIO_ITAG, DEFAULT_TARGET_DURATION_SECS, DataType, FormatInfo, FormatSelection, FragmentUrl, MetaInfo, Options,
    new_format_info,
};
use crate::video_info::Extractor;
use crate::{log_error, log_general};

/// Everything known about the stream once formats are selected.
pub struct Prepared {
    /// `None` for direct googlevideo URLs.
    pub extractor: Option<Extractor>,
    pub sel: FormatSelection,
    pub video_id: String,
    pub live: bool,
    pub last_sq: Option<u64>,
    pub target_duration: u32,
    pub format_info: FormatInfo,
    pub metadata: MetaInfo,
    pub thumbnail_url: String,
    /// Rebuilt from a kept download because the stream can no longer be
    /// looked up; missing fragments go straight to recovery.
    pub unavailable: bool,
    /// Another stream's kept database remains in the temp dir (excluded by
    /// `--no-audio`/`--no-video`), so cleanup must not delete that folder.
    pub kept_other_stream: bool,
}

pub enum PrepareOutcome {
    Ready(Box<Prepared>),
    /// Resolved to a stream already recorded by `--monitor-channel`.
    Skipped(String),
    /// Nothing downloadable was found (already logged).
    Failed,
}

pub async fn prepare(
    opts: &Arc<Options>,
    http: &Http,
    cancel: &CancellationToken,
    input: &InputUrl,
    skip_ids: &HashSet<String>,
) -> PrepareOutcome {
    if let InputUrl::GoogleVideo { id, itag, url } = input {
        let Some(sel) = gvideo_selection(opts, *itag, url.clone()).await else {
            return PrepareOutcome::Failed;
        };
        let mut format_info = new_format_info();
        format_info.insert("id".into(), id.clone());
        return PrepareOutcome::Ready(Box::new(Prepared {
            extractor: None,
            sel,
            video_id: id.clone(),
            live: false,
            last_sq: None,
            target_duration: DEFAULT_TARGET_DURATION_SECS,
            format_info,
            metadata: MetaInfo::new(),
            thumbnail_url: String::new(),
            unavailable: false,
            kept_other_stream: false,
        }));
    }

    let mut extractor = Extractor::new(opts.clone(), http.clone(), cancel.clone(), input, skip_ids);
    let Some(sel) = extractor.get_video_info().await else {
        return if extractor.skipped_already_recorded {
            PrepareOutcome::Skipped(extractor.video_id)
        } else {
            PrepareOutcome::Failed
        };
    };
    PrepareOutcome::Ready(Box::new(Prepared {
        sel,
        video_id: extractor.video_id.clone(),
        live: extractor.live,
        last_sq: extractor.last_sq,
        target_duration: extractor.target_duration,
        format_info: extractor.format_info.clone(),
        metadata: extractor.metadata.clone(),
        thumbnail_url: extractor.thumbnail.clone(),
        unavailable: false,
        kept_other_stream: false,
        extractor: Some(extractor),
    }))
}

async fn prompt_url(data_type: DataType) -> Option<(FragmentUrl, u32)> {
    loop {
        let raw = get_user_input(&format!("Please enter the {} url, or nothing to skip: ", data_type.as_str())).await;
        if raw.is_empty() {
            return None;
        }
        match parse_gvideo_url(&raw, data_type) {
            Ok(video) => return Some(video),
            Err(error) => log_general!("{error}"),
        }
    }
}

fn from_flag(raw: Option<&str>, data_type: DataType) -> Result<Option<(FragmentUrl, u32)>, Error> {
    raw.map(|value| {
        parse_gvideo_url(value, data_type).inspect_err(|error| log_error!("--{}-url: {error}", data_type.as_str()))
    })
    .transpose()
}

/// Direct googlevideo URLs, no extraction: builds the selection from the
/// input URL plus `--audio-url`/`--video-url`, prompting for the other
/// stream if only one was given.
async fn gvideo_selection(opts: &Options, input_itag: u32, input_url: FragmentUrl) -> Option<FormatSelection> {
    let mut audio = from_flag(opts.audio_url.as_deref(), DataType::Audio).ok()?;
    let mut video = from_flag(opts.video_url.as_deref(), DataType::Video).ok()?;
    if input_itag == AUDIO_ITAG {
        audio.get_or_insert((input_url, input_itag));
    } else {
        video.get_or_insert((input_url, input_itag));
    }

    if audio.is_none() && !opts.video_only {
        audio = prompt_url(DataType::Audio).await;
    }
    if video.is_none() && !opts.audio_only {
        video = prompt_url(DataType::Video).await;
    }
    if opts.video_only {
        audio = None;
    }
    if opts.audio_only {
        video = None;
    }
    if audio.is_none() && video.is_none() {
        log_error!("Neither an audio nor a video URL was given.");
        return None;
    }
    Some(FormatSelection {
        label: String::new(),
        codec: None,
        video_itag: video.as_ref().map(|pair| pair.1),
        audio_url: audio.map(|pair| pair.0),
        video_url: video.map(|pair| pair.0),
    })
}

/// Itags with a `{video_id}.f{itag}.state` file in `dir`.
fn state_itags(dir: &Path, video_id: &str) -> Vec<u32> {
    let prefix = format!("{video_id}.f");
    let Ok(entries) = fs::read_dir(if dir.as_os_str().is_empty() { Path::new(".") } else { dir }) else {
        return Vec::new();
    };
    let mut itags: Vec<u32> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_prefix(&prefix)?.strip_suffix(".state")?.parse().ok()
        })
        .collect();
    itags.sort_unstable();
    itags
}

/// Rebuilds what `download` needs from a download of `video_id` that was
/// kept for retrying. `None` if there is none, or without --recover-private.
pub fn prepared_from_kept(opts: &Options, video_id: &str) -> Option<Prepared> {
    if !opts.recover_private || opts.disable_save_state {
        return None;
    }
    let dir = opts.temp_dir.clone().unwrap_or_default();
    let mut kept: Vec<(u32, KeptStream)> = Vec::new();
    let mut temp_dir = PathBuf::new();
    for itag in state_itags(&dir, video_id) {
        let Some(state) = load_state(&state_file(opts, video_id, itag)) else {
            continue;
        };
        if state.fragments == 0 || !state.temp_dir.is_dir() {
            continue;
        }
        if let Some(kept_stream) = read_kept(&state.temp_dir, itag).filter(|found| found.info.video_id == video_id) {
            temp_dir = state.temp_dir;
            kept.push((itag, kept_stream));
        }
    }
    let info = kept.first()?.1.info.clone();
    // The newest URL of each stream; recovery tries the others too.
    let url = |itag: u32| {
        kept.iter()
            .find(|(kept_itag, _)| *kept_itag == itag)
            .and_then(|(_, kept_stream)| kept_stream.urls.last().cloned())
    };
    let audio_url = (!opts.video_only).then(|| url(AUDIO_ITAG)).flatten();
    let video_url = (!opts.audio_only).then(|| info.video_itag.and_then(url)).flatten();
    if audio_url.is_none() && video_url.is_none() {
        return None;
    }
    let video_itag = video_url.as_ref().and(info.video_itag);
    let kept_other_stream =
        kept.iter().any(|(itag, _)| !((*itag == AUDIO_ITAG && audio_url.is_some()) || Some(*itag) == video_itag));
    log_general!(
        "The stream can no longer be looked up; retrying the missing fragments of the download kept in {}",
        temp_dir.display()
    );
    Some(Prepared {
        extractor: None,
        sel: FormatSelection {
            label: if video_url.is_none() { "audio_only".into() } else { info.label },
            codec: None,
            video_itag,
            audio_url,
            video_url,
        },
        video_id: video_id.to_string(),
        live: false,
        last_sq: kept.iter().filter_map(|(_, kept_stream)| kept_stream.head).max(),
        target_duration: info.target_duration,
        format_info: info.format_info,
        metadata: info.metadata,
        thumbnail_url: info.thumbnail_url,
        unavailable: true,
        kept_other_stream,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::DbSink;
    use crate::types::{DownloadState, SavedDownload};

    #[test]
    fn finds_state_files_of_the_video() {
        let dir = std::env::temp_dir().join(format!("ytarchive-rs-kept-{}", std::process::id()));
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        fs::create_dir_all(&dir).unwrap();
        for name in ["abc.f299.state", "abc.f140.state", "abcd.f1.state", "abc.f140.state.bak", "abc.fx.state"] {
            fs::write(dir.join(name), b"{}").unwrap();
        }
        assert_eq!(state_itags(&dir, "abc"), [140, 299]);
        assert!(state_itags(&dir, "zzz").is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prepared_from_kept_honors_stream_filters() {
        let dir = std::env::temp_dir().join(format!("ytarchive-rs-kept-filters-{}", std::process::id()));
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        let stream_dir = dir.join("stream");
        fs::create_dir_all(&stream_dir).unwrap();

        let saved = SavedDownload {
            video_id: "vid1".into(),
            label: "1080p60".into(),
            video_itag: Some(299),
            target_duration: 5,
            ..Default::default()
        };
        let download_json = serde_json::to_string(&saved).unwrap();
        let state_bytes = serde_json::to_vec(&DownloadState {
            start_frag: 0,
            fragments: 3,
            size: 100,
            temp_dir: stream_dir.clone(),
            ..Default::default()
        })
        .unwrap();

        for itag in [140u32, 299] {
            let db = DbSink::open(&stream_dir.join(format!("out.f{itag}.sqlite")), 0o644).unwrap();
            db.set_meta("download", &download_json).unwrap();
            db.set_meta("head", "10").unwrap();
            let raw_url = format!("https://r.googlevideo.com/videoplayback?itag={itag}&noclen=1");
            db.save_urls(&[FragmentUrl::with_sq_param(&raw_url)]).unwrap();
            fs::write(dir.join(format!("vid1.f{itag}.state")), &state_bytes).unwrap();
        }

        let cases = [
            ("both", false, false, true, true, Some(299), "1080p60", false),
            ("video_only", true, false, false, true, Some(299), "1080p60", true),
            ("audio_only", false, true, true, false, None, "audio_only", true),
        ];
        for (name, video_only, audio_only, want_audio, want_video, want_itag, want_label, want_other) in cases {
            let options = Options {
                recover_private: true,
                temp_dir: Some(dir.clone()),
                video_only,
                audio_only,
                ..Default::default()
            };
            let prepared = prepared_from_kept(&options, "vid1").unwrap_or_else(|| panic!("{name}: expected Some"));
            assert_eq!(prepared.sel.audio_url.is_some(), want_audio, "{name}");
            assert_eq!(prepared.sel.video_url.is_some(), want_video, "{name}");
            assert_eq!(prepared.sel.video_itag, want_itag, "{name}");
            assert_eq!(prepared.sel.label, want_label, "{name}");
            assert_eq!(prepared.kept_other_stream, want_other, "{name}");
        }

        fs::remove_file(dir.join("vid1.f299.state")).unwrap();
        let audio_only =
            Options { recover_private: true, temp_dir: Some(dir.clone()), audio_only: true, ..Default::default() };
        let prepared = prepared_from_kept(&audio_only, "vid1").unwrap();
        assert!(!prepared.kept_other_stream);
        let video_only =
            Options { recover_private: true, temp_dir: Some(dir.clone()), video_only: true, ..Default::default() };
        assert!(prepared_from_kept(&video_only, "vid1").is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn gvideo_selection_rejects_invalid_flag_without_prompting() {
        let options = Options { audio_url: Some("https://example.com/bad".into()), ..Default::default() };
        let input_url = FragmentUrl::with_sq_param("https://r.googlevideo.com/videoplayback?itag=299&noclen=1");
        assert!(gvideo_selection(&options, 299, input_url).await.is_none());
    }
}
