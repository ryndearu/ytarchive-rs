//! Ctrl-C handling for the whole program: the first interrupt stops the
//! current download gracefully (merge/save prompts follow); a second one
//! exits immediately, like Go's `signal.Reset` after the first.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio_util::sync::CancellationToken;

use crate::log_warn;

/// The signals that count as an interrupt. On Windows that is Ctrl-C and
/// Ctrl-Break, both of which Go's `os.Interrupt` catches.
struct Signals {
    #[cfg(windows)]
    ctrl_c: tokio::signal::windows::CtrlC,
    #[cfg(windows)]
    ctrl_break: tokio::signal::windows::CtrlBreak,
}

impl Signals {
    fn new() -> std::io::Result<Self> {
        Ok(Self {
            #[cfg(windows)]
            ctrl_c: tokio::signal::windows::ctrl_c()?,
            #[cfg(windows)]
            ctrl_break: tokio::signal::windows::ctrl_break()?,
        })
    }

    /// Waits for the next interrupt; `false` if signals can't be received.
    #[cfg(windows)]
    async fn recv(&mut self) -> bool {
        tokio::select! {
            r = self.ctrl_c.recv() => r.is_some(),
            r = self.ctrl_break.recv() => r.is_some(),
        }
    }

    #[cfg(not(windows))]
    async fn recv(&mut self) -> bool {
        tokio::signal::ctrl_c().await.is_ok()
    }
}

/// Waits for one interrupt (used by prompts); never returns if signals
/// can't be received.
pub async fn next() {
    if let Ok(mut s) = Signals::new()
        && s.recv().await
    {
        return;
    }
    std::future::pending().await
}

pub struct Interrupt {
    flag: AtomicBool,
    token: CancellationToken,
}

impl Interrupt {
    pub fn install() -> Arc<Self> {
        let me = Arc::new(Self { flag: AtomicBool::new(false), token: CancellationToken::new() });
        let m = me.clone();
        tokio::spawn(async move {
            let Ok(mut signals) = Signals::new() else { return };
            if !signals.recv().await {
                return;
            }
            m.flag.store(true, Ordering::SeqCst);
            eprintln!();
            log_warn!("User Interrupt, Stopping download...");
            m.token.cancel();
            if signals.recv().await {
                eprintln!("\nExiting...");
                crate::platform::restore();
                crate::ytdlp_cookies::remove();
                std::process::exit(130);
            }
        });
        me
    }

    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// A token for one run, cancelled on interrupt.
    pub fn child_token(&self) -> CancellationToken {
        self.token.child_token()
    }

    /// Waits for `dur` unless interrupted first. Returns `false` if interrupted.
    pub async fn sleep(&self, dur: std::time::Duration) -> bool {
        tokio::select! {
            _ = self.token.cancelled() => false,
            _ = tokio::time::sleep(dur) => true,
        }
    }
}
