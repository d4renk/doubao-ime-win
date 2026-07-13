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

/// A completed rewrite that is waiting for explicit user confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolishPresentation {
    pub session: VoiceSessionRecord,
    pub rewritten_text: String,
}

/// Tracks the only voice session that may still be rewritten.
#[derive(Debug, Default)]
pub struct VoiceSessionStore {
    generation: AtomicU64,
    latest: Mutex<Option<VoiceSessionRecord>>,
}

impl VoiceSessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Invalidate all prior work and return the generation for a new recording.
    pub fn begin_session(&self) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self
            .latest
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        generation
    }

    pub fn publish(&self, record: VoiceSessionRecord) -> bool {
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
}

#[cfg(test)]
mod tests {
    use super::{VoiceSessionRecord, VoiceSessionStore};
    use crate::business::TargetWindow;

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
}
