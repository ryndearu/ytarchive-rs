//! Stamps the version string shown at startup and by `--version`.
//!
//! - Release builds (`YTARCHIVE_CHANNEL=release`, set by release.yml):
//!   `v0.1.0 (2026-11-07)`, using the tag from `YTARCHIVE_VERSION`.
//! - Everything else (nightly, CI, local): `v0.1.0-abc1234 (2026-11-07)`.
//!
//! The date is the commit date, so rebuilding the same commit gives the same
//! string. CI passes `YTARCHIVE_COMMIT` and `YTARCHIVE_DATE` too, since git
//! isn't usable inside `cross` containers.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

fn env(name: &str) -> Option<String> {
    println!("cargo:rerun-if-env-changed={name}");
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");

    let release = env("YTARCHIVE_CHANNEL").as_deref() == Some("release");
    let version = env("YTARCHIVE_VERSION")
        .unwrap_or_else(|| format!("v{}", std::env::var("CARGO_PKG_VERSION").unwrap_or_default()));
    let commit = env("YTARCHIVE_COMMIT")
        .or_else(|| git(&["rev-parse", "--short=7", "HEAD"]))
        .unwrap_or_else(|| "unknown".into());
    let date = env("YTARCHIVE_DATE")
        .or_else(|| git(&["log", "-1", "--format=%cs"]))
        .unwrap_or_else(|| "unknown date".into());

    let full = if release {
        format!("{version} ({date})")
    } else {
        format!("{version}-{commit} ({date})")
    };
    println!("cargo:rustc-env=YTARCHIVE_VERSION_STRING={full}");
}
