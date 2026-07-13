use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use super::TargetWindow;

/// Stable text and target metadata captured for one completed voice session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceSessionRecord {
    pub generation: u64,
    pub target_window: TargetWindow,
    pub text: String,
    pub preceding_part: String,
    pub follows_below: String,
    pub inserted_chars: usize,
}

/// Tracks the only voice session that may still be rewritten.
#[derive(Debug, Default)]
pub struct VoiceSessionStore {
    generation: AtomicU64,
    latest: Mutex<Option<VoiceSessionRecord>>,
    operation: Mutex<()>,
}

impl VoiceSessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Invalidate all prior work and return the generation for a new recording.
    pub fn begin_session(&self) -> u64 {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self
            .latest
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        generation
    }

    pub fn publish(&self, record: VoiceSessionRecord) -> bool {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !self.is_current(record.generation) {
            return false;
        }
        *self
            .latest
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(record);
        true
    }

    pub fn latest(&self) -> Option<VoiceSessionRecord> {
        self.latest
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
    }

    /// Run an operation only while this generation is current.
    ///
    /// Starting a new session is blocked until `action` completes, so an old
    /// cloud result cannot pass validation and then overwrite a newer session.
    pub fn run_if_current<R>(&self, generation: u64, action: impl FnOnce() -> R) -> Option<R> {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.is_current(generation).then(action)
    }
}

#[cfg(test)]
mod tests {
    use super::{VoiceSessionRecord, VoiceSessionStore};
    use crate::business::TargetWindow;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    fn record(generation: u64) -> VoiceSessionRecord {
        VoiceSessionRecord {
            generation,
            target_window: TargetWindow::from_raw(1),
            text: "voice".to_string(),
            preceding_part: "before".to_string(),
            follows_below: "after".to_string(),
            inserted_chars: 5,
        }
    }

    #[test]
    fn starting_another_recording_invalidates_old_results() {
        let store = VoiceSessionStore::new();
        let first = store.begin_session();
        assert!(store.publish(record(first)));
        assert!(store.latest().is_some());

        let second = store.begin_session();
        assert_ne!(first, second);
        assert!(store.latest().is_none());
        assert!(!store.publish(record(first)));
    }

    #[test]
    fn current_generation_runs_replacement_but_expired_generation_does_not() {
        let store = VoiceSessionStore::new();
        let first = store.begin_session();
        assert_eq!(store.run_if_current(first, || "replaced"), Some("replaced"));

        store.begin_session();
        assert_eq!(store.run_if_current(first, || "wrong target"), None);
    }

    #[test]
    fn new_session_waits_for_current_replacement_to_finish() {
        let store = Arc::new(VoiceSessionStore::new());
        let generation = store.begin_session();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let replacing_store = store.clone();
        let replacement = std::thread::spawn(move || {
            replacing_store.run_if_current(generation, || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
        });
        entered_rx.recv().unwrap();

        let (started_tx, started_rx) = mpsc::channel();
        let starting_store = store.clone();
        let start = std::thread::spawn(move || {
            let next = starting_store.begin_session();
            started_tx.send(next).unwrap();
        });
        assert!(started_rx.recv_timeout(Duration::from_millis(50)).is_err());

        release_tx.send(()).unwrap();
        replacement.join().unwrap();
        assert!(started_rx.recv_timeout(Duration::from_secs(1)).is_ok());
        start.join().unwrap();
    }
}
