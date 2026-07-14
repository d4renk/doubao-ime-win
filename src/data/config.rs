//! Application Configuration
//!
//! Handles loading and saving application configuration.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub hotkey: HotkeyConfig,
    #[serde(default)]
    pub floating_button: FloatingButtonConfig,
    #[serde(default)]
    pub asr: AsrConfig,
    #[serde(default)]
    pub cloud: CloudConfig,
}

impl AppConfig {
    /// Get the config file path
    pub fn config_path() -> PathBuf {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        exe_dir.join("config.toml")
    }

    /// Get the credentials file path
    pub fn credentials_path() -> PathBuf {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        exe_dir.join("credentials.json")
    }

    /// Load configuration from file or create default
    pub fn load_or_default() -> Result<Self> {
        let path = Self::config_path();

        if path.exists() {
            let content = fs::read_to_string(&path)?;
            let config: AppConfig = toml::from_str(&content)?;
            Ok(config)
        } else {
            let config = AppConfig::default();
            config.save()?;
            Ok(config)
        }
    }

    /// Save configuration to file
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        let content = toml::to_string_pretty(self)?;
        fs::write(&path, content)?;
        Ok(())
    }
}

/// General configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default = "default_language")]
    pub language: String,
}

fn default_language() -> String {
    "zh-CN".to_string()
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            auto_start: false,
            language: default_language(),
        }
    }
}

/// Hotkey configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeyConfig {
    /// Active binding source: `standard` uses the existing global-hotkey
    /// implementation, while `raw` listens to a Windows keyboard event.
    #[serde(default = "default_hotkey_binding")]
    pub binding: String,
    #[serde(default = "default_hotkey_mode")]
    pub mode: String,
    #[serde(default = "default_combo_key")]
    pub combo_key: String,
    #[serde(default = "default_double_tap_key")]
    pub double_tap_key: String,
    #[serde(default = "default_double_tap_interval")]
    pub double_tap_interval: u64,
    /// Windows virtual-key code captured for a raw binding.
    #[serde(default)]
    pub raw_vk_code: u32,
    /// Windows scan code captured for a raw binding.
    #[serde(default)]
    pub raw_scan_code: u32,
    /// Whether the captured raw key has the extended-key flag.
    #[serde(default)]
    pub raw_extended: bool,
}

fn default_hotkey_binding() -> String {
    "standard".to_string()
}

fn default_hotkey_mode() -> String {
    "combo".to_string()
}

fn default_combo_key() -> String {
    "Ctrl+Shift+V".to_string()
}

fn default_double_tap_key() -> String {
    "Ctrl".to_string()
}

fn default_double_tap_interval() -> u64 {
    300
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            binding: default_hotkey_binding(),
            mode: default_hotkey_mode(),
            combo_key: default_combo_key(),
            double_tap_key: default_double_tap_key(),
            double_tap_interval: default_double_tap_interval(),
            raw_vk_code: 0,
            raw_scan_code: 0,
            raw_extended: false,
        }
    }
}

/// Floating button configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FloatingButtonConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_position")]
    pub position_x: i32,
    #[serde(default = "default_position")]
    pub position_y: i32,
}

fn default_true() -> bool {
    true
}

fn default_position() -> i32 {
    100
}

impl Default for FloatingButtonConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            position_x: 100,
            position_y: 100,
        }
    }
}

/// ASR configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrConfig {
    #[serde(default = "default_true")]
    pub vad_enabled: bool,
    #[serde(default)]
    pub aec_enabled: bool,
    #[serde(default)]
    pub audio_quality: AudioQuality,
    #[serde(default)]
    pub punctuation_mode: PunctuationMode,
    #[serde(default = "default_end_smooth_window_ms")]
    pub end_smooth_window_ms: u32,
    #[serde(default = "default_post_ratio_gain")]
    pub post_ratio_gain: f32,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            vad_enabled: true,
            aec_enabled: false,
            audio_quality: AudioQuality::default(),
            punctuation_mode: PunctuationMode::default(),
            end_smooth_window_ms: default_end_smooth_window_ms(),
            post_ratio_gain: default_post_ratio_gain(),
        }
    }
}

fn default_end_smooth_window_ms() -> u32 {
    800
}

fn default_post_ratio_gain() -> f32 {
    1.0
}

#[derive(Debug, Clone, Copy)]
pub struct AudioProcessingConfig {
    pub vad_enabled: bool,
    pub aec_enabled: bool,
    pub end_smooth_window_ms: u32,
    pub post_ratio_gain: f32,
}

impl From<&AsrConfig> for AudioProcessingConfig {
    fn from(config: &AsrConfig) -> Self {
        let post_ratio_gain = if config.post_ratio_gain.is_finite() {
            config.post_ratio_gain.clamp(0.25, 4.0)
        } else {
            default_post_ratio_gain()
        };
        Self {
            vad_enabled: config.vad_enabled,
            aec_enabled: config.aec_enabled,
            end_smooth_window_ms: config.end_smooth_window_ms.min(3_000),
            post_ratio_gain,
        }
    }
}

/// Optional cloud processing applied around voice input sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudConfig {
    /// Send final ASR text to NER for future context and candidate improvement.
    #[serde(default = "default_true")]
    pub ner_enabled: bool,
    /// Remove filler speech after a voice session and auto-replace on success.
    #[serde(default = "default_true")]
    pub auto_polish_enabled: bool,
    /// Include text surrounding the target caret as LLM correction context.
    #[serde(default)]
    pub llm_context_enabled: bool,
    /// Explicitly select the custom OpenAI-compatible backend. `None` keeps
    /// compatibility with older configs, where a non-empty URL enabled it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_custom_api_enabled: Option<bool>,
    /// OpenAI-compatible Chat Completions URL. Empty keeps the built-in service.
    #[serde(default)]
    pub llm_base_url: String,
    /// Bearer token for the custom OpenAI-compatible service.
    #[serde(default)]
    pub llm_api_key: String,
    /// Model name used with the custom OpenAI-compatible service.
    #[serde(default)]
    pub llm_model: String,
    /// Optional replacement prompt for voice correction. Empty uses the built-in prompt.
    #[serde(default)]
    pub llm_prompt: String,
    /// Optional provider extension: `omit`, `disabled`, or `enabled`.
    #[serde(default = "default_llm_thinking_mode")]
    pub llm_thinking_mode: String,
    /// Optional OpenAI-compatible reasoning effort, such as `low`, `medium`, or `high`.
    #[serde(default)]
    pub llm_reasoning_effort: String,
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            ner_enabled: true,
            auto_polish_enabled: true,
            llm_context_enabled: false,
            llm_custom_api_enabled: Some(false),
            llm_base_url: String::new(),
            llm_api_key: String::new(),
            llm_model: String::new(),
            llm_prompt: String::new(),
            llm_thinking_mode: default_llm_thinking_mode(),
            llm_reasoning_effort: String::new(),
        }
    }
}

impl CloudConfig {
    /// Resolve the backend while preserving configs written before the
    /// explicit custom-API switch was introduced.
    pub fn custom_api_enabled(&self) -> bool {
        self.llm_custom_api_enabled
            .unwrap_or_else(|| !self.llm_base_url.trim().is_empty())
    }
}

fn default_llm_thinking_mode() -> String {
    "omit".to_string()
}

/// Audio format sent to the ASR service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AudioQuality {
    /// Official-compatible 16kHz mono Opus.
    #[default]
    Standard,
    /// Experimental 24kHz mono Opus; some ASR routes are less accurate.
    HighQuality,
}

impl AudioQuality {
    pub const fn sample_rate(self) -> u32 {
        match self {
            Self::Standard => 16_000,
            Self::HighQuality => 24_000,
        }
    }

    pub const fn channels(self) -> u16 {
        1
    }
}

/// Client-side punctuation display behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PunctuationMode {
    #[default]
    Smart,
    Spaces,
    NoSentenceFinal,
    Preserve,
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, AudioProcessingConfig, AudioQuality, PunctuationMode};

    #[test]
    fn legacy_config_uses_new_asr_defaults() {
        let config: AppConfig = toml::from_str(
            r#"
                [asr]
                vad_enabled = true
            "#,
        )
        .unwrap();

        assert_eq!(config.asr.audio_quality, AudioQuality::Standard);
        assert!(!config.asr.aec_enabled);
        assert_eq!(config.asr.audio_quality.sample_rate(), 16_000);
        assert_eq!(config.asr.punctuation_mode, PunctuationMode::Smart);
        assert_eq!(config.asr.end_smooth_window_ms, 800);
        assert_eq!(config.asr.post_ratio_gain, 1.0);
        assert!(config.cloud.ner_enabled);
        assert!(config.cloud.auto_polish_enabled);
        assert!(!config.cloud.llm_context_enabled);
        assert!(!config.cloud.custom_api_enabled());
        assert!(config.cloud.llm_prompt.is_empty());
        assert_eq!(config.cloud.llm_thinking_mode, "omit");
    }

    #[test]
    fn legacy_raw_trigger_is_ignored_without_losing_vendor_key() {
        let config: AppConfig = toml::from_str(
            r#"
                [hotkey]
                binding = "raw"
                raw_vk_code = 183
                raw_scan_code = 110
                raw_extended = true
                raw_trigger = "hold"
            "#,
        )
        .unwrap();

        assert_eq!(config.hotkey.raw_vk_code, 183);
        assert_eq!(config.hotkey.raw_scan_code, 110);
        assert!(config.hotkey.raw_extended);
        assert!(!toml::to_string(&config).unwrap().contains("raw_trigger"));
    }

    #[test]
    fn audio_quality_sample_rates_are_stable() {
        assert_eq!(AudioQuality::Standard.sample_rate(), 16_000);
        assert_eq!(AudioQuality::HighQuality.sample_rate(), 24_000);
    }

    #[test]
    fn partial_cloud_config_uses_enabled_defaults() {
        let config: AppConfig = toml::from_str(
            r#"
                [cloud]
                ner_enabled = false
            "#,
        )
        .unwrap();

        assert!(!config.cloud.ner_enabled);
        assert!(config.cloud.auto_polish_enabled);
        assert!(!config.cloud.llm_context_enabled);
        assert!(!config.cloud.custom_api_enabled());
        assert!(config.cloud.llm_prompt.is_empty());
        assert_eq!(config.cloud.llm_thinking_mode, "omit");
    }

    #[test]
    fn asr_options_round_trip() {
        let mut config = AppConfig::default();
        config.asr.audio_quality = AudioQuality::Standard;
        config.asr.aec_enabled = true;
        config.asr.punctuation_mode = PunctuationMode::Preserve;
        config.asr.end_smooth_window_ms = 1_200;
        config.asr.post_ratio_gain = 1.25;
        config.cloud.ner_enabled = false;
        config.cloud.auto_polish_enabled = false;
        config.cloud.llm_context_enabled = true;
        config.cloud.llm_custom_api_enabled = Some(true);
        config.cloud.llm_base_url = "https://example.com/v1/chat/completions".to_string();
        config.cloud.llm_api_key = "secret".to_string();
        config.cloud.llm_model = "example-model".to_string();
        config.cloud.llm_prompt = "只删除口头语。".to_string();
        config.cloud.llm_thinking_mode = "enabled".to_string();
        config.cloud.llm_reasoning_effort = "high".to_string();

        let serialized = toml::to_string(&config).unwrap();
        let restored: AppConfig = toml::from_str(&serialized).unwrap();

        assert_eq!(restored.asr.audio_quality, AudioQuality::Standard);
        assert!(restored.asr.aec_enabled);
        assert_eq!(restored.asr.punctuation_mode, PunctuationMode::Preserve);
        assert_eq!(restored.asr.end_smooth_window_ms, 1_200);
        assert_eq!(restored.asr.post_ratio_gain, 1.25);
        assert!(!restored.cloud.ner_enabled);
        assert!(!restored.cloud.auto_polish_enabled);
        assert!(restored.cloud.llm_context_enabled);
        assert!(restored.cloud.custom_api_enabled());
        assert_eq!(
            restored.cloud.llm_base_url,
            "https://example.com/v1/chat/completions"
        );
        assert_eq!(restored.cloud.llm_api_key, "secret");
        assert_eq!(restored.cloud.llm_model, "example-model");
        assert_eq!(restored.cloud.llm_prompt, "只删除口头语。");
        assert_eq!(restored.cloud.llm_thinking_mode, "enabled");
        assert_eq!(restored.cloud.llm_reasoning_effort, "high");
    }

    #[test]
    fn legacy_custom_url_enables_custom_api_but_explicit_switch_wins() {
        let legacy: AppConfig = toml::from_str(
            r#"
                [cloud]
                llm_base_url = "https://example.com/v1/chat/completions"
            "#,
        )
        .unwrap();
        assert_eq!(legacy.cloud.llm_custom_api_enabled, None);
        assert!(legacy.cloud.custom_api_enabled());

        let explicitly_builtin: AppConfig = toml::from_str(
            r#"
                [cloud]
                llm_custom_api_enabled = false
                llm_base_url = "https://example.com/v1/chat/completions"
            "#,
        )
        .unwrap();
        assert!(!explicitly_builtin.cloud.custom_api_enabled());
    }

    #[test]
    fn audio_processing_runtime_values_are_bounded() {
        let mut config = AppConfig::default();
        config.asr.end_smooth_window_ms = 20_000;
        config.asr.post_ratio_gain = 10.0;
        let processing = AudioProcessingConfig::from(&config.asr);
        assert_eq!(processing.end_smooth_window_ms, 3_000);
        assert_eq!(processing.post_ratio_gain, 4.0);

        config.asr.post_ratio_gain = f32::NAN;
        let processing = AudioProcessingConfig::from(&config.asr);
        assert_eq!(processing.post_ratio_gain, 1.0);
    }
}
