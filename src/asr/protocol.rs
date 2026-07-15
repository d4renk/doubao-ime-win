//! ASR Protocol Handling
//!
//! Handles building and parsing ASR protocol messages.

use prost::Message;
use serde::Serialize;
use serde_json::Value;

use super::proto::{AsrRequest, AsrResponse as AsrResponseProto, FrameState};
use crate::data::AudioQuality;

/// Response types from ASR server
#[derive(Debug, Clone, PartialEq)]
pub enum ResponseType {
    TaskStarted,
    SessionStarted,
    SessionFinished,
    VadStart,
    InterimResult,
    FinalResult,
    Heartbeat,
    Error,
    Unknown,
}

/// Parsed ASR response
#[derive(Debug, Clone)]
pub struct AsrResponse {
    pub response_type: ResponseType,
    pub text: String,
    pub is_final: bool,
    pub session_finished: bool,
    pub vad_start: bool,
    pub vad_finished: bool,
    pub stream_asr_finished: bool,
    pub nonstream_result: bool,
    pub is_offline_result: bool,
    pub packet_number: i32,
    pub error_msg: String,
    pub raw_json: Option<Value>,
}

impl Default for AsrResponse {
    fn default() -> Self {
        Self {
            response_type: ResponseType::Unknown,
            text: String::new(),
            is_final: false,
            session_finished: false,
            vad_start: false,
            vad_finished: false,
            stream_asr_finished: false,
            nonstream_result: false,
            is_offline_result: false,
            packet_number: -1,
            error_msg: String::new(),
            raw_json: None,
        }
    }
}

/// Session configuration for ASR
#[derive(Debug, Serialize)]
pub struct SessionConfig {
    pub audio_info: AudioInfo,
    pub enable_punctuation: bool,
    pub enable_speech_rejection: bool,
    pub extra: SessionExtra,
}

#[derive(Debug, Serialize)]
pub struct AudioInfo {
    pub channel: u16,
    pub format: String,
    pub sample_rate: u32,
}

#[derive(Debug, Serialize)]
pub struct SessionExtra {
    pub app_name: String,
    pub cell_compress_rate: u32,
    pub did: String,
    pub enable_asr_threepass: bool,
    pub enable_asr_twopass: bool,
    pub use_twopass_retry: bool,
    pub end_smooth_window_ms: u32,
    pub input_mode: String,
}

impl SessionConfig {
    pub fn new(device_id: &str, audio_quality: AudioQuality, end_smooth_window_ms: u32) -> Self {
        Self {
            audio_info: AudioInfo {
                channel: audio_quality.channels(),
                format: "speech_opus".to_string(),
                sample_rate: audio_quality.sample_rate(),
            },
            enable_punctuation: true,
            enable_speech_rejection: false,
            extra: SessionExtra {
                app_name: "com.android.chrome".to_string(),
                cell_compress_rate: 8,
                did: device_id.to_string(),
                enable_asr_threepass: true,
                enable_asr_twopass: true,
                use_twopass_retry: true,
                end_smooth_window_ms: end_smooth_window_ms.min(3_000),
                input_mode: "tool".to_string(),
            },
        }
    }
}

/// Build StartTask message
pub fn build_start_task(request_id: &str, token: &str) -> Vec<u8> {
    let request = AsrRequest {
        token: token.to_string(),
        service_name: "ASR".to_string(),
        method_name: "StartTask".to_string(),
        payload: String::new(),
        audio_data: Vec::new(),
        request_id: request_id.to_string(),
        frame_state: FrameState::Unspecified as i32,
    };
    request.encode_to_vec()
}

/// Build StartSession message
pub fn build_start_session(request_id: &str, token: &str, config: &SessionConfig) -> Vec<u8> {
    let payload = serde_json::to_string(config).unwrap_or_default();
    let request = AsrRequest {
        token: token.to_string(),
        service_name: "ASR".to_string(),
        method_name: "StartSession".to_string(),
        payload,
        audio_data: Vec::new(),
        request_id: request_id.to_string(),
        frame_state: FrameState::Unspecified as i32,
    };
    request.encode_to_vec()
}

/// Build FinishSession message
pub fn build_finish_session(request_id: &str, token: &str) -> Vec<u8> {
    let request = AsrRequest {
        token: token.to_string(),
        service_name: "ASR".to_string(),
        method_name: "FinishSession".to_string(),
        payload: String::new(),
        audio_data: Vec::new(),
        request_id: request_id.to_string(),
        frame_state: FrameState::Unspecified as i32,
    };
    request.encode_to_vec()
}

/// Build TaskRequest message (audio frame)
pub fn build_task_request(
    request_id: &str,
    audio_data: Vec<u8>,
    frame_state: FrameState,
    timestamp_ms: u64,
) -> Vec<u8> {
    let metadata = serde_json::json!({
        "extra": {},
        "timestamp_ms": timestamp_ms
    });
    let request = AsrRequest {
        token: String::new(), // Token not needed for TaskRequest
        service_name: "ASR".to_string(),
        method_name: "TaskRequest".to_string(),
        payload: metadata.to_string(),
        audio_data,
        request_id: request_id.to_string(),
        frame_state: frame_state as i32,
    };
    request.encode_to_vec()
}

/// Parse ASR response from binary data
pub fn parse_response(data: &[u8]) -> AsrResponse {
    let pb = match AsrResponseProto::decode(data) {
        Ok(pb) => pb,
        Err(e) => {
            tracing::error!("Failed to decode ASR response: {}", e);
            return AsrResponse {
                response_type: ResponseType::Error,
                error_msg: format!("Decode error: {}", e),
                ..Default::default()
            };
        }
    };

    let message_type = &pb.message_type;
    let result_json = &pb.result_json;
    let status_message = &pb.status_message;

    // Check message type
    match message_type.as_str() {
        "TaskStarted" => {
            return AsrResponse {
                response_type: ResponseType::TaskStarted,
                ..Default::default()
            };
        }
        "SessionStarted" => {
            return AsrResponse {
                response_type: ResponseType::SessionStarted,
                ..Default::default()
            };
        }
        // Recognition revisions are delivered by the result callback. These
        // messages only close the server-side lifecycle, even if a server
        // variant happens to attach diagnostic JSON.
        "ASR_Finished" | "SessionFinished" => {
            return AsrResponse {
                response_type: ResponseType::SessionFinished,
                session_finished: true,
                ..Default::default()
            };
        }
        "TaskFailed" | "SessionFailed" => {
            return AsrResponse {
                response_type: ResponseType::Error,
                error_msg: status_message.clone(),
                ..Default::default()
            };
        }
        _ => {}
    }

    // Parse result_json for recognition results
    if result_json.is_empty() {
        return AsrResponse {
            response_type: ResponseType::Unknown,
            ..Default::default()
        };
    }

    let json_data: Value = match serde_json::from_str(result_json) {
        Ok(v) => v,
        Err(_) => {
            return AsrResponse {
                response_type: ResponseType::Unknown,
                ..Default::default()
            };
        }
    };

    let results = json_data.get("results");
    let extra = json_data.get("extra").cloned().unwrap_or(Value::Null);

    // No results - might be heartbeat
    if results.is_none() {
        let packet_number = extra
            .get("packet_number")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1) as i32;
        return AsrResponse {
            response_type: ResponseType::Heartbeat,
            packet_number,
            raw_json: Some(json_data),
            ..Default::default()
        };
    }

    // Check for VAD start
    if extra
        .get("vad_start")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return AsrResponse {
            response_type: ResponseType::VadStart,
            vad_start: true,
            raw_json: Some(json_data),
            ..Default::default()
        };
    }

    // Parse recognition results
    let results = results.unwrap();
    // The native callback consumes results[0] as the complete replacement for
    // the current, not-yet-committed segment.
    let result = results.as_array().and_then(|items| items.first());
    let text = result
        .and_then(|r| r.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let result_flag = |name: &str| {
        result
            .and_then(|r| {
                r.get(name)
                    .or_else(|| r.get("extra").and_then(|extra| extra.get(name)))
            })
            .or_else(|| extra.get(name))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let vad_finished = result_flag("is_vad_finished");
    let stream_asr_finished = result_flag("stream_asr_finish");
    let nonstream_result = result_flag("nonstream_result");
    let is_offline_result = result_flag("is_offline_result");

    // Determine response type
    if vad_finished {
        AsrResponse {
            response_type: ResponseType::FinalResult,
            text,
            is_final: true,
            vad_finished,
            stream_asr_finished,
            nonstream_result,
            is_offline_result,
            raw_json: Some(json_data),
            ..Default::default()
        }
    } else {
        AsrResponse {
            response_type: ResponseType::InterimResult,
            text,
            is_final: false,
            stream_asr_finished,
            nonstream_result,
            is_offline_result,
            raw_json: Some(json_data),
            ..Default::default()
        }
    }
}
