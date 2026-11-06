//! Console/OS setup for the duration of the program (port of Go's
//! util_windows.go). No-op on Linux and macOS.
//!
//! On Windows: keep the system from sleeping while running, disable the
//! console's QuickEdit mode (a stray click would otherwise pause output and
//! stall the download), and enable ANSI colors. Everything is restored when
//! the guard is dropped.

pub struct ConsoleGuard {
    #[cfg(windows)]
    restore: Vec<(windows_sys::Win32::Foundation::HANDLE, u32)>,
}

#[cfg(not(windows))]
pub fn setup() -> ConsoleGuard {
    ConsoleGuard {}
}

#[cfg(windows)]
pub fn setup() -> ConsoleGuard {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        ENABLE_EXTENDED_FLAGS, ENABLE_QUICK_EDIT_MODE, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        GetConsoleMode, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        SetConsoleMode,
    };
    use windows_sys::Win32::System::Power::{
        ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };

    let mut restore = Vec::new();
    // SAFETY: plain Win32 calls with handles from GetStdHandle; failures
    // (e.g. output redirected to a file) are detected and skipped.
    unsafe {
        // Applies to the calling thread, which is the main thread for the
        // whole run.
        SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);

        let mut adjust = |which, set: u32, clear: u32| {
            let h = GetStdHandle(which);
            if h.is_null() || h == INVALID_HANDLE_VALUE {
                return;
            }
            let mut mode = 0u32;
            if GetConsoleMode(h, &mut mode) == 0 {
                return;
            }
            if SetConsoleMode(h, (mode | set) & !clear) != 0 {
                restore.push((h, mode));
            }
        };
        // QuickEdit can only be changed together with ENABLE_EXTENDED_FLAGS
        // (Go omitted it, so its change had no effect).
        adjust(STD_INPUT_HANDLE, ENABLE_EXTENDED_FLAGS, ENABLE_QUICK_EDIT_MODE);
        adjust(STD_OUTPUT_HANDLE, ENABLE_VIRTUAL_TERMINAL_PROCESSING, 0);
        adjust(STD_ERROR_HANDLE, ENABLE_VIRTUAL_TERMINAL_PROCESSING, 0);
    }
    ConsoleGuard { restore }
}

#[cfg(windows)]
impl Drop for ConsoleGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Console::SetConsoleMode;
        use windows_sys::Win32::System::Power::{ES_CONTINUOUS, SetThreadExecutionState};
        // SAFETY: restores modes on handles that were valid at setup.
        unsafe {
            for (h, mode) in &self.restore {
                SetConsoleMode(*h, *mode);
            }
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
}
