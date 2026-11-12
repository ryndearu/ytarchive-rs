//! Small helpers: filesystem (honoring `--file-permissions` /
//! `--directory-permissions`), locking and sleeping, and human-readable
//! sizes and durations.

pub mod filename;

use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

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
pub fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
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
        set_mode(path, mode)?;
        Ok(())
    }
    .await;
    if res.is_err()
        && let Err(error) = tokio::fs::remove_file(path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        crate::log_debug!("Could not delete the partial file {}: {error}", path.display());
    }
    res
}

/// Replaces `path` with `data` in one step (a temporary file renamed over
/// it), so a crash mid-write leaves the old contents instead of a partial
/// file.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> std::io::Result<()> {
    let mut temp_name = path.as_os_str().to_os_string();
    temp_name.push(".tmp");
    let temp_path = std::path::PathBuf::from(temp_name);
    let res = std::fs::write(&temp_path, data)
        .and_then(|()| set_mode(&temp_path, mode))
        .and_then(|()| std::fs::rename(&temp_path, path));
    if res.is_err()
        && let Err(error) = std::fs::remove_file(&temp_path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        crate::log_debug!("Could not delete the partial file {}: {error}", temp_path.display());
    }
    res
}

/// Moves `src` to `dst`, copying across filesystems if rename fails.
/// A missing `src` is not an error (nothing to move).
pub fn try_move(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !src.exists() {
        return Ok(());
    }
    if src == dst {
        return Ok(());
    }
    crate::log_info!("Moving file {} to {}", src.display(), dst.display());
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    crate::log_warn!("Rename failed; copying {} instead", src.display());
    std::fs::copy(src, dst)
        .and_then(|_| std::fs::remove_file(src))
        .inspect_err(|error| crate::log_warn!("Error moving file {}: {error}", src.display()))
}

/// Deletes `path` if it exists, logging (not returning) errors.
pub fn try_delete(path: &Path) {
    if !path.exists() {
        return;
    }
    crate::log_info!("Deleting file {}", path.display());
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        crate::log_warn!("Error deleting file {}: {error}", path.display());
    }
}

/// Creates a new uniquely named directory `{parent}/{prefix}{random}`.
pub fn create_temp_dir(parent: &Path, prefix: &str, mode: u32) -> std::io::Result<std::path::PathBuf> {
    let seed =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |duration| duration.as_nanos())
            ^ u128::from(std::process::id());
    for attempt in 0..100u128 {
        let dir = parent.join(format!("{prefix}{:x}", (seed.wrapping_add(attempt)) & 0xffff_ffff));
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                apply_mode(&dir, mode);
                return Ok(dir);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other("could not create a unique temporary directory"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_with_mode() {
        let dir = std::env::temp_dir().join(format!("ytarchive-rs-fs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.bin");
        write_file_with_mode(&file, b"hello", 0o600).await.unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!("ytarchive-rs-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.state");
        write_atomic(&file, b"old", 0o644).unwrap();
        write_atomic(&file, b"new", 0o644).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            write_atomic(&file, b"newer", 0o600).unwrap();
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }

        // A failed write keeps the old contents and leaves no temp file.
        let blocked = dir.join("y.state");
        std::fs::create_dir(dir.join("y.state.tmp")).unwrap();
        std::fs::write(&blocked, b"old").unwrap();
        assert!(write_atomic(&blocked, b"new", 0o644).is_err());
        assert_eq!(std::fs::read(&blocked).unwrap(), b"old");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn move_replaces_the_destination_and_skips_a_missing_source() {
        let dir = std::env::temp_dir().join(format!("ytarchive-rs-move-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (source, destination) = (dir.join("a.ts"), dir.join("b.ts"));
        std::fs::write(&source, b"data").unwrap();
        std::fs::write(&destination, b"stale").unwrap();
        try_move(&source, &destination).unwrap();
        assert!(!source.exists());
        assert_eq!(std::fs::read(&destination).unwrap(), b"data");
        try_move(&source, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"data");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
