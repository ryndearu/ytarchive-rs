//! Rewriting Go-style flags for clap, and splitting `--ytdlp-opts`.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::mem;

use clap::CommandFactory;

use super::Cli;
use crate::error::{Error, Result};

/// Splits `--ytdlp-opts` respecting single and double quotes while keeping
/// Windows backslashes intact unless escaping quotes.
pub(super) fn split_ytdlp_opts(input: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut in_token = false;

    while let Some(character) = chars.next() {
        if in_single {
            if character == '\'' {
                in_single = false;
            } else {
                current.push(character);
            }
        } else if in_double {
            match character {
                '"' => in_double = false,
                '\\' => {
                    let mut backslashes = 1usize;
                    while chars.peek() == Some(&'\\') {
                        chars.next();
                        backslashes += 1;
                    }
                    if chars.peek() == Some(&'"') {
                        for _ in 0..(backslashes / 2) {
                            current.push('\\');
                        }
                        if backslashes % 2 == 1 {
                            chars.next();
                            current.push('"');
                        }
                    } else {
                        for _ in 0..backslashes {
                            current.push('\\');
                        }
                    }
                }
                _ => current.push(character),
            }
        } else {
            match character {
                value if value.is_whitespace() => {
                    if in_token {
                        out.push(mem::take(&mut current));
                        in_token = false;
                    }
                }
                '\'' => {
                    in_single = true;
                    in_token = true;
                }
                '"' => {
                    in_double = true;
                    in_token = true;
                }
                '\\' => {
                    in_token = true;
                    if let Some(&next) = chars.peek()
                        && (next == '"' || next == '\'')
                    {
                        chars.next();
                        current.push(next);
                    } else {
                        current.push('\\');
                    }
                }
                _ => {
                    in_token = true;
                    current.push(character);
                }
            }
        }
    }

    if in_single || in_double {
        return Err(Error::InvalidArg("--ytdlp-opts has an unclosed quote".into()));
    }
    if in_token {
        out.push(current);
    }
    Ok(out)
}

/// Go flag aliases that are multi-character but single-dash.
const GO_SHORT_ALIASES: &[(&str, &str)] =
    &[("td", "temporary-dir"), ("dp", "directory-permissions"), ("fp", "file-permissions")];

/// Rewrites Go-style single-dash long options (`-threads 4`, `-td DIR`) to
/// `--flag` so clap accepts them. Values following a value-taking flag are
/// left untouched.
pub fn normalize_args<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = OsString>,
{
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

    let mut skip_next = false;
    let mut passthrough = false;
    for arg in iter {
        if passthrough || skip_next {
            skip_next = false;
            out.push(arg);
            continue;
        }
        let Some(text) = arg.to_str() else {
            out.push(arg);
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
                    let suffix = rest.strip_prefix(name).unwrap_or("");
                    (Some(format!("--{long}{suffix}")), Some(long.to_string()))
                }
                None => (None, None),
            }
        } else if let Some(character) = text.strip_prefix('-').and_then(|rest| rest.chars().next()) {
            // Plain single-char short flag like `-o`.
            if short_takes_value.contains(&character) && text.len() == 2 {
                skip_next = true;
            }
            (None, None)
        } else {
            (None, None)
        };

        let final_arg = rewritten.unwrap_or_else(|| text.to_string());
        if let Some(name) = name {
            let has_inline_value = final_arg.contains('=');
            if !has_inline_value && long_takes_value.get(&name).copied().unwrap_or(false) {
                skip_next = true;
            }
        }
        out.push(OsString::from(final_arg));
    }
    out
}
