//! A cheap, cloneable cancellation handle, analogous to pi's `AbortSignal`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

/// A cloneable cancellation flag that tools can observe.
///
/// Clones share one flag, so cancelling any clone cancels them all.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl CancelToken {
    /// Create a fresh, un-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancel every clone of this token and wake anyone awaiting [`cancelled`].
    ///
    /// [`cancelled`]: CancelToken::cancelled
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Clear the cancellation flag so the token can be reused for a new run.
    pub fn reset(&self) {
        self.flag.store(false, Ordering::SeqCst);
    }

    /// Resolve once cancellation is requested. Returns immediately if already cancelled.
    pub async fn cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}
