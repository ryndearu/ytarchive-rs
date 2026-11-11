//! Console/OS setup for the duration of the program (port of Go's
//! util_windows.go). No-op outside Windows.
//!
//! On Windows: keep the system from sleeping while running, enable ANSI
//! colors, and turn off the console's QuickEdit mode, where a stray click
//! starts a selection that pauses all output (and with it the download
//! progress) until a key is pressed. All of it goes through safe wrapper
//! crates, so there is no `unsafe` here.

pub struct ConsoleGuard {
    /// Releases the keep-awake request when dropped.
    #[cfg(windows)]
    _awake: Option<keepawake::KeepAwake>,
}

/// Console input mode before QuickEdit was turned off. QuickEdit is a setting
/// of the console window, which outlives this program, so it's restored on
/// exit (by `ConsoleGuard` or, before `process::exit`, by `restore`).
#[cfg(windows)]
static INPUT_MODE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

/// Undoes console changes; call before `std::process::exit`.
pub fn restore() {
    #[cfg(windows)]
    if let Some(&mode) = INPUT_MODE.get()
        && let Err(error) = winapi_util::console::set_mode(std::io::stdin(), mode)
    {
        crate::log_debug!("Could not restore the console mode: {error}");
    }
}

#[cfg(not(windows))]
pub fn setup() -> ConsoleGuard {
    ConsoleGuard {}
}

#[cfg(windows)]
const ENABLE_QUICK_EDIT_MODE: u32 = 0x0040;
/// Must be set along with the QuickEdit change for it to take effect (Go
/// left it out, so its attempt to disable QuickEdit did nothing).
#[cfg(windows)]
const ENABLE_EXTENDED_FLAGS: u32 = 0x0080;

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
    disable_quick_edit();
    ConsoleGuard { _awake: awake }
}

/// Turns QuickEdit off, remembering the previous input mode. Fails quietly
/// when stdin isn't a console (redirected, or no console at all).
#[cfg(windows)]
fn disable_quick_edit() -> Option<()> {
    use winapi_util::console;
    let stdin = std::io::stdin();
    let mode = console::mode(&stdin)
        .inspect_err(|e| crate::log_debug!("Could not read the console mode: {e}"))
        .ok()?;
    console::set_mode(&stdin, (mode | ENABLE_EXTENDED_FLAGS) & !ENABLE_QUICK_EDIT_MODE)
        .inspect_err(|e| crate::log_debug!("Could not disable QuickEdit mode: {e}"))
        .ok()?;
    INPUT_MODE.set(mode).ok()
}

impl Drop for ConsoleGuard {
    fn drop(&mut self) {
        restore();
    }
}
