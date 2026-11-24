//! Rewriting Go-style flags for clap while preserving raw OS bytes for path
//! flags (`--cookies`, `--temporary-dir`).

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};

use clap::CommandFactory;

use super::Cli;

/// Go flag aliases that are multi-character but single-dash.
const GO_SHORT_ALIASES: &[(&str, &str)] =
    &[("td", "temporary-dir"), ("dp", "directory-permissions"), ("fp", "file-permissions")];

/// Value-taking flags whose values are `PathBuf`s and must keep raw OS bytes.
const PATH_LONG_FLAGS: &[&str] = &["cookies", "temporary-dir"];
const PATH_SHORT_FLAGS: &[char] = &['c'];

fn lossy_os(arg: OsString) -> OsString {
    match arg.into_string() {
        Ok(text) => OsString::from(text),
        Err(raw) => OsString::from(raw.to_string_lossy().into_owned()),
    }
}

#[cfg(unix)]
fn rewrite_single_dash_path_flag(arg: &OsStr) -> Option<OsString> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = arg.as_bytes();
    for (short, long) in [
        (&b"-td="[..], &b"--temporary-dir="[..]),
        (&b"-temporary-dir="[..], &b"--temporary-dir="[..]),
        (&b"-cookies="[..], &b"--cookies="[..]),
    ] {
        if let Some(rest) = bytes.strip_prefix(short) {
            let mut rebuilt = OsString::from(OsStr::from_bytes(long));
            rebuilt.push(OsStr::from_bytes(rest));
            return Some(rebuilt);
        }
    }
    None
}

#[cfg(windows)]
fn rewrite_single_dash_path_flag(arg: &OsStr) -> Option<OsString> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let units: Vec<u16> = arg.encode_wide().collect();
    for (short, long) in
        [("-td=", "--temporary-dir="), ("-temporary-dir=", "--temporary-dir="), ("-cookies=", "--cookies=")]
    {
        let prefix: Vec<u16> = short.encode_utf16().collect();
        if let Some(rest) = units.strip_prefix(prefix.as_slice()) {
            let mut rebuilt = OsString::from(long);
            rebuilt.push(OsString::from_wide(rest));
            return Some(rebuilt);
        }
    }
    None
}

/// Preserves raw OS bytes for inline path flags (`--cookies=...`, `-c...`,
/// `--temporary-dir=...`, and single-dash Go forms like `-td=...` or `-cookies=...`).
fn preserve_inline_path_flag(arg: &OsStr) -> Option<OsString> {
    let bytes = arg.as_encoded_bytes();
    if bytes.starts_with(b"--cookies=") || bytes.starts_with(b"--temporary-dir=") {
        return Some(arg.to_os_string());
    }
    if let Some(rewritten) = rewrite_single_dash_path_flag(arg) {
        return Some(rewritten);
    }
    if bytes.starts_with(b"-c") && bytes.len() > 2 {
        return Some(arg.to_os_string());
    }
    None
}

/// Rewrites Go-style single-dash long options (`-threads 4`, `-td DIR`) to
/// `--flag` so clap accepts them. Non-UTF-8 values are preserved for `PathBuf`
/// flags (`--cookies`, `--temporary-dir`) and converted lossily for text flags
/// and positionals so clap does not reject legacy encodings.
pub fn normalize_args<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = OsString>,
{
    enum NextValue {
        None,
        Lossy,
        RawPath,
    }

    let cmd = Cli::command();
    let mut long_takes_value = HashMap::new();
    let mut short_takes_value = HashSet::new();
    for arg in cmd.get_arguments() {
        let takes = arg.get_action().takes_values();
        if let Some(line) = arg.get_long() {
            long_takes_value.insert(line.to_string(), takes);
        }
        if let Some(character) = arg.get_short()
            && takes
        {
            short_takes_value.insert(character);
        }
    }

    let mut out = Vec::new();
    let mut iter = args.into_iter();
    if let Some(prog) = iter.next() {
        out.push(prog);
    }

    let mut next_value = NextValue::None;
    let mut passthrough = false;
    for arg in iter {
        if passthrough {
            out.push(lossy_os(arg));
            continue;
        }
        match next_value {
            NextValue::RawPath => {
                next_value = NextValue::None;
                out.push(arg);
                continue;
            }
            NextValue::Lossy => {
                next_value = NextValue::None;
                out.push(lossy_os(arg));
                continue;
            }
            NextValue::None => {}
        }
        let Some(text) = arg.to_str() else {
            out.push(preserve_inline_path_flag(&arg).unwrap_or_else(|| lossy_os(arg)));
            continue;
        };
        if text == "--" {
            passthrough = true;
            out.push(arg);
            continue;
        }

        let (rewritten, name) = if let Some(rest) = text.strip_prefix("--") {
            (None, Some(rest.split('=').next().unwrap_or(rest).to_string()))
        } else if let Some(rest) = text.strip_prefix('-').filter(|stripped| stripped.len() > 1) {
            let name = rest.split('=').next().unwrap_or(rest);
            let mapped = GO_SHORT_ALIASES
                .iter()
                .find(|(arg, _)| *arg == name)
                .map(|(_, line)| *line)
                .or_else(|| long_takes_value.contains_key(name).then_some(name));
            match mapped {
                Some(long) => {
                    let suffix = &rest[name.len()..];
                    (Some(format!("--{long}{suffix}")), Some(long.to_string()))
                }
                None => (None, None),
            }
        } else if let Some(character) = text.strip_prefix('-').and_then(|rest| rest.chars().next()) {
            // Plain single-char short flag like `-o` or `-c`.
            if short_takes_value.contains(&character) && text.len() == 2 {
                next_value = if PATH_SHORT_FLAGS.contains(&character) { NextValue::RawPath } else { NextValue::Lossy };
            }
            (None, None)
        } else {
            (None, None)
        };

        let final_arg = rewritten.unwrap_or_else(|| text.to_string());
        if let Some(name) = name {
            let has_inline_value = final_arg.contains('=');
            if !has_inline_value && long_takes_value.get(&name).copied().unwrap_or(false) {
                next_value =
                    if PATH_LONG_FLAGS.contains(&name.as_str()) { NextValue::RawPath } else { NextValue::Lossy };
            }
        }
        out.push(OsString::from(final_arg));
    }
    out
}
