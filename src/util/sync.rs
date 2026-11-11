//! Locking that can't panic, and a sleep that can be cancelled.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// Sleeps unless cancelled first. Returns `false` if cancelled.
pub async fn sleep_or_cancel(cancel: &CancellationToken, dur: Duration) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(dur) => true,
    }
}

/// Locks `mutex` even if a thread panicked while holding it. The values
/// guarded here (counters, URL lists, timestamps) stay usable after a panic,
/// and one failed task shouldn't take the whole download down with it.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
