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
