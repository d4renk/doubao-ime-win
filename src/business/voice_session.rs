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
