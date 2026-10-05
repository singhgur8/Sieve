//! Cooperative cancellation of superseded preview renders (Phase 8d).
//!
//! `DevelopCache::render` runs the pipeline inside [`scope`]; row loops of the *uncached* render stages
//! capture [`Cancel::current`] on the calling thread and return early when [`Cancel::is_set`] (a newer
//! ticket for the same image / slot exists). A cancelled render leaves garbage pixels behind, so the caller
//! must discard the output (it does: `is_current` is re-checked right after) and nothing cached may be
//! computed inside a scope.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

type Token = (Arc<AtomicU32>, u32);

#[derive(Clone)]
pub struct Cancel(Option<Token>);

thread_local! {
    static CURRENT: RefCell<Option<Token>> = const { RefCell::new(None) };
}

impl Cancel {
    /// The scope of the calling thread (a no-op token outside [`scope`], e.g. exports).
    pub fn current() -> Self {
        Cancel(CURRENT.with(|c| c.borrow().clone()))
    }

    /// True once a newer ticket superseded the render running in the captured scope.
    #[inline]
    pub fn is_set(&self) -> bool {
        match &self.0 {
            Some((latest, seq)) => latest.load(Ordering::Relaxed) != *seq,
            None => false,
        }
    }
}

/// Runs `f` with `latest != seq` as the cancel condition for the row loops it starts on this thread.
pub fn scope<R>(latest: Arc<AtomicU32>, seq: u32, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Token>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|c| *c.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(CURRENT.with(|c| c.borrow_mut().replace((latest, seq))));
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_follows_the_latest_seq() {
        let latest = Arc::new(AtomicU32::new(1));
        assert!(!Cancel::current().is_set());
        scope(latest.clone(), 1, || {
            let c = Cancel::current();
            assert!(!c.is_set());
            latest.store(2, Ordering::Relaxed);
            assert!(c.is_set());
        });
        assert!(!Cancel::current().is_set());
    }
}
