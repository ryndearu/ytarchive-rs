//! Output filename templating (`%(key)s`) and sanitizing.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::error::{Error, Result};
use crate::log_warn;
use crate::types::FILENAME_FORMAT_BLACKLIST;

/// Max bytes for the final path component: 255 minus room for `.description`.
pub const MAX_FILENAME_LEN: usize = 243;

static MAP_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"%\((\w+)\)s").unwrap());

const FORBIDDEN: [(char, char); 9] = [
    ('<', '＜'),
    ('>', '＞'),
    (':', '：'),
    ('"', '″'),
    ('/', '⧸'),
    ('\\', '⧹'),
    ('|', '｜'),
    ('?', '？'),
    ('*', '＊'),
];

/// Replaces characters that are illegal in filenames with `_`, or with
/// yt-dlp's full-width lookalikes when `lookalike` is set.
pub fn sterilize_filename(s: &str, lookalike: bool) -> String {
    s.chars()
        .map(|c| match FORBIDDEN.iter().find(|(f, _)| *f == c) {
            Some((_, look)) if lookalike => *look,
            Some(_) => '_',
            None => c,
        })
        .collect()
}

/// Truncates to at most `max_bytes` without splitting a UTF-8 character.
pub fn truncate_to_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Single-pass `%(key)s` substitution. Keys are case-insensitive; unknown keys
/// are an error. Substituted values are never re-scanned, so titles containing
/// `%(...)s` are inserted literally.
pub fn format_python_map(format: &str, vals: &BTreeMap<String, String>) -> Result<String> {
    if let Some(unknown) = MAP_KEY
        .captures_iter(format)
        .map(|c| c[1].to_lowercase())
        .find(|k| !vals.contains_key(k))
    {
        return Err(Error::InvalidFormat(format!(
            "unknown output format key: '{unknown}'"
        )));
    }
    Ok(MAP_KEY
        .replace_all(format, |c: &regex::Captures<'_>| {
            vals[&c[1].to_lowercase()].clone()
        })
        .into_owned())
}

/// Fills the output template with sanitized values. Blacklisted keys expand to
/// an empty string. If the resulting file name is too long, the title is
/// shortened to fit.
pub fn format_filename(
    format: &str,
    vals: &BTreeMap<String, String>,
    lookalike: bool,
) -> Result<String> {
    let mut fname_vals: BTreeMap<String, String> = vals
        .iter()
        .map(|(k, v)| {
            let blacklisted = FILENAME_FORMAT_BLACKLIST
                .iter()
                .any(|b| b.eq_ignore_ascii_case(k.trim()));
            let v = if blacklisted {
                String::new()
            } else {
                sterilize_filename(v, lookalike)
            };
            (k.clone(), v)
        })
        .collect();

    let out = format_python_map(format, &fname_vals)?;
    let base_len = Path::new(&out)
        .file_name()
        .map_or(0, |n| n.as_encoded_bytes().len());
    if base_len <= MAX_FILENAME_LEN {
        return Ok(out);
    }

    log_warn!("Formatted filename is too long. Truncating the title to try and fix.");
    let over = base_len - MAX_FILENAME_LEN;
    if let Some(title) = fname_vals.get_mut("title") {
        // The title may appear several times in the base name.
        let occurrences = MAP_KEY
            .captures_iter(Path::new(format).file_name().map_or("", |n| n.to_str().unwrap_or("")))
            .filter(|c| c[1].eq_ignore_ascii_case("title"))
            .count()
            .max(1);
        let cut = over.div_ceil(occurrences);
        let keep = title.len().saturating_sub(cut);
        *title = truncate_to_bytes(title, keep).to_string();
    }
    format_python_map(format, &fname_vals)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vals(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn sterilize() {
        assert_eq!(sterilize_filename(r#"a<b>c:d"e/f\g|h?i*j"#, false), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(sterilize_filename("a/b?", true), "a⧸b？");
        assert_eq!(sterilize_filename("日本語 ok", false), "日本語 ok");
    }

    #[test]
    fn truncate() {
        assert_eq!(truncate_to_bytes("hello", 10), "hello");
        assert_eq!(truncate_to_bytes("hello", 3), "hel");
        // "日" is 3 bytes; never split it.
        assert_eq!(truncate_to_bytes("日本", 4), "日");
        assert_eq!(truncate_to_bytes("日本", 2), "");
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
        assert_eq!(
            format_filename("%(title)s-%(id)s%(description)s", &v, false).unwrap(),
            "a_b-x"
        );
        // Directory separators in the template itself are kept.
        assert_eq!(format_filename("dir/%(id)s", &v, false).unwrap(), "dir/x");
    }

    #[test]
    fn filename_truncates_long_title() {
        let title = "あ".repeat(200); // 600 bytes
        let v = vals(&[("title", &title), ("id", "abcdefghijk")]);
        let out = format_filename("out/%(title)s [%(id)s]", &v, false).unwrap();
        let base = Path::new(&out).file_name().unwrap().to_str().unwrap();
        assert!(base.len() <= MAX_FILENAME_LEN, "{}", base.len());
        assert!(base.ends_with(" [abcdefghijk]"));

        let v = vals(&[("title", &title)]);
        let out = format_filename("%(title)s %(title)s", &v, false).unwrap();
        assert!(out.len() <= MAX_FILENAME_LEN, "{}", out.len());
    }
}
