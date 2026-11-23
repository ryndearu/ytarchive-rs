//! Signal handling for the whole program: the first interrupt (`SIGINT` /
//! `SIGTERM` on Unix, Ctrl-C / Ctrl-Break on Windows) stops the current
//! download gracefully; a second one exits immediately, like Go's
//! `signal.Reset`. Prompts exit immediately on the first signal.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use tokio_util::sync::CancellationToken;

use crate::log_warn;

/// True while waiting on an interactive stdin prompt, so an interrupt exits
/// immediately without a competing signal listener.
static PROMPTING: AtomicBool = AtomicBool::new(false);

pub struct PromptGuard;

impl Drop for PromptGuard {
    fn drop(&mut self) {
        PROMPTING.store(false, Ordering::SeqCst);
    }
}

pub fn enter_prompt() -> PromptGuard {
    PROMPTING.store(true, Ordering::SeqCst);
    PromptGuard
}

/// The signals that count as an interrupt.
struct Signals {
    #[cfg(windows)]
    ctrl_c: tokio::signal::windows::CtrlC,
    #[cfg(windows)]
    ctrl_break: tokio::signal::windows::CtrlBreak,
    #[cfg(unix)]
    sigint: tokio::signal::unix::Signal,
    #[cfg(unix)]
    sigterm: tokio::signal::unix::Signal,
}

impl Signals {
    fn new() -> std::io::Result<Self> {
        #[cfg(unix)]
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            #[cfg(windows)]
            ctrl_c: tokio::signal::windows::ctrl_c()?,
            #[cfg(windows)]
            ctrl_break: tokio::signal::windows::ctrl_break()?,
            #[cfg(unix)]
            sigint: signal(SignalKind::interrupt())?,
            #[cfg(unix)]
            sigterm: signal(SignalKind::terminate())?,
        })
    }

    /// Waits for the next interrupt and returns its exit code (130 or 143).
    #[cfg(windows)]
    async fn recv(&mut self) -> Option<i32> {
        tokio::select! {
            signal = self.ctrl_c.recv() => signal.map(|()| 130),
            signal = self.ctrl_break.recv() => signal.map(|()| 130),
        }
    }

    #[cfg(unix)]
    async fn recv(&mut self) -> Option<i32> {
        tokio::select! {
            signal = self.sigint.recv() => signal.map(|()| 130),
            signal = self.sigterm.recv() => signal.map(|()| 143),
        }
    }

    #[cfg(not(any(windows, unix)))]
    async fn recv(&mut self) -> Option<i32> {
        tokio::signal::ctrl_c().await.ok().map(|()| 130)
    }
}

pub struct Interrupt {
    signal: AtomicI32,
    token: CancellationToken,
}

impl Interrupt {
    pub fn install() -> Arc<Self> {
        let installed = Arc::new(Self { signal: AtomicI32::new(0), token: CancellationToken::new() });
        tokio::spawn({
            let interrupt = installed.clone();
            async move {
                let Ok(mut signals) = Signals::new() else { return };
                while let Some(code) = signals.recv().await {
                    let already_interrupted =
                        interrupt.signal.compare_exchange(0, code, Ordering::SeqCst, Ordering::SeqCst).is_err();
                    if PROMPTING.load(Ordering::SeqCst) || already_interrupted {
                        let exit_code = if already_interrupted || code == 143 { code } else { 1 };
                        eprintln!("\nExiting...");
                        crate::platform::restore();
                        crate::ytdlp_cookies::remove();
                        std::process::exit(exit_code);
                    }
                    eprintln!();
                    if code == 143 {
                        log_warn!("Termination signal received, Stopping download...");
                    } else {
                        log_warn!("User Interrupt, Stopping download...");
                    }
                    interrupt.token.cancel();
                }
            }
        });
        installed
    }

    pub fn is_set(&self) -> bool {
        self.signal.load(Ordering::SeqCst) != 0
    }

    pub fn terminated(&self) -> bool {
        self.signal.load(Ordering::SeqCst) == 143
    }

    /// A token for one run, cancelled on interrupt.
    pub fn child_token(&self) -> CancellationToken {
        self.token.child_token()
    }

    /// Waits for `dur` unless interrupted first. Returns `false` if interrupted.
    pub async fn sleep(&self, dur: std::time::Duration) -> bool {
        crate::util::sleep_or_cancel(&self.token, dur).await
    }
}
