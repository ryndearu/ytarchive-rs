//! Small helpers: filesystem (honoring `--file-permissions` /
//! `--directory-permissions`), locking and sleeping, and human-readable
//! sizes and durations.

pub mod filename;

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio_util::sync::CancellationToken;

pub use filename::{NameStyle, format_filename, format_python_map, portable_file_name, sterilize_filename};

use crate::error::Result;

/// Sleeps unless cancelled first. Returns `false` if cancelled.
pub async fn sleep_or_cancel(cancel: &CancellationToken, dur: Duration) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(dur) => true,
    }
}

/// Locks `mutex` even if a thread panicked while holding it. The values
/// guarded here (counters, URL lists, timestamps) stay usable after a panic,
/// and one failed task shouldn't take the whole download down with it.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

const KIB: f64 = 1024.0;
const MIB: f64 = KIB * 1024.0;
const GIB: f64 = MIB * 1024.0;

pub fn format_size(bytes: u64) -> String {
    let size = bytes as f64;
    if size >= GIB {
        format!("{:.2}GiB", size / GIB)
    } else if size >= MIB {
        format!("{:.2}MiB", size / MIB)
    } else if size >= KIB {
        format!("{:.2}KiB", size / KIB)
    } else {
        format!("{bytes}B")
    }
}

/// `1d2h3m4s`; zero-valued leading units are omitted, seconds always shown.
pub fn seconds_to_duration_str(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3600;
    let minutes = seconds % 3600 / 60;
    let secs = seconds % 60;

    let mut out = String::new();
    if days > 0 {
        out.push_str(&format!("{days}d"));
    }
    if hours > 0 {
        out.push_str(&format!("{hours}h"));
    }
    if minutes > 0 {
        out.push_str(&format!("{minutes}m"));
    }
    out.push_str(&format!("{secs}s"));
    out
}

/// `H:MM:SS` when an hour or more, otherwise `MM:SS`. Hours are not wrapped into days.
pub fn seconds_to_time_str(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    let secs = seconds % 60;
    if hours > 0 { format!("{hours}:{minutes:02}:{secs:02}") } else { format!("{minutes:02}:{secs:02}") }
}

/// `1h1m1s (1:01:01)`
pub fn seconds_to_duration_and_time_str(seconds: u64) -> String {
    format!("{} ({})", seconds_to_duration_str(seconds), seconds_to_time_str(seconds))
}

/// Applies a unix permission mode. No-op on other platforms.
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// `set_mode` for a file or folder that is usable either way: a failure
/// (e.g. a filesystem without permissions) is only logged.
pub fn apply_mode(path: &Path, mode: u32) {
    if let Err(error) = set_mode(path, mode) {
        crate::log_debug!("Could not set the permissions of {}: {error}", path.display());
    }
}

/// Writes `data` to `path` and sets its mode. Removes the file on failure so
/// no partial file is left behind.
pub async fn write_file_with_mode(path: &Path, data: &[u8], mode: u32) -> Result<()> {
    let res = async {
        tokio::fs::write(path, data).await?;
        apply_mode(path, mode);
        Ok(())
    }
    .await;
    if res.is_err()
        && let Err(error) = tokio::fs::remove_file(path).await
        && error.kind() != io::ErrorKind::NotFound
    {
        crate::log_debug!("Could not delete the partial file {}: {error}", path.display());
    }
    res
}

#[cfg(windows)]
fn rename_atomic(from: &Path, to: &Path) -> io::Result<()> {
    // Antivirus and search indexers on Windows can briefly hold a handle on
    // the state or temp file, causing transient access-denied (5) or
    // sharing-violation (32) errors.
    let mut delay = Duration::from_millis(10);
    for _ in 0..3 {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error)
                if error.kind() == io::ErrorKind::PermissionDenied || matches!(error.raw_os_error(), Some(5 | 32)) =>
            {
                std::thread::sleep(delay);
                delay *= 2;
            }
            Err(error) => return Err(error),
        }
    }
    fs::rename(from, to)
}

#[cfg(not(windows))]
fn rename_atomic(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

/// Replaces `path` with `data` in one step (a temporary file renamed over
/// it), so a crash mid-write leaves the old contents instead of a partial
/// file.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    let mut temp_name = path.as_os_str().to_os_string();
    temp_name.push(".tmp");
    let temp_path = PathBuf::from(temp_name);
    let res = fs::write(&temp_path, data).and_then(|()| {
        apply_mode(&temp_path, mode);
        rename_atomic(&temp_path, path)
    });
    if res.is_err()
        && let Err(error) = fs::remove_file(&temp_path)
        && error.kind() != io::ErrorKind::NotFound
    {
        crate::log_debug!("Could not delete the partial file {}: {error}", temp_path.display());
    }
    res
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(any(windows, test))]
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// Candidate paths for an executable on Windows, honoring `PATHEXT` and never
/// matching bare extensionless files (such as POSIX shims on `PATH`).
#[cfg(any(windows, test))]
fn windows_executable_candidates(candidate: &Path, pathext: Option<&str>) -> Vec<PathBuf> {
    let raw = pathext.filter(|value| !value.trim().is_empty()).unwrap_or(DEFAULT_PATHEXT);
    let exts: Vec<&str> = raw.split(';').map(str::trim).filter(|ext| !ext.is_empty()).collect();
    let has_pathext = candidate
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| exts.iter().any(|known| known.trim_start_matches('.').eq_ignore_ascii_case(ext)));
    if has_pathext {
        return vec![candidate.to_path_buf()];
    }
    let mut out = Vec::with_capacity(exts.len() + 1);
    for ext in exts {
        let mut with_ext = candidate.as_os_str().to_os_string();
        if !ext.starts_with('.') {
            with_ext.push(".");
        }
        with_ext.push(ext);
        out.push(PathBuf::from(with_ext));
    }
    if candidate.extension().is_some() {
        out.push(candidate.to_path_buf());
    }
    out
}

fn find_executable(candidate: PathBuf) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let pathext = env::var("PATHEXT").ok();
        windows_executable_candidates(&candidate, pathext.as_deref()).into_iter().find(|path| is_executable_file(path))
    }
    #[cfg(not(windows))]
    {
        is_executable_file(&candidate).then_some(candidate)
    }
}

/// Resolves `program` as a path (if it contains a separator), else next to the
/// current executable, else on `PATH`. Does not fall back to the working
/// directory for bare names.
pub fn resolve_program(program: &str) -> Option<PathBuf> {
    if program.is_empty() {
        return None;
    }
    let path = Path::new(program);
    if path.components().count() > 1 {
        return find_executable(path.to_path_buf());
    }
    let exe_dir = env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
    let path_dirs = env::var_os("PATH").map(|paths| env::split_paths(&paths).collect::<Vec<_>>()).unwrap_or_default();
    exe_dir.into_iter().chain(path_dirs).find_map(|dir| find_executable(dir.join(program)))
}

/// Moves `src` to `dst`, copying across filesystems if rename fails.
/// A missing `src` is not an error (nothing to move).
pub fn try_move(src: &Path, dst: &Path) -> io::Result<()> {
    if !src.exists() {
        return Ok(());
    }
    if src == dst {
        return Ok(());
    }
    crate::log_info!("Moving file {} to {}", src.display(), dst.display());
    match fs::rename(src, dst) {
        Ok(()) => return Ok(()),
        Err(error) => crate::log_debug!("Rename failed ({error}); copying {} instead", src.display()),
    }
    if let Err(error) = fs::copy(src, dst) {
        if let Err(remove_err) = fs::remove_file(dst)
            && remove_err.kind() != io::ErrorKind::NotFound
        {
            crate::log_debug!("Could not delete the partial file {}: {remove_err}", dst.display());
        }
        crate::log_warn!("Error moving file {}: {error}", src.display());
        return Err(error);
    }
    fs::remove_file(src).inspect_err(|error| crate::log_warn!("Error moving file {}: {error}", src.display()))
}

/// Deletes `path` if it exists, logging (not returning) errors.
pub fn try_delete(path: &Path) {
    if !path.exists() {
        return;
    }
    crate::log_info!("Deleting file {}", path.display());
    if let Err(error) = fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        crate::log_warn!("Error deleting file {}: {error}", path.display());
    }
}

/// Creates a new uniquely named directory `{parent}/{prefix}{random}`.
pub fn create_temp_dir(parent: &Path, prefix: &str, mode: u32) -> io::Result<PathBuf> {
    let seed = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_nanos())
        ^ u128::from(process::id());
    for attempt in 0..100u128 {
        let dir = parent.join(format!("{prefix}{:x}", (seed.wrapping_add(attempt)) & 0xffff_ffff));
        match fs::create_dir(&dir) {
            Ok(()) => {
                apply_mode(&dir, mode);
                return Ok(dir);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other("could not create a unique temporary directory"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_with_mode() {
        let dir = env::temp_dir().join(format!("ytarchive-rs-fs-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.bin");
        write_file_with_mode(&file, b"hello", 0o600).await.unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp_file() {
        let dir = env::temp_dir().join(format!("ytarchive-rs-atomic-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.state");
        write_atomic(&file, b"old", 0o644).unwrap();
        write_atomic(&file, b"new", 0o644).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"new");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            write_atomic(&file, b"newer", 0o600).unwrap();
            assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }

        // A failed write keeps the old contents and leaves no temp file.
        let blocked = dir.join("y.state");
        fs::create_dir(dir.join("y.state.tmp")).unwrap();
        fs::write(&blocked, b"old").unwrap();
        assert!(write_atomic(&blocked, b"new", 0o644).is_err());
        assert_eq!(fs::read(&blocked).unwrap(), b"old");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn move_replaces_the_destination_and_skips_a_missing_source() {
        let dir = env::temp_dir().join(format!("ytarchive-rs-move-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (source, destination) = (dir.join("a.ts"), dir.join("b.ts"));
        fs::write(&source, b"data").unwrap();
        fs::write(&destination, b"stale").unwrap();
        try_move(&source, &destination).unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read(&destination).unwrap(), b"data");
        try_move(&source, &destination).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"data");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn windows_candidates_follow_pathext_and_skip_extensionless_shims() {
        let bare = windows_executable_candidates(Path::new("yt-dlp"), Some(".EXE;.CMD"));
        assert_eq!(bare, [PathBuf::from("yt-dlp.EXE"), PathBuf::from("yt-dlp.CMD")]);

        let explicit = windows_executable_candidates(Path::new("yt-dlp.cmd"), Some(".EXE;.CMD"));
        assert_eq!(explicit, [PathBuf::from("yt-dlp.cmd")]);

        let dotted = windows_executable_candidates(Path::new("yt-dlp.nightly"), Some(".EXE"));
        assert_eq!(dotted, [PathBuf::from("yt-dlp.nightly.EXE"), PathBuf::from("yt-dlp.nightly")]);
    }
}
