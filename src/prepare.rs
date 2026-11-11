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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::download::{KeptStream, load_state, read_kept};
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
}

/// `None` if nothing downloadable was found (already logged).
pub async fn prepare(
    opts: &Arc<Options>,
    http: &Http,
    cancel: &CancellationToken,
    input: &InputUrl,
) -> Option<Prepared> {
    if let InputUrl::GoogleVideo { id, itag, url } = input {
        let sel = gvideo_selection(opts, *itag, url.clone()).await?;
        let mut format_info = new_format_info();
        format_info.insert("id".into(), id.clone());
        return Some(Prepared {
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
        });
    }

    let mut extractor = Extractor::new(opts.clone(), http.clone(), cancel.clone(), input);
    let sel = extractor.get_video_info().await?;
    Some(Prepared {
        sel,
        video_id: extractor.video_id.clone(),
        live: extractor.live,
        last_sq: extractor.last_sq,
        target_duration: extractor.target_duration,
        format_info: extractor.format_info.clone(),
        metadata: extractor.metadata.clone(),
        thumbnail_url: extractor.thumbnail.clone(),
        unavailable: false,
        extractor: Some(extractor),
    })
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

fn from_flag(raw: Option<&str>, data_type: DataType) -> Option<(FragmentUrl, u32)> {
    let raw = raw?;
    parse_gvideo_url(raw, data_type).inspect_err(|error| log_error!("--{}-url: {error}", data_type.as_str())).ok()
}

/// Direct googlevideo URLs, no extraction: builds the selection from the
/// input URL plus `--audio-url`/`--video-url`, prompting for the other
/// stream if only one was given.
async fn gvideo_selection(opts: &Options, input_itag: u32, input_url: FragmentUrl) -> Option<FormatSelection> {
    let mut audio = from_flag(opts.audio_url.as_deref(), DataType::Audio);
    let mut video = from_flag(opts.video_url.as_deref(), DataType::Video);
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
    let Ok(entries) = std::fs::read_dir(if dir.as_os_str().is_empty() { Path::new(".") } else { dir }) else {
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
        if let Some(kept_stream) =
            read_kept(&state.temp_dir, itag, opts.file_mode).filter(|found| found.info.video_id == video_id)
        {
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
    let audio_url = url(AUDIO_ITAG);
    let video_url = info.video_itag.and_then(url);
    if audio_url.is_none() && video_url.is_none() {
        return None;
    }
    log_general!(
        "The stream can no longer be looked up; retrying the missing fragments of the download kept in {}",
        temp_dir.display()
    );
    Some(Prepared {
        extractor: None,
        sel: FormatSelection {
            label: info.label,
            codec: None,
            video_itag: video_url.as_ref().and(info.video_itag),
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_state_files_of_the_video() {
        let dir = std::env::temp_dir().join(format!("ytarchive-rs-kept-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["abc.f299.state", "abc.f140.state", "abcd.f1.state", "abc.f140.state.bak", "abc.fx.state"] {
            std::fs::write(dir.join(name), b"{}").unwrap();
        }
        assert_eq!(state_itags(&dir, "abc"), [140, 299]);
        assert!(state_itags(&dir, "zzz").is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
