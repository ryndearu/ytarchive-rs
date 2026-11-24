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

/// Sleeps for `dur` measured by the wall clock, re-checking every 60 s so
/// system sleep/suspend does not extend the wait. Returns `false` if cancelled.
pub async fn sleep_wall_clock(cancel: &CancellationToken, dur: Duration) -> bool {
    let Some(deadline) = SystemTime::now().checked_add(dur) else {
        return sleep_or_cancel(cancel, dur).await;
    };
    loop {
        let Ok(remaining) = deadline.duration_since(SystemTime::now()) else {
            return true;
        };
        if remaining.is_zero() {
            return true;
        }
        if !sleep_or_cancel(cancel, remaining.min(Duration::from_secs(60))).await {
            return false;
        }
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

/// Copies `src` to `dst` without failing when setting Unix permissions is
/// rejected (e.g. exFAT, FAT32, or SMB mounts).
fn copy_file_data(src: &Path, dst: &Path) -> io::Result<()> {
    let mut reader = fs::File::open(src)?;
    let mut writer = fs::File::create(dst)?;
    io::copy(&mut reader, &mut writer)?;
    #[cfg(unix)]
    if let Ok(metadata) = reader.metadata() {
        use std::os::unix::fs::PermissionsExt;
        apply_mode(dst, metadata.permissions().mode() & 0o7777);
    }
    Ok(())
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
    match rename_atomic(src, dst) {
        Ok(()) => return Ok(()),
        Err(error) => crate::log_debug!("Rename failed ({error}); copying {} instead", src.display()),
    }
    if let Err(error) = copy_file_data(src, dst) {
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
mod tests;
