//! Data module for configuration and credential management

mod config;
mod credential;

pub use config::{
    AppConfig, AsrConfig, AudioProcessingConfig, AudioQuality, CloudConfig, FloatingButtonConfig,
    GeneralConfig, HotkeyConfig, PunctuationMode,
};
pub use credential::CredentialStore;
