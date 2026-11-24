//! A repaired copy of the cookies file for yt-dlp, made only when yt-dlp
//! would refuse the original.
//!
//! ytarchive reads cookies as Go did and accepts files that yt-dlp (Python's
//! `MozillaCookieJar`) rejects outright: invalid UTF-8, a missing
//! `# Netscape HTTP Cookie File` header, or a cookie whose include-subdomains
//! column disagrees with its domain's leading dot. Go passed such a file to
//! yt-dlp anyway, which then failed. The copy fixes just those problems. A
//! clean file is passed as-is, so yt-dlp can still save refreshed cookies
//! back to it.

use std::borrow::Cow;
use std::env;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, ErrorKind, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::util::lock;
use crate::{log_debug, log_warn};

const HEADER: &str = "# Netscape HTTP Cookie File";
const HTTPONLY_PREFIX: &str = "#HttpOnly_";

struct ActiveCookies {
    path: PathBuf,
    lock_path: PathBuf,
    /// Held for the lifetime of this process so another instance's stale-file
    /// cleanup never deletes a repaired cookies file still in use.
    _lock: Option<File>,
}

/// The copy in use, removed on exit (also before `process::exit`).
static ACTIVE: Mutex<Option<ActiveCookies>> = Mutex::new(None);

fn lock_path_for(cookie_path: &Path) -> PathBuf {
    let mut name = cookie_path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

fn acquire_lock(lock_path: &Path) -> Option<File> {
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    opts.mode(0o600);
    let file = opts
        .open(lock_path)
        .inspect_err(|error| log_debug!("Could not open cookies lock file {}: {error}", lock_path.display()))
        .ok()?;
    match file.try_lock() {
        Ok(()) => Some(file),
        Err(error) => {
            log_debug!("Could not lock cookies file {}: {error}", lock_path.display());
            None
        }
    }
}

/// Returns `true` if `cookie_path` has a companion lock file currently held by
/// a running process (or whose lock state could not be checked).
fn is_locked_by_live_process(cookie_path: &Path) -> bool {
    let lock_path = lock_path_for(cookie_path);
    let file = match OpenOptions::new().read(true).write(true).open(&lock_path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    match file.try_lock() {
        Ok(()) => false,
        Err(TryLockError::WouldBlock) | Err(TryLockError::Error(_)) => true,
    }
}

/// Python's universal newlines: `\r\n`, `\r`, and `\n` all end a line.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n').flat_map(|line| line.strip_suffix('\r').unwrap_or(line).split('\r'))
}

/// Python's `NETSCAPE_MAGIC_RGX.match`: `#( Netscape)? HTTP Cookie File` at the start.
fn is_header(line: &str) -> bool {
    line.starts_with("# HTTP Cookie File") || line.starts_with(HEADER)
}

fn valid_expiry(expiry: &str) -> bool {
    let mut parts = expiry.splitn(2, '.');
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    parts.next().is_some_and(digits) && parts.next().is_none_or(digits)
}

/// yt-dlp's `prepare_line`: the line Python gets to see, or `None` for a
/// line yt-dlp skips with a warning (wrong field count or expiry).
fn kept(line: &str) -> Option<&str> {
    let line = line.strip_prefix(HTTPONLY_PREFIX).unwrap_or(line);
    if line.starts_with('#') || line.trim().is_empty() {
        return Some(line);
    }
    let fields: Vec<&str> = line.split('\t').collect();
    match fields.as_slice() {
        [_, _, _, _, expiry, _, _] if expiry.is_empty() || valid_expiry(expiry) => Some(line),
        _ => None,
    }
}

/// For a cookie line Python would reject (`assert domain_specified ==
/// initial_dot`), the include-subdomains value that matches its domain.
fn fixed_flag(line: &str) -> Option<&'static str> {
    let line = kept(line)?;
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('$') {
        return None;
    }
    let mut fields = line.split('\t');
    let dot = fields.next()?.starts_with('.');
    ((fields.next()? == "TRUE") != dot).then_some(if dot { "TRUE" } else { "FALSE" })
}

/// A line yt-dlp would skip that starts like JSON: yt-dlp then stops with
/// "Cookies file must be Netscape formatted, not JSON" instead of skipping it.
fn json_like(line: &str) -> bool {
    kept(line).is_none() && line.trim().starts_with(['[', '{', '"'])
}

/// A whole JSON cookie export. It isn't repaired: ytarchive can't read it
/// either, and yt-dlp's "not JSON" error explains the problem.
fn is_json_file(data: &[u8]) -> bool {
    let text = String::from_utf8_lossy(data);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with(['[', '{'])
}

/// Python checks the header on the first line yt-dlp keeps.
fn has_header(text: &str) -> bool {
    lines(text).find_map(kept).is_some_and(is_header)
}

/// Why yt-dlp would refuse `data`, or `None` if it reads it fine.
pub fn problem(data: &[u8]) -> Option<&'static str> {
    if is_json_file(data) {
        return None;
    }
    let Ok(text) = std::str::from_utf8(data) else {
        return Some("it isn't valid UTF-8");
    };
    if !has_header(text) {
        return Some("it doesn't start with \"# Netscape HTTP Cookie File\"");
    }
    if lines(text).any(json_like) {
        return Some("a line looks like JSON");
    }
    lines(text)
        .filter(|line| kept(line).is_some())
        .skip(1)
        .any(|line| fixed_flag(line).is_some())
        .then_some("a cookie's include-subdomains column doesn't match its domain")
}

/// `data` with invalid UTF-8, a byte-order mark, and JSON-like lines
/// dropped, the header added if missing, and include-subdomains columns
/// matched to the domains' leading dots.
pub fn repair(data: &[u8]) -> String {
    let text: String = data.utf8_chunks().map(|chunk| chunk.valid()).collect();
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut all: Vec<&str> = lines(text).collect();
    if all.last() == Some(&"") {
        all.pop();
    }
    let mut out = String::new();
    if !has_header(text) {
        out.push_str(HEADER);
        out.push('\n');
    }
    for line in all.into_iter().filter(|candidate| !json_like(candidate)) {
        let line: Cow<str> = match fixed_flag(line) {
            Some(flag) => {
                let fields: Vec<&str> = line
                    .split('\t')
                    .enumerate()
                    .map(|(column, field)| if column == 1 { flag } else { field })
                    .collect();
                fields.join("\t").into()
            }
            None => line.into(),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

pub struct CookiesGuard;

impl Drop for CookiesGuard {
    fn drop(&mut self) {
        remove();
    }
}

pub fn guard() -> CookiesGuard {
    CookiesGuard
}

/// If yt-dlp would refuse the cookies file, writes a repaired copy to the
/// temp folder (readable only by the user) and returns its path.
pub fn prepare(original: &Path, data: &[u8]) -> io::Result<Option<PathBuf>> {
    let Some(why) = problem(data) else {
        return Ok(None);
    };
    let pid = process::id();
    let seed = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_nanos()) ^ u128::from(pid);
    let temp_dir = env::temp_dir();
    let repaired = repair(data);
    let mut created = None;
    for attempt in 0..100u128 {
        let suffix = (seed.wrapping_add(attempt)) & 0xffff_ffff;
        let path = temp_dir.join(format!("ytarchive-cookies-{pid}-{suffix:08x}.txt"));
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        opts.mode(0o600);
        match opts.open(&path) {
            Ok(mut file) => {
                let write_res = file.write_all(repaired.as_bytes());
                drop(file);
                if let Err(error) = write_res {
                    delete(&path);
                    return Err(error);
                }
                created = Some(path);
                break;
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    let path = created.ok_or_else(|| io::Error::other("could not create a unique temporary cookies file"))?;
    let lock_path = lock_path_for(&path);
    let active_lock = acquire_lock(&lock_path);
    *lock(&ACTIVE) = Some(ActiveCookies { path: path.clone(), lock_path, _lock: active_lock });
    if let Some(cutoff) = SystemTime::now().checked_sub(Duration::from_secs(24 * 60 * 60)) {
        cleanup_stale_cookie_files(&temp_dir, &path, cutoff);
    }
    log_warn!(
        "yt-dlp can't read the cookies file {} ({why}); passing it a repaired copy. Cookies yt-dlp refreshes won't be saved back to the file for this run.",
        original.display()
    );
    Ok(Some(path))
}

fn cleanup_stale_cookie_files(dir: &Path, current: &Path, cutoff: SystemTime) {
    #[cfg(unix)]
    let owner_uid = fs::symlink_metadata(current).ok().map(|meta| meta.uid());
    let current_lock = lock_path_for(current);
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path == current || entry_path == current_lock {
            continue;
        }
        let Some(name) = entry_path.file_name().and_then(|file| file.to_str()) else {
            continue;
        };
        let is_cookie = name.starts_with("ytarchive-cookies-") && name.ends_with(".txt");
        let is_orphan_lock = name.starts_with("ytarchive-cookies-") && name.ends_with(".txt.lock");
        if !is_cookie && !is_orphan_lock {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&entry_path) else {
            continue;
        };
        if !meta.file_type().is_file() {
            continue;
        }
        #[cfg(unix)]
        if owner_uid != Some(meta.uid()) {
            continue;
        }
        if !meta.modified().is_ok_and(|mtime| mtime <= cutoff) {
            continue;
        }
        if is_cookie {
            if is_locked_by_live_process(&entry_path) {
                continue;
            }
            delete(&entry_path);
            delete(&lock_path_for(&entry_path));
        } else if let Some(cookie_name) = name.strip_suffix(".lock") {
            let cookie_path = dir.join(cookie_name);
            if !cookie_path.exists() && !is_locked_by_live_process(&cookie_path) {
                delete(&entry_path);
            }
        }
    }
}

/// Deletes the repaired copy, if any.
pub fn remove() {
    if let Some(active) = lock(&ACTIVE).take() {
        // Release the lock handle before removing the lock file (required on Windows).
        drop(active._lock);
        delete(&active.path);
        delete(&active.lock_path);
    }
}

fn delete(path: &Path) {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != ErrorKind::NotFound
    {
        log_warn!("Could not delete the repaired cookies copy {}: {error}", path.display());
    }
}

#[cfg(test)]
mod tests;
