//! Stamps the version string: `v0.1.0 (2026-11-07)` for releases (tag from
//! `YTARCHIVE_VERSION`), else `v0.1.0+abc1234 (2026-11-07)`. The commit is
//! SemVer build metadata (`+`), so the build still compares equal to its
//! version; the date is the commit's, so rebuilds give the same string.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}

fn main() {
    for path in ["HEAD", "refs/heads", "packed-refs"] {
        if let Some(git_path) = git(&["rev-parse", "--git-path", path])
            && Path::new(&git_path).exists()
        {
            println!("cargo:rerun-if-changed={git_path}");
        }
    }
    println!("cargo:rerun-if-env-changed=YTARCHIVE_VERSION");

    let date = git(&["log", "-1", "--format=%cs"]).unwrap_or_else(|| "unknown date".into());
    let full = match std::env::var("YTARCHIVE_VERSION").ok().filter(|tag| !tag.trim().is_empty()) {
        Some(tag) => format!("{tag} ({date})"),
        None => {
            let commit = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".into());
            format!("v{}+{commit} ({date})", env!("CARGO_PKG_VERSION"))
        }
    };
    println!("cargo:rustc-env=YTARCHIVE_VERSION_STRING={full}");
}
