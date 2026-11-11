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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::log_warn;
use crate::util::lock;

const HEADER: &str = "# Netscape HTTP Cookie File";
const HTTPONLY_PREFIX: &str = "#HttpOnly_";

/// The copy in use, removed on exit (also before `process::exit`).
static ACTIVE: Mutex<Option<PathBuf>> = Mutex::new(None);

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

/// If yt-dlp would refuse the cookies file, writes a repaired copy to the
/// temp folder (readable only by the user) and returns its path.
pub fn prepare(original: &Path, data: &[u8]) -> std::io::Result<Option<PathBuf>> {
    let Some(why) = problem(data) else {
        return Ok(None);
    };
    let path = std::env::temp_dir().join(format!("ytarchive-cookies-{}.txt", std::process::id()));
    // A copy left by an earlier process with the same id; `create_new` below
    // reports the problem if it can't be replaced.
    delete(&path);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(&path)?.write_all(repair(data).as_bytes())?;
    *lock(&ACTIVE) = Some(path.clone());
    log_warn!(
        "yt-dlp can't read the cookies file {} ({why}); passing it a repaired copy. Cookies yt-dlp refreshes won't be saved back to the file for this run.",
        original.display()
    );
    Ok(Some(path))
}

/// Deletes the repaired copy, if any.
pub fn remove() {
    if let Some(path) = lock(&ACTIVE).take() {
        delete(&path);
    }
}

fn delete(path: &Path) {
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        log_warn!("Could not delete the repaired cookies copy {}: {error}", path.display());
    }
}

#[cfg(test)]
mod tests {
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
}
