//! Business logic module
//!
//! Contains the core business logic for voice input control.

mod hotkey_manager;
mod punctuation;
mod text_inserter;
mod voice_controller;

pub use hotkey_manager::{HotkeyEvent, HotkeyManager, RawKeyBinding};
pub use text_inserter::TextInserter;
pub use voice_controller::VoiceController;
