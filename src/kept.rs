//! Running a kept download again after its stream can no longer be looked up
//! (it went private, or its live download is gone). With --recover-private,
//! when a download ends with fragments missing, its database is kept with the stream info and the
//! download URLs (see `Finish::keep_for_retry`); running the same command
//! again finds it through the state files and retries only the missing
//! fragments, while the URLs still work.

use std::path::{Path, PathBuf};

use crate::download::{load_state, read_kept, KeptStream};
use crate::log_general;
use crate::output::state_file;
use crate::prepare::Prepared;
use crate::types::{FormatSelection, Options, AUDIO_ITAG};

/// Itags with a `{video_id}.f{itag}.state` file in `dir`.
fn state_itags(dir: &Path, video_id: &str) -> Vec<u32> {
    let prefix = format!("{video_id}.f");
    let Ok(entries) = std::fs::read_dir(if dir.as_os_str().is_empty() { Path::new(".") } else { dir }) else {
        return Vec::new();
    };
    let mut itags: Vec<u32> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
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
        let Some(st) = load_state(&state_file(opts, video_id, itag)) else { continue };
        if st.fragments == 0 || !st.temp_dir.is_dir() {
            continue;
        }
        if let Some(k) = read_kept(&st.temp_dir, itag, opts.file_mode).filter(|k| k.info.video_id == video_id) {
            temp_dir = st.temp_dir;
            kept.push((itag, k));
        }
    }
    let info = kept.first()?.1.info.clone();
    // The newest URL of each stream; recovery tries the others too.
    let url = |itag: u32| kept.iter().find(|(i, _)| *i == itag).and_then(|(_, k)| k.urls.last().cloned());
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
        last_sq: kept.iter().filter_map(|(_, k)| k.head).max(),
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
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["abc.f299.state", "abc.f140.state", "abcd.f1.state", "abc.f140.state.bak", "abc.fx.state"] {
            std::fs::write(dir.join(name), b"{}").unwrap();
        }
        assert_eq!(state_itags(&dir, "abc"), [140, 299]);
        assert!(state_itags(&dir, "zzz").is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
