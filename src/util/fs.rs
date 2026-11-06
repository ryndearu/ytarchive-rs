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

/// Writes `data` to `path` and sets its mode. Removes the file on failure so
/// no partial file is left behind.
pub async fn write_file_with_mode(path: &Path, data: &[u8], mode: u32) -> Result<()> {
    let res = async {
        tokio::fs::write(path, data).await?;
        set_mode(path, mode)?;
        Ok(())
    }
    .await;
    if res.is_err() {
        let _ = tokio::fs::remove_file(path).await;
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
        .inspect_err(|e| crate::log_warn!("Error moving file {}: {e}", src.display()))
}

/// Deletes `path` if it exists, logging (not returning) errors.
pub fn try_delete(path: &Path) {
    if !path.exists() {
        return;
    }
    crate::log_info!("Deleting file {}", path.display());
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        crate::log_warn!("Error deleting file {}: {e}", path.display());
    }
}

/// Creates a new uniquely named directory `{parent}/{prefix}{random}`.
pub fn create_temp_dir(parent: &Path, prefix: &str, mode: u32) -> std::io::Result<std::path::PathBuf> {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
        ^ u128::from(std::process::id());
    for i in 0..100u128 {
        let dir = parent.join(format!("{prefix}{:x}", (seed.wrapping_add(i)) & 0xffff_ffff));
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                let _ = set_mode(&dir, mode);
                return Ok(dir);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
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
    fn move_delete_and_temp_dir() {
        let base = std::env::temp_dir().join(format!("ytarchive-rs-mv-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let t1 = create_temp_dir(&base, "vid__", 0o755).unwrap();
        let t2 = create_temp_dir(&base, "vid__", 0o755).unwrap();
        assert_ne!(t1, t2);
        assert!(t1.file_name().unwrap().to_str().unwrap().starts_with("vid__"));

        let src = t1.join("a.ts");
        std::fs::write(&src, b"x").unwrap();
        let dst = base.join("a.ts");
        try_move(&src, &dst).unwrap();
        assert!(!src.exists() && dst.exists());
        // Missing source is fine.
        try_move(&src, &dst).unwrap();
        try_delete(&dst);
        assert!(!dst.exists());
        try_delete(&dst);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[tokio::test]
    async fn failure_leaves_no_file() {
        let missing = std::env::temp_dir().join("ytarchive-rs-no-such-dir/x.bin");
        assert!(write_file_with_mode(&missing, b"x", 0o644).await.is_err());
        assert!(!missing.exists());
    }
}
