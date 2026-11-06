//! Output file naming, final/temporary directories, and resume state files.

use std::path::{Path, PathBuf};

use crate::download::load_state;
use crate::error::{Error, Result};
use crate::types::{DownloadState, FormatInfo, Options};
use crate::util::filename::MAX_FILENAME_LEN;
use crate::util::{
    create_temp_dir, format_filename, portable_file_name, sterilize_filename, NameStyle,
};
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
                .is_some_and(|n| n.to_string_lossy().starts_with(&format!("{video_id}__")))
    }

    pub fn remove_temp_dir(&self, video_id: &str) {
        if self.owns_temp_dir(video_id)
            && let Err(e) = std::fs::remove_dir_all(&self.temp_dir)
        {
            log_warn!("Error removing temporary directory {}: {e}", self.temp_dir.display());
        }
    }
}

/// Final directory and base file name from the output template.
pub fn final_location(opts: &Options, fi: &FormatInfo) -> Result<(PathBuf, String)> {
    let style = NameStyle { lookalike: opts.lookalike_chars, windows: opts.windows_filenames };
    let full = format_filename(&opts.output_format, fi, style)?;
    let full_path = Path::new(&full);
    let mut dir = full_path.parent().map(Path::to_path_buf).unwrap_or_default();
    // Relative templates must stay relative (a value can't make them absolute).
    if !Path::new(&opts.output_format).is_absolute() {
        dir = dir.components().filter(|c| !matches!(c, std::path::Component::RootDir)).collect();
    }
    // With --windows-filenames, folder names from the template (e.g.
    // %(channel)s) must be valid on Windows too; drive prefixes, roots, `.`
    // and `..` are kept as-is.
    if style.windows {
        dir = dir
            .components()
            .map(|c| match c {
                std::path::Component::Normal(s) => PathBuf::from(portable_file_name(&s.to_string_lossy())),
                other => PathBuf::from(other.as_os_str()),
            })
            .collect();
    }
    if dir.as_os_str().is_empty() {
        dir = PathBuf::from(".");
    }
    let dir = std::path::absolute(&dir).unwrap_or(dir);

    let mut name = sterilize_filename(
        full_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().as_str(),
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
    Ok((dir, name))
}

/// Creates `dir`, falling back to the current directory on failure.
pub fn ensure_dir(dir: PathBuf, mode: u32, what: &str) -> PathBuf {
    if dir == Path::new(".") || dir.is_dir() {
        return dir;
    }
    match std::fs::create_dir_all(&dir) {
        Ok(()) => {
            let _ = crate::util::set_mode(&dir, mode);
            dir
        }
        Err(e) => {
            log_warn!("Error creating {what} directory {}: {e}", dir.display());
            log_warn!("Using the current working directory instead");
            PathBuf::from(".")
        }
    }
}

/// `{temp-dir option or cwd}/{video_id}.f{itag}.state`
pub fn state_file(opts: &Options, video_id: &str, itag: u32) -> PathBuf {
    opts.temp_dir
        .clone()
        .unwrap_or_default()
        .join(format!("{video_id}.f{itag}.state"))
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
            let st = load_state(&path).unwrap_or_default();
            if prev_dir.is_none() && !st.temp_dir.as_os_str().is_empty() && st.temp_dir.is_dir() {
                prev_dir = Some(st.temp_dir.clone());
            }
            DownloadState { file: path, ..st }
        })
        .collect();
    (states, prev_dir)
}

/// Picks the temp dir: the previous run's, else a new `{id}__XXXX` under the
/// temp-dir option (or the final dir). Falls back to the final dir.
pub fn temp_dir(opts: &Options, prev: Option<PathBuf>, final_dir: &Path, video_id: &str) -> PathBuf {
    if let Some(p) = prev {
        return p;
    }
    let base = opts.temp_dir.clone().unwrap_or_else(|| final_dir.to_path_buf());
    match create_temp_dir(&base, &format!("{video_id}__"), opts.dir_mode) {
        Ok(d) => d,
        Err(e) => {
            log_warn!("Error creating temp directory: {e}");
            log_warn!("Will download data directly to {} instead", final_dir.display());
            final_dir.to_path_buf()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::new_format_info;

    fn fi(title: &str) -> FormatInfo {
        let mut f = new_format_info();
        f.insert("title".into(), title.into());
        f.insert("id".into(), "vid".into());
        f.insert("channel".into(), "Chan/nel".into());
        f
    }

    /// Options with `--windows-filenames` on or off.
    fn opts(format: &str, windows: bool) -> Options {
        Options { output_format: format.into(), windows_filenames: windows, ..Default::default() }
    }

    #[test]
    fn location_from_template() {
        let o = opts("%(channel)s/%(title)s-%(id)s", true);
        let (dir, name) = final_location(&o, &fi("A: B")).unwrap();
        assert_eq!(name, "A_ B-vid");
        assert!(dir.is_absolute());
        assert!(dir.ends_with("Chan_nel"));
    }

    #[test]
    fn minimal_sanitizing() {
        let o = opts("%(channel)s/%(title)s", false);
        let mut f = fi("Why? A: B.");
        let (dir, name) = final_location(&o, &f).unwrap();
        assert_eq!(name, "Why? A: B.");
        // A separator in a value still can't create a folder.
        assert!(dir.ends_with("Chan_nel"));
        // Reserved names are left alone. (Not checked on Windows, where
        // making "Con" absolute yields the device path `\\.\Con`, which is
        // why --windows-filenames is the default there.)
        if cfg!(not(windows)) {
            f.insert("channel".into(), "Con".into());
            assert!(final_location(&o, &f).unwrap().0.ends_with("Con"));
        }
    }

    #[test]
    fn leading_dash_and_empty_names() {
        let o = opts("%(title)s", true);
        assert_eq!(final_location(&o, &fi("-x")).unwrap().1, "_-x");
        assert!(final_location(&o, &fi("")).is_err());
        assert!(final_location(&o, &fi(".")).is_err());
        assert_eq!(final_location(&o, &fi("aux")).unwrap().1, "_aux");
        assert_eq!(final_location(&o, &fi("Why?")).unwrap().1, "Why_");
    }

    #[test]
    fn folder_names_are_windows_safe() {
        let o = opts("out/%(channel)s/%(title)s", true);
        let mut f = fi("t");
        f.insert("channel".into(), "Con".into());
        let (dir, _) = final_location(&o, &f).unwrap();
        assert!(dir.ends_with("out/_Con"), "{}", dir.display());
        f.insert("channel".into(), "Mr. ".into());
        let (dir, _) = final_location(&o, &f).unwrap();
        assert!(dir.ends_with("out/Mr__"), "{}", dir.display());
        // `..` in the template itself is kept.
        let up = opts("../%(title)s", true);
        let (dir, _) = final_location(&up, &f).unwrap();
        assert_eq!(dir, std::path::absolute("..").unwrap());
    }

    #[test]
    fn temp_paths() {
        let p = OutputPlan { final_dir: "/f".into(), file_name: "n".into(), temp_dir: "/t/vid__ab12".into() };
        assert_eq!(p.temp(".f140.ts"), PathBuf::from("/t/vid__ab12/n.f140.ts"));
        assert_eq!(p.final_path(".jpg"), PathBuf::from("/f/n.jpg"));
        assert!(p.owns_temp_dir("vid"));
        assert!(!p.owns_temp_dir("other"));
        let home = OutputPlan { temp_dir: "/home/user".into(), ..p.clone() };
        assert!(!home.owns_temp_dir("vid"));
        let same = OutputPlan { temp_dir: "/f".into(), ..p };
        assert!(!same.owns_temp_dir("vid"));
    }

    #[test]
    fn state_paths_and_loading() {
        let dir = std::env::temp_dir().join(format!("ytarchive-rs-out-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let o = Options { temp_dir: Some(dir.clone()), ..Default::default() };
        let path = state_file(&o, "vid", 140);
        assert_eq!(path, dir.join("vid.f140.state"));
        // serde_json escapes Windows backslashes in the path.
        let json = serde_json::json!({"StartFrag": 5, "Fragments": 2, "Size": 10, "TempDir": dir});
        std::fs::write(&path, json.to_string()).unwrap();
        let (states, prev) = load_states(&o, "vid", &[140, 299]);
        assert_eq!(states[0].start_frag, 5);
        assert_eq!(states[0].file, path);
        assert_eq!(states[1].fragments, 0);
        assert_eq!(prev.as_deref(), Some(dir.as_path()));

        let disabled = Options { disable_save_state: true, ..o };
        assert_eq!(load_states(&disabled, "vid", &[140]).0[0].fragments, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
