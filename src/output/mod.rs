//! Output file naming, final/temporary directories, and resume state files.

use std::path::{Path, PathBuf};

use crate::download::load_state;
use crate::error::{Error, Result};
use crate::types::{DownloadState, FormatInfo, Options};
use crate::util::filename::MAX_FILENAME_LEN;
use crate::util::{NameStyle, create_temp_dir, format_filename, portable_file_name, sterilize_filename};
use crate::{log_error, log_warn};

#[derive(Debug, Clone)]
pub struct OutputPlan {
    pub final_dir: PathBuf,
    /// Base file name, without extension.
    pub file_name: String,
    pub temp_dir: PathBuf,
}

impl OutputPlan {
    pub fn temp(&self, suffix: &str) -> PathBuf {
        self.temp_dir.join(format!("{}{suffix}", self.file_name))
    }

    pub fn final_path(&self, suffix: &str) -> PathBuf {
        self.final_dir.join(format!("{}{suffix}", self.file_name))
    }

    /// Whether the temp dir is ours to delete afterwards: it must be a
    /// `{video_id}__XXXX` dir we create, never an arbitrary dir named in a
    /// state file.
    pub fn owns_temp_dir(&self, video_id: &str) -> bool {
        self.temp_dir != self.final_dir
            && self
                .temp_dir
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(&format!("{video_id}__")))
    }

    pub fn remove_temp_dir(&self, video_id: &str) {
        if self.owns_temp_dir(video_id)
            && let Err(error) = std::fs::remove_dir_all(&self.temp_dir)
        {
            log_warn!("Error removing temporary directory {}: {error}", self.temp_dir.display());
        }
    }
}

/// Windows' MAX_PATH: 260 UTF-16 units, including the terminating NUL.
const WINDOWS_MAX_PATH: usize = 260;
/// Room for the longest suffix added to the base name, a fragment file
/// `.f{itag}.frag{seq}.ts`.
const SUFFIX_ROOM: usize = 24;
/// Random part of a `{id}__XXXX` temp dir name (see `create_temp_dir`).
const TEMP_DIR_RANDOM: usize = 8;

/// Final directory and base file name from the output template. With
/// `--windows-filenames` (unless `--no-path-limit`), the title is shortened
/// so every file path (final and temporary, with any suffix) fits in MAX_PATH.
pub fn final_location(opts: &Options, format_info: &FormatInfo) -> Result<(PathBuf, String)> {
    let style = NameStyle { lookalike: opts.lookalike_chars, windows: opts.windows_filenames };
    let mut full = format_filename(&opts.output_format, format_info, style, None)?;
    let mut dir = output_dir(opts, &full, style);
    let mut room = None;
    if style.windows && !opts.no_path_limit {
        let video_id = format_info.get("id").map_or("", String::as_str);
        let units = windows_name_room(opts, &dir, video_id);
        full = format_filename(&opts.output_format, format_info, style, Some(units))?;
        dir = output_dir(opts, &full, style);
        room = Some(units);
    }
    let full_path = Path::new(&full);

    let mut name = sterilize_filename(
        full_path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default().as_str(),
        style,
    );
    if name.starts_with('-') {
        name.insert(0, '_');
    }
    if name.trim().is_empty() || name == "." {
        log_error!("Expanded output file path: {full}");
        return Err(Error::InvalidFormat("output file name appears to be empty after formatting".into()));
    }
    let name = if style.windows { portable_file_name(&name) } else { name };
    if name.len() > MAX_FILENAME_LEN {
        return Err(Error::InvalidFormat(format!("output file name is too long ({} bytes)", name.len())));
    }
    if room.is_some_and(|limit| name.encode_utf16().count() > limit) {
        log_warn!(
            "The output paths are longer than the Windows limit of {WINDOWS_MAX_PATH} characters even after shortening the title; writing files may fail."
        );
    }
    Ok((dir, name))
}

/// Absolute directory part of a formatted output path.
fn output_dir(opts: &Options, full: &str, style: NameStyle) -> PathBuf {
    let mut dir = Path::new(full).parent().map(Path::to_path_buf).unwrap_or_default();
    // Relative templates must stay relative (a value can't make them absolute).
    if !Path::new(&opts.output_format).is_absolute() {
        dir = dir.components().filter(|component| !matches!(component, std::path::Component::RootDir)).collect();
    }
    // With --windows-filenames, folder names from the template (e.g.
    // %(channel)s) must be valid on Windows too; drive prefixes, roots, `.`
    // and `..` are kept as-is.
    if style.windows {
        dir = dir
            .components()
            .map(|component| match component {
                std::path::Component::Normal(name) => PathBuf::from(portable_file_name(&name.to_string_lossy())),
                other => PathBuf::from(other.as_os_str()),
            })
            .collect();
    }
    if dir.as_os_str().is_empty() {
        dir = PathBuf::from(".");
    }
    std::path::absolute(&dir).unwrap_or(dir)
}

/// UTF-16 units left for the base name so that `{dir}\{name}{suffix}` fits
/// in MAX_PATH for both the final dir and the `{id}__XXXX` temp dir.
fn windows_name_room(opts: &Options, final_dir: &Path, video_id: &str) -> usize {
    let units = |path: &Path| path.as_os_str().to_string_lossy().encode_utf16().count();
    let temp_parent = match &opts.temp_dir {
        Some(path) => std::path::absolute(path).unwrap_or_else(|_| path.clone()),
        None => final_dir.to_path_buf(),
    };
    let temp_dir = units(&temp_parent) + 1 + video_id.len() + 2 + TEMP_DIR_RANDOM;
    let longest_dir = units(final_dir).max(temp_dir);
    // dir + separator + name + suffix + NUL
    (WINDOWS_MAX_PATH - 1).saturating_sub(longest_dir + 1 + SUFFIX_ROOM)
}

/// Creates `dir`, falling back to the current directory on failure.
pub fn ensure_dir(dir: PathBuf, mode: u32, what: &str) -> PathBuf {
    if dir == Path::new(".") || dir.is_dir() {
        return dir;
    }
    match std::fs::create_dir_all(&dir) {
        Ok(()) => {
            crate::util::apply_mode(&dir, mode);
            dir
        }
        Err(error) => {
            log_warn!("Error creating {what} directory {}: {error}", dir.display());
            log_warn!("Using the current working directory instead");
            PathBuf::from(".")
        }
    }
}

/// `{temp-dir option or cwd}/{video_id}.f{itag}.state`
pub fn state_file(opts: &Options, video_id: &str, itag: u32) -> PathBuf {
    opts.temp_dir.clone().unwrap_or_default().join(format!("{video_id}.f{itag}.state"))
}

/// Loads saved state for each itag (unless disabled). Returns states and the
/// temp dir a previous run used, if any.
pub fn load_states(opts: &Options, video_id: &str, itags: &[u32]) -> (Vec<DownloadState>, Option<PathBuf>) {
    let mut prev_dir = None;
    let states = itags
        .iter()
        .map(|&itag| {
            if opts.disable_save_state {
                return DownloadState::default();
            }
            let path = state_file(opts, video_id, itag);
            let state = load_state(&path).unwrap_or_default();
            if prev_dir.is_none() && !state.temp_dir.as_os_str().is_empty() && state.temp_dir.is_dir() {
                prev_dir = Some(state.temp_dir.clone());
            }
            DownloadState { file: path, ..state }
        })
        .collect();
    (states, prev_dir)
}

/// Picks the temp dir: the previous run's, else a new `{id}__XXXX` under the
/// temp-dir option (or the final dir). Falls back to the final dir.
pub fn temp_dir(opts: &Options, prev: Option<PathBuf>, final_dir: &Path, video_id: &str) -> PathBuf {
    if let Some(path) = prev {
        return path;
    }
    let base = opts.temp_dir.clone().unwrap_or_else(|| final_dir.to_path_buf());
    match create_temp_dir(&base, &format!("{video_id}__"), opts.dir_mode) {
        Ok(dir) => dir,
        Err(error) => {
            log_warn!("Error creating temp directory: {error}");
            log_warn!("Will download data directly to {} instead", final_dir.display());
            final_dir.to_path_buf()
        }
    }
}

#[cfg(test)]
mod tests;
