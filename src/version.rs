//! Version string stamped by build.rs: `v0.1.0 (date)` for releases,
//! `v0.1.0-<commit> (date)` for nightly and other builds.

pub const VERSION: &str = env!("YTARCHIVE_VERSION_STRING");

/// The startup line, e.g. `ytarchive v0.1.0-abc1234 (2026-11-07)`.
pub fn banner() -> String {
    format!("ytarchive {VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_has_prefix_and_date() {
        assert!(VERSION.starts_with('v'), "{VERSION}");
        assert!(VERSION.ends_with(')'), "{VERSION}");
        assert!(banner().starts_with("ytarchive v"));
    }
}
