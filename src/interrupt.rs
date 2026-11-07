//! Ctrl-C handling for the whole program: the first interrupt stops the
//! current download gracefully (merge/save prompts follow); a second one
//! exits immediately, like Go's `signal.Reset` after the first.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::log_warn;

pub struct Interrupt {
    flag: AtomicBool,
    token: CancellationToken,
}

impl Interrupt {
    pub fn install() -> Arc<Self> {
        let me = Arc::new(Self {
            flag: AtomicBool::new(false),
            token: CancellationToken::new(),
        });
        let m = me.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_err() {
                return;
            }
            m.flag.store(true, Ordering::SeqCst);
            eprintln!();
            log_warn!("User Interrupt, Stopping download...");
            m.token.cancel();
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("\nExiting...");
                crate::platform::restore();
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
