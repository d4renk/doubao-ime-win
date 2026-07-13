//! Data module for configuration and credential management

mod config;
mod credential;

pub use config::{
    AppConfig, AsrConfig, AudioQuality, FloatingButtonConfig, GeneralConfig, HotkeyConfig,
    PunctuationMode,
};
pub use credential::CredentialStore;
