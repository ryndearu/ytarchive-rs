use super::*;

const GOOD: &[u8] = b"# Netscape HTTP Cookie File\n\
    .youtube.com\tTRUE\t/\tTRUE\t0\tA\tb\n\
    #HttpOnly_www.youtube.com\tFALSE\t/\tTRUE\t0\tB\tc\n\
    bad\tline\n";

#[test]
fn clean_file_is_left_alone() {
    assert_eq!(problem(GOOD), None);
    assert_eq!(problem(b"# HTTP Cookie File\r\nx.com\tFALSE\t/\tFALSE\t1.5\tA\tb\r\n"), None);
    // yt-dlp drops the first line, so the header is the first line it keeps.
    assert_eq!(problem(b"bad\tline\n# Netscape HTTP Cookie File\n"), None);
    // A JSON export is left for yt-dlp to reject with its own clear error.
    assert_eq!(problem(b"[{\"domain\": \".youtube.com\"}]\n"), None);
}

#[test]
fn detects_what_ytdlp_refuses() {
    assert!(problem(b"# Netscape HTTP Cookie File\n.y.com\tTRUE\t/\tTRUE\t0\tA\tb\xff\n").is_some());
    assert!(problem(b".youtube.com\tTRUE\t/\tTRUE\t0\tA\tb\n").is_some());
    assert!(problem(b"").is_some());
    assert!(problem(b"\xef\xbb\xbf# Netscape HTTP Cookie File\n").is_some());
    assert!(problem(b"\n# Netscape HTTP Cookie File\n").is_some());
    assert!(problem(b"# Netscape HTTP Cookie File\n{\"stray\": 1}\n").is_some());
    assert!(problem(b"# Netscape HTTP Cookie File\nyoutube.com\tTRUE\t/\tTRUE\t0\tA\tb\n").is_some());
    assert!(problem(b"# Netscape HTTP Cookie File\n#HttpOnly_.y.com\tfalse\t/\tTRUE\t0\tA\tb\n").is_some());
}

#[test]
fn repairs_into_a_file_ytdlp_reads() {
    let bad = b".y.com\tTRUE\t/\tTRUE\t0\tA\tb\xffc\n  \"x\": 1\nyoutube.com\tTRUE\t/\tTRUE\t0\tB\td\r\n";
    let fixed = repair(bad);
    assert_eq!(problem(fixed.as_bytes()), None);
    assert_eq!(repair(b""), "# Netscape HTTP Cookie File\n");
    assert_eq!(
        fixed,
        "# Netscape HTTP Cookie File\n.y.com\tTRUE\t/\tTRUE\t0\tA\tbc\nyoutube.com\tFALSE\t/\tTRUE\t0\tB\td\n"
    );
}

#[test]
fn removes_only_stale_matching_cookie_files() {
    let dir = env::temp_dir().join(format!("ytarchive-rs-cookie-clean-{}", process::id()));
    fs::create_dir_all(&dir).unwrap();
    let current = dir.join("ytarchive-cookies-1-current.txt");
    let stale = dir.join("ytarchive-cookies-1-stale.txt");
    let live = dir.join("ytarchive-cookies-2-live.txt");
    let unrelated = dir.join("other.txt");
    fs::write(&current, b"cur").unwrap();
    fs::write(&stale, b"old").unwrap();
    fs::write(&live, b"active").unwrap();
    fs::write(&unrelated, b"keep").unwrap();
    let live_lock = acquire_lock(&lock_path_for(&live)).unwrap();

    let minute = Duration::from_secs(60);
    let past_cutoff = SystemTime::now() - minute;
    cleanup_stale_cookie_files(&dir, &current, past_cutoff);
    assert!(stale.exists());
    assert!(live.exists());

    let future_cutoff = SystemTime::now() + minute;
    cleanup_stale_cookie_files(&dir, &current, future_cutoff);
    assert!(current.exists());
    assert!(!stale.exists());
    assert!(live.exists(), "a locked cookie file from a running process must be kept");
    assert!(unrelated.exists());

    drop(live_lock);
    // A parallel test spawning a subprocess can briefly hold a forked copy
    // of the lock fd across fork/exec; retry cleanup briefly if needed.
    for _ in 0..20 {
        cleanup_stale_cookie_files(&dir, &current, future_cutoff);
        if !live.exists() && !lock_path_for(&live).exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!live.exists());
    assert!(!lock_path_for(&live).exists());
    fs::remove_dir_all(&dir).unwrap();
}
