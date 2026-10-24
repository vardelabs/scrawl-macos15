//! One warm helper process kept between uses (the command transcriber, the dictation formatter),
//! so the next dictation skips loading the model. A parked process unloads after `PARK_FOR`
//! unused, which frees the memory its model holds.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// How long a parked process waits for its next use before it is dropped.
pub const PARK_FOR: Duration = Duration::from_secs(10 * 60);

pub struct Park<T> {
    slot: Mutex<Option<(u64, T)>>,
    next: AtomicU64,
}

impl<T: Send + 'static> Park<T> {
    pub const fn new() -> Self {
        Self { slot: Mutex::new(None), next: AtomicU64::new(0) }
    }

    /// The parked item, if any. The caller checks that it still fits before using it.
    pub fn take(&self) -> Option<T> {
        self.lock().take().map(|(_, item)| item)
    }

    /// Parks `item` in place of any parked before, and drops it after `idle` unless taken.
    pub fn put(&'static self, item: T, idle: Duration) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let replaced = self.lock().replace((id, item));
        drop(replaced);
        std::thread::spawn(move || {
            std::thread::sleep(idle);
            let mut slot = self.lock();
            let expired = slot.as_ref().is_some_and(|(parked, _)| *parked == id).then(|| slot.take());
            // Dropped after the lock: dropping a process kills and reaps it.
            drop(slot);
            drop(expired);
        });
    }

    fn lock(&self) -> MutexGuard<'_, Option<(u64, T)>> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    struct Counted(Arc<AtomicUsize>);

    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn a_parked_item_is_taken_once_replaced_and_dropped_when_idle() {
        static PARK: Park<Counted> = Park::new();
        let dropped = Arc::new(AtomicUsize::new(0));
        let idle = Duration::from_millis(100);
        PARK.put(Counted(dropped.clone()), idle);
        assert!(PARK.take().is_some());
        assert!(PARK.take().is_none(), "taken once");
        assert_eq!(dropped.load(Ordering::SeqCst), 1, "the taken item dropped here");

        PARK.put(Counted(dropped.clone()), idle);
        PARK.put(Counted(dropped.clone()), idle);
        assert_eq!(dropped.load(Ordering::SeqCst), 2, "a replaced item is dropped");
        std::thread::sleep(idle * 3);
        assert!(PARK.take().is_none(), "unloaded after idle");
        assert_eq!(dropped.load(Ordering::SeqCst), 3);

        PARK.put(Counted(dropped.clone()), idle);
        let taken = PARK.take();
        PARK.put(Counted(dropped.clone()), Duration::from_secs(60));
        std::thread::sleep(idle * 3);
        assert!(PARK.take().is_some(), "an older timer never drops a newer item");
        drop(taken);
    }
}
