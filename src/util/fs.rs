//! Filesystem helpers honoring `--file-permissions` / `--directory-permissions`.

use std::path::Path;

use crate::error::Result;

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
}
