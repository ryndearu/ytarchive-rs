//! Console/OS setup for the duration of the program (port of Go's
//! util_windows.go). No-op outside Windows.
//!
//! On Windows: keep the system from sleeping while running, and enable ANSI
//! colors. Both go through safe wrapper crates, so there is no `unsafe` here.
//! Go also tried to disable the console's QuickEdit mode, but without the
//! required ENABLE_EXTENDED_FLAGS bit it never took effect; it isn't ported.

pub struct ConsoleGuard {
    /// Releases the keep-awake request when dropped.
    #[cfg(windows)]
    _awake: Option<keepawake::KeepAwake>,
}

#[cfg(not(windows))]
pub fn setup() -> ConsoleGuard {
    ConsoleGuard {}
}

#[cfg(windows)]
pub fn setup() -> ConsoleGuard {
    if let Err(e) = enable_ansi_support::enable_ansi_support() {
        crate::log_debug!("Could not enable ANSI colors: {e}");
    }
    // Same as Go's SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED),
    // made from the main thread, which lives for the whole run.
    let awake = keepawake::Builder::default()
        .idle(true)
        .reason("Downloading a livestream")
        .app_name("ytarchive")
        .create()
        .inspect_err(|e| crate::log_debug!("Could not prevent system sleep: {e}"))
        .ok();
    ConsoleGuard { _awake: awake }
}
