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

    #[tokio::test]
    async fn failure_leaves_no_file() {
        let missing = std::env::temp_dir().join("ytarchive-rs-no-such-dir/x.bin");
        assert!(write_file_with_mode(&missing, b"x", 0o644).await.is_err());
        assert!(!missing.exists());
    }
}
