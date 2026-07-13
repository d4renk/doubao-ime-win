//! Doubao Voice Input - Windows voice input tool
//!
//! A lightweight voice input tool that uses Doubao ASR for real-time
//! speech recognition and inserts text into the focused window.

pub mod asr;
pub mod audio;
pub mod business;
pub mod cloud;
pub mod data;
pub mod ui;

pub use asr::AsrClient;
pub use audio::AudioCapture;
pub use business::{
    HotkeyManager, PolishPresentation, TextInserter, VoiceController, VoiceSessionStore,
};
pub use cloud::{NerClient, NerLexicon, RichChatClient};
pub use data::{AppConfig, CredentialStore};
