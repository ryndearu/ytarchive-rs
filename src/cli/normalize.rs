//! Go-style single-dash long flags (`-threads 4`, `-td DIR`) rewritten to
//! `--flag` so clap accepts them.

use std::ffi::OsString;

use clap::CommandFactory;

use super::Cli;

/// Go flag aliases that are multi-character but single-dash.
const GO_SHORT_ALIASES: &[(&str, &str)] =
    &[("td", "temporary-dir"), ("dp", "directory-permissions"), ("fp", "file-permissions")];

/// Rewrites Go-style `-flag` long options to `--flag` so clap accepts them.
/// Values following a value-taking flag are left untouched.
pub fn normalize_args<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = OsString>,
{
    let cmd = Cli::command();
    let mut long_takes_value = std::collections::HashMap::new();
    let mut short_takes_value = std::collections::HashSet::new();
    for arg in cmd.get_arguments() {
        let takes = arg.get_action().takes_values();
        if let Some(l) = arg.get_long() {
            long_takes_value.insert(l.to_string(), takes);
        }
        if let Some(s) = arg.get_short()
            && takes
        {
            short_takes_value.insert(s);
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
        let Some(s) = arg.to_str() else {
            out.push(arg);
            continue;
        };
        if s == "--" {
            passthrough = true;
            out.push(arg);
            continue;
        }

        let (rewritten, name) = if let Some(rest) = s.strip_prefix("--") {
            (None, Some(rest.split('=').next().unwrap_or(rest).to_string()))
        } else if let Some(rest) = s.strip_prefix('-').filter(|r| r.len() > 1) {
            let name = rest.split('=').next().unwrap_or(rest);
            let mapped = GO_SHORT_ALIASES
                .iter()
                .find(|(a, _)| *a == name)
                .map(|(_, l)| *l)
                .or_else(|| long_takes_value.contains_key(name).then_some(name));
            match mapped {
                Some(long) => {
                    let suffix = &rest[name.len()..];
                    (Some(format!("--{long}{suffix}")), Some(long.to_string()))
                }
                None => (None, None),
            }
        } else if let Some(c) = s.strip_prefix('-').and_then(|r| r.chars().next()) {
            // Plain single-char short flag like `-o`.
            if short_takes_value.contains(&c) && s.len() == 2 {
                skip_next = true;
            }
            (None, None)
        } else {
            (None, None)
        };

        let final_arg = rewritten.unwrap_or_else(|| s.to_string());
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
