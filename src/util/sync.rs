//! Locking that can't panic.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks `mutex` even if a thread panicked while holding it. The values
/// guarded here (counters, URL lists, timestamps) stay usable after a panic,
/// and one failed task shouldn't take the whole download down with it.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
