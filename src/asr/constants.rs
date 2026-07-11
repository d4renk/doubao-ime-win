//! ASR Protocol Constants

/// Device registration API URL
pub const REGISTER_URL: &str = "https://log.snssdk.com/service/2/device_register/";

/// Settings API URL (for getting ASR token)
pub const SETTINGS_URL: &str = "https://is.snssdk.com/service/settings/v3/";

/// ASR WebSocket URL
pub const WEBSOCKET_URL: &str = "wss://frontier-audio-ime-ws.doubao.com/ocean/api/v1/ws";

/// Doubao IME App ID
pub const AID: u32 = 401734;

/// App configuration (Doubao IME)
pub const APP_NAME: &str = "oime";
pub const VERSION_CODE: u32 = 100102018;
pub const VERSION_NAME: &str = "1.1.3";
pub const CHANNEL: &str = "official";
pub const PACKAGE: &str = "com.bytedance.android.doubaoime";

/// Default device configuration (China-market Xiaomi 14, HyperOS / Android 14)
pub const DEVICE_PLATFORM: &str = "android";
pub const OS: &str = "android";
pub const OS_API: &str = "34";
pub const OS_VERSION: &str = "14";
pub const DEVICE_TYPE: &str = "23127PN0CC";
pub const DEVICE_BRAND: &str = "Xiaomi";
pub const DEVICE_MODEL: &str = "23127PN0CC";
pub const RESOLUTION: &str = "1200*2670";
pub const DPI: &str = "460";
pub const LANGUAGE: &str = "zh_CN";
pub const TIMEZONE: i32 = 8;
pub const ACCESS: &str = "wifi";
pub const ROM: &str = "miui_OS1.0.45.0.UNCCNXM";
pub const ROM_VERSION: &str = "OS1.0.45.0.UNCCNXM";

/// User agent string
pub const USER_AGENT: &str = "com.bytedance.android.doubaoime/100102018 (Linux; U; Android 14; zh_CN; 23127PN0CC; Build/UKQ1.230804.001; Cronet/TTNetVersion:94cf429a 2025-11-17 QuicVersion:1f89f732 2025-05-08)";

/// Audio configuration
pub const SAMPLE_RATE: u32 = 16000;
pub const CHANNELS: u16 = 1;
pub const FRAME_DURATION_MS: u32 = 20;

/// Service name for ASR
pub const SERVICE_NAME: &str = "ASR";
