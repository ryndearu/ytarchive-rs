//! Output filename templating (`%(key)s`) and sanitizing.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{Error, Result};
use crate::log_warn;
use crate::types::FILENAME_FORMAT_BLACKLIST;

/// Max bytes for the final path component: 255 minus room for `.description`.
pub const MAX_FILENAME_LEN: usize = 243;

const FORBIDDEN: [(char, char); 9] =
    [('<', '＜'), ('>', '＞'), (':', '：'), ('"', '″'), ('/', '⧸'), ('\\', '⧹'), ('|', '｜'), ('?', '？'), ('*', '＊')];

/// How file and folder names are sanitized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameStyle {
    /// Replace forbidden characters with full-width lookalikes instead of `_`.
    pub lookalike: bool,
    /// Make names valid on Windows (`--windows-filenames`). When off, only
    /// path separators and NUL are replaced (`--no-windows-filenames`).
    pub windows: bool,
}

/// Characters replaced when Windows rules are off: path separators (both, on
/// Windows) and NUL, which no filesystem accepts.
fn minimal_forbidden(c: char) -> bool {
    c == '/' || c == '\0' || (cfg!(windows) && c == '\\')
}

/// Replaces characters that are illegal in filenames with `_`, or with
/// yt-dlp's full-width lookalikes when `style.lookalike` is set. With Windows
/// rules, that's `<>:"/\|?*` plus control characters (Go kept control
/// characters); otherwise only path separators and NUL.
pub fn sterilize_filename(s: &str, style: NameStyle) -> String {
    s.chars()
        .map(|c| {
            let lookalike = FORBIDDEN.iter().find(|(f, _)| *f == c).map(|(_, l)| *l);
            let replace = if style.windows { lookalike.is_some() || c.is_control() } else { minimal_forbidden(c) };
            match (replace, lookalike) {
                (false, _) => c,
                (true, Some(look)) if style.lookalike => look,
                (true, _) => '_',
            }
        })
        .collect()
}

/// Device names Windows reserves regardless of extension.
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "COM¹", "COM²", "COM³", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹",
    "LPT²", "LPT³",
];

/// Makes a single file or folder name valid on Windows (used with
/// `--windows-filenames`): no trailing dots/spaces, and no reserved device
/// names like `CON` or `nul.txt`.
pub fn portable_file_name(name: &str) -> String {
    let trimmed = name.trim_end_matches(['.', ' ']);
    let mut out = format!("{trimmed}{}", "_".repeat(name.len() - trimmed.len()));
    let stem = out.split('.').next().unwrap_or("");
    if WINDOWS_RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem.trim_end())) {
        out.insert(0, '_');
    }
    out
}

/// Single-pass `%(key)s` substitution. Keys are case-insensitive; unknown keys
/// are an error. Substituted values are never re-scanned, so titles containing
/// `%(...)s` are inserted literally.
pub fn format_python_map(format: &str, vals: &BTreeMap<String, String>) -> Result<String> {
    let mut out = String::with_capacity(format.len());
    for piece in pieces(format) {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Key(key) => {
                let key = key.to_lowercase();
                let value = vals
                    .get(&key)
                    .ok_or_else(|| Error::InvalidFormat(format!("unknown output format key: '{key}'")))?;
                out.push_str(value);
            }
        }
    }
    Ok(out)
}

enum Piece<'a> {
    Text(&'a str),
    /// The `key` of a `%(key)s` field.
    Key(&'a str),
}

/// Splits a template into plain text and `%(key)s` fields (key: letters,
/// digits, and `_`).
fn pieces(format: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut rest = format;
    while let Some((before, after)) = rest.split_once("%(") {
        out.push(Piece::Text(before));
        let key_len = after.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(after.len());
        let (key, tail) = after.split_at(key_len);
        match tail.strip_prefix(")s") {
            Some(tail) if !key.is_empty() => {
                out.push(Piece::Key(key));
                rest = tail;
            }
            // Not a field: keep the text and look again right after it.
            _ => {
                out.push(Piece::Text("%("));
                rest = after;
            }
        }
    }
    out.push(Piece::Text(rest));
    out
}

/// Fills the output template with sanitized values. Blacklisted keys expand to
/// an empty string. If the resulting file name is longer than
/// `MAX_FILENAME_LEN` bytes, or than `max_units` UTF-16 units (the room
/// Windows' MAX_PATH leaves, see `output::final_location`), the title is
/// shortened to fit.
pub fn format_filename(
    format: &str,
    vals: &BTreeMap<String, String>,
    style: NameStyle,
    max_units: Option<usize>,
) -> Result<String> {
    let mut fname_vals: BTreeMap<String, String> = vals
        .iter()
        .map(|(k, v)| {
            let blacklisted = FILENAME_FORMAT_BLACKLIST.iter().any(|b| b.eq_ignore_ascii_case(k.trim()));
            let v = if blacklisted { String::new() } else { sterilize_filename(v, style) };
            (k.clone(), v)
        })
        .collect();

    let out = format_python_map(format, &fname_vals)?;
    let base = Path::new(&out).file_name().map_or("", |n| n.to_str().unwrap_or(""));
    let mut over_bytes = base.len().saturating_sub(MAX_FILENAME_LEN);
    let mut over_units = max_units.map_or(0, |m| base.encode_utf16().count().saturating_sub(m));
    if over_bytes == 0 && over_units == 0 {
        return Ok(out);
    }

    log_warn!("Formatted filename is too long. Truncating the title to try and fix.");
    if let Some(title) = fname_vals.get_mut("title") {
        // The title may appear several times in the base name.
        let occurrences = pieces(Path::new(format).file_name().map_or("", |n| n.to_str().unwrap_or("")))
            .iter()
            .filter(|piece| matches!(piece, Piece::Key(key) if key.eq_ignore_ascii_case("title")))
            .count()
            .max(1);
        over_bytes = over_bytes.div_ceil(occurrences);
        over_units = over_units.div_ceil(occurrences);
        while over_bytes > 0 || over_units > 0 {
            let Some(c) = title.pop() else { break };
            over_bytes = over_bytes.saturating_sub(c.len_utf8());
            over_units = over_units.saturating_sub(c.len_utf16());
        }
    }
    format_python_map(format, &fname_vals)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vals(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    const WIN: NameStyle = NameStyle { lookalike: false, windows: true };
    const WIN_LOOK: NameStyle = NameStyle { lookalike: true, windows: true };
    const MIN: NameStyle = NameStyle { lookalike: false, windows: false };
    const MIN_LOOK: NameStyle = NameStyle { lookalike: true, windows: false };

    #[test]
    fn sterilize_windows() {
        assert_eq!(sterilize_filename(r#"a<b>c:d"e/f\g|h?i*j"#, WIN), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(sterilize_filename("a/b?", WIN_LOOK), "a⧸b？");
        assert_eq!(sterilize_filename("日本語 ok", WIN), "日本語 ok");
        assert_eq!(sterilize_filename("a\tb\nc", WIN_LOOK), "a_b_c");
    }

    #[test]
    fn sterilize_minimal() {
        // Only the path separator (and NUL) is replaced.
        assert_eq!(sterilize_filename("a<b>:\"|?*\tc", MIN), "a<b>:\"|?*\tc");
        assert_eq!(sterilize_filename("a/b\0c", MIN), "a_b_c");
        assert_eq!(sterilize_filename("a/b", MIN_LOOK), "a⧸b");
        let backslash = if cfg!(windows) { "a_b" } else { r"a\b" };
        assert_eq!(sterilize_filename(r"a\b", MIN), backslash);
    }

    #[test]
    fn portable_names() {
        assert_eq!(portable_file_name("normal title"), "normal title");
        assert_eq!(portable_file_name("ends with dot."), "ends with dot_");
        assert_eq!(portable_file_name("dots.. "), "dots___");
        assert_eq!(portable_file_name("CON"), "_CON");
        assert_eq!(portable_file_name("nul.txt"), "_nul.txt");
        assert_eq!(portable_file_name("Com1"), "_Com1");
        assert_eq!(portable_file_name("CONSOLE"), "CONSOLE");
        assert_eq!(portable_file_name("LPT10"), "LPT10");
        assert_eq!(portable_file_name("conin$"), "_conin$");
        assert_eq!(portable_file_name("COM¹.ts"), "_COM¹.ts");
    }

    #[test]
    fn python_map() {
        let v = vals(&[("title", "T"), ("id", "abc")]);
        assert_eq!(format_python_map("%(title)s-%(ID)s", &v).unwrap(), "T-abc");
        assert_eq!(format_python_map("no keys", &v).unwrap(), "no keys");
        assert!(format_python_map("%(nope)s", &v).is_err());
    }

    #[test]
    fn python_map_does_not_reexpand_values() {
        let v = vals(&[("title", "%(title)s %(id)s"), ("id", "abc")]);
        assert_eq!(format_python_map("%(title)s", &v).unwrap(), "%(title)s %(id)s");
    }

    #[test]
    fn filename_blacklist_and_sanitize() {
        let v = vals(&[("title", "a/b"), ("id", "x"), ("description", "secret")]);
        assert_eq!(format_filename("%(title)s-%(id)s%(description)s", &v, WIN, None).unwrap(), "a_b-x");
        // Directory separators in the template itself are kept.
        assert_eq!(format_filename("dir/%(id)s", &v, MIN, None).unwrap(), "dir/x");
        // A separator inside a value is always replaced.
        assert_eq!(format_filename("%(title)s", &v, MIN, None).unwrap(), "a_b");
    }

    #[test]
    fn filename_truncates_long_title() {
        let title = "あ".repeat(200); // 600 bytes
        let v = vals(&[("title", &title), ("id", "abcdefghijk")]);
        let out = format_filename("out/%(title)s [%(id)s]", &v, WIN, None).unwrap();
        let base = Path::new(&out).file_name().unwrap().to_str().unwrap();
        assert!(base.len() <= MAX_FILENAME_LEN, "{}", base.len());
        assert!(base.ends_with(" [abcdefghijk]"));

        let v = vals(&[("title", &title)]);
        let out = format_filename("%(title)s %(title)s", &v, MIN, None).unwrap();
        assert!(out.len() <= MAX_FILENAME_LEN, "{}", out.len());
    }

    #[test]
    fn filename_fits_windows_path_room() {
        // Fits in 243 bytes but not 100 units; emoji are 2 units each.
        let v = vals(&[("title", &"a".repeat(200)), ("id", "abcdefghijk")]);
        let out = format_filename("%(title)s [%(id)s]", &v, WIN, Some(100)).unwrap();
        assert_eq!(out.encode_utf16().count(), 100);
        assert!(out.ends_with(" [abcdefghijk]"));

        let v = vals(&[("title", &"🎄".repeat(50))]);
        let out = format_filename("%(title)s", &v, WIN, Some(51)).unwrap();
        assert_eq!(out.encode_utf16().count(), 50, "a surrogate pair is never split");
        // Short names are left alone.
        let v = vals(&[("title", "short")]);
        assert_eq!(format_filename("%(title)s", &v, WIN, Some(100)).unwrap(), "short");
    }
}
