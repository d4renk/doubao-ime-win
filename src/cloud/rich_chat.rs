use std::sync::Arc;

use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};

use super::sse::{SseDecoder, SseEvent};
use super::{http_client, CloudEndpoints, CloudError, RICH_CHAT_TIMEOUT, USER_AGENT};
use crate::data::CloudConfig;

const SPEECH_CORRECTION_INSTRUCTION: &str = "删除口头语和重复内容，并结合上下文纠错、调整语序。";
const CUSTOM_LLM_TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CustomLlmTestResult {
    Success,
    InvalidConfig,
    AuthenticationFailed,
    Unsupported,
    Timeout,
    NetworkError,
    HttpError(u16),
}

impl CustomLlmTestResult {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }

    pub fn message(&self) -> String {
        match self {
            Self::Success => "连接成功，鉴权有效".into(),
            Self::InvalidConfig => "测试失败，请填写有效的接口地址和密钥".into(),
            Self::AuthenticationFailed => "鉴权失败，请检查密钥".into(),
            Self::Unsupported => "服务不支持模型列表测试接口".into(),
            Self::Timeout => "连接超时，请检查接口地址和网络".into(),
            Self::NetworkError => "网络错误，请检查接口地址和网络连接".into(),
            Self::HttpError(status) => format!("连接失败，服务返回状态码 {status}"),
        }
    }
}

pub async fn test_custom_llm(config: &CloudConfig) -> CustomLlmTestResult {
    let Some(models_url) = custom_llm_models_url(&config.llm_base_url) else {
        return CustomLlmTestResult::InvalidConfig;
    };
    let api_key = config.llm_api_key.trim();
    if api_key.is_empty() {
        return CustomLlmTestResult::InvalidConfig;
    }

    let http = match http_client() {
        Ok(http) => http,
        Err(_) => return CustomLlmTestResult::NetworkError,
    };
    let response = http
        .get(models_url)
        .bearer_auth(api_key)
        .timeout(CUSTOM_LLM_TEST_TIMEOUT)
        .send()
        .await;
    match response {
        Ok(response) if response.status().is_success() => CustomLlmTestResult::Success,
        Ok(response)
            if matches!(
                response.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ) =>
        {
            CustomLlmTestResult::AuthenticationFailed
        }
        Ok(response)
            if matches!(
                response.status(),
                StatusCode::NOT_FOUND
                    | StatusCode::METHOD_NOT_ALLOWED
                    | StatusCode::NOT_IMPLEMENTED
            ) =>
        {
            CustomLlmTestResult::Unsupported
        }
        Ok(response) => CustomLlmTestResult::HttpError(response.status().as_u16()),
        Err(error) if error.is_timeout() => CustomLlmTestResult::Timeout,
        Err(_) => CustomLlmTestResult::NetworkError,
    }
}

fn custom_llm_models_url(chat_completions_url: &str) -> Option<Url> {
    let mut url = Url::parse(chat_completions_url.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    let segments = url.path_segments()?.collect::<Vec<_>>();
    let chat_index = segments
        .iter()
        .rposition(|segment| segment.eq_ignore_ascii_case("chat"))?;
    if segments
        .get(chat_index + 1)
        .is_none_or(|segment| !segment.eq_ignore_ascii_case("completions"))
        || chat_index + 2 != segments.len()
    {
        return None;
    }
    let models_path = segments[..chat_index]
        .iter()
        .chain(std::iter::once(&"models"))
        .copied()
        .collect::<Vec<_>>()
        .join("/");
    url.set_path(&models_path);
    Some(url)
}

pub(crate) fn default_speech_correction_instruction() -> &'static str {
    SPEECH_CORRECTION_INSTRUCTION
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RichChatInput {
    pub query: String,
    pub preceding_part: String,
    pub follows_below: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RichChatResult {
    pub content: String,
    pub used_delta_fallback: bool,
}

#[derive(Clone)]
pub struct RichChatClient {
    http: Client,
    backend: RichChatBackend,
    instruction: Arc<str>,
}

#[derive(Clone)]
enum RichChatBackend {
    Doubao {
        endpoints: CloudEndpoints,
        did: Arc<str>,
    },
    OpenAi(OpenAiConfig),
}

#[derive(Clone)]
struct OpenAiConfig {
    url: Arc<str>,
    api_key: Arc<str>,
    model: Arc<str>,
    thinking_mode: Option<Arc<str>>,
    reasoning_effort: Option<Arc<str>>,
}

impl RichChatClient {
    pub fn new(did: impl Into<String>, config: &CloudConfig) -> Result<Self, CloudError> {
        Self::with_client(http_client()?, did, config, CloudEndpoints::default())
    }

    pub fn with_client(
        http: Client,
        did: impl Into<String>,
        config: &CloudConfig,
        endpoints: CloudEndpoints,
    ) -> Result<Self, CloudError> {
        let did = did.into();
        let custom_api_enabled = config.custom_api_enabled();
        if did.trim().is_empty() && !custom_api_enabled {
            return Err(CloudError::InvalidResponse("device id is empty".into()));
        }
        let backend = if !custom_api_enabled {
            RichChatBackend::Doubao {
                endpoints,
                did: Arc::from(did),
            }
        } else {
            let url = config.llm_base_url.trim();
            let api_key = config.llm_api_key.trim();
            let model = config.llm_model.trim();
            if url.is_empty() || api_key.is_empty() || model.is_empty() {
                return Err(CloudError::InvalidResponse(
                    "custom OpenAI-compatible LLM requires URL, API key, and model".into(),
                ));
            }
            RichChatBackend::OpenAi(OpenAiConfig {
                url: Arc::from(url),
                api_key: Arc::from(api_key),
                model: Arc::from(model),
                thinking_mode: normalize_thinking_mode(&config.llm_thinking_mode).map(Arc::from),
                reasoning_effort: normalize_reasoning_effort(&config.llm_reasoning_effort)
                    .map(Arc::from),
            })
        };
        Ok(Self {
            http,
            backend,
            instruction: Arc::from(speech_correction_instruction(&config.llm_prompt)),
        })
    }

    pub async fn polish(&self, input: RichChatInput) -> Result<RichChatResult, CloudError> {
        tokio::time::timeout(RICH_CHAT_TIMEOUT, self.polish_inner(input))
            .await
            .map_err(|_| CloudError::Timeout)?
    }

    async fn polish_inner(&self, input: RichChatInput) -> Result<RichChatResult, CloudError> {
        if input.query.trim().is_empty() {
            return Err(CloudError::InvalidResponse(
                "rich chat query is empty".into(),
            ));
        }
        match &self.backend {
            RichChatBackend::Doubao { endpoints, did } => {
                self.polish_doubao(input, endpoints, did).await
            }
            RichChatBackend::OpenAi(config) => self.polish_openai(input, config).await,
        }
    }

    async fn polish_doubao(
        &self,
        input: RichChatInput,
        endpoints: &CloudEndpoints,
        did: &str,
    ) -> Result<RichChatResult, CloudError> {
        let cleanup_query = speech_correction_query(&self.instruction, &input);
        let request = RichChatRequest {
            scene: 5,
            query: &cleanup_query,
            preceding_part: &input.preceding_part,
            follows_below: &input.follows_below,
            format_query: "",
            output_format: 3,
        };
        let mut response = self
            .http
            .post(&endpoints.rich_chat_url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .header("User-Agent", USER_AGENT)
            .header("x-tt-e-k", format!("{}+W", did))
            .header("x-tt-e-b", "1")
            .header("x-metasec-bp-body-compress", "1")
            .json(&request)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(CloudError::ApiStatus(format!(
                "rich chat HTTP {}",
                response.status()
            )));
        }

        let mut decoder = SseDecoder::default();
        let mut state = RichChatState::default();
        while let Some(chunk) = response.chunk().await? {
            for event in decoder.push(&chunk)? {
                state.apply(event)?;
                if let Some(result) = state.completed_result() {
                    return Ok(result);
                }
            }
        }
        for event in decoder.finish()? {
            state.apply(event)?;
        }
        state.finish()
    }

    async fn polish_openai(
        &self,
        input: RichChatInput,
        config: &OpenAiConfig,
    ) -> Result<RichChatResult, CloudError> {
        let prompt = speech_correction_query(&self.instruction, &input);
        let request = OpenAiRequest {
            model: &config.model,
            messages: [
                OpenAiMessage {
                    role: "system",
                    content: &self.instruction,
                },
                OpenAiMessage {
                    role: "user",
                    content: &prompt,
                },
            ],
            stream: true,
            thinking: config
                .thinking_mode
                .as_deref()
                .map(|kind| Thinking { kind }),
            reasoning_effort: config.reasoning_effort.as_deref(),
        };
        let mut response = self
            .http
            .post(config.url.as_ref())
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .bearer_auth(config.api_key.as_ref())
            .json(&request)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(CloudError::ApiStatus(format!(
                "OpenAI-compatible LLM HTTP {}",
                response.status()
            )));
        }

        let mut decoder = SseDecoder::default();
        let mut state = OpenAiStreamState::default();
        while let Some(chunk) = response.chunk().await? {
            for event in decoder.push(&chunk)? {
                state.apply(event)?;
            }
        }
        for event in decoder.finish()? {
            state.apply(event)?;
        }
        state.finish()
    }
}

fn non_empty(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then(|| value.trim())
}

fn normalize_thinking_mode(value: &str) -> Option<&str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "enabled" => Some("enabled"),
        "disabled" => Some("disabled"),
        _ => None,
    }
}

fn normalize_reasoning_effort(value: &str) -> Option<&str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        _ => None,
    }
}

fn speech_correction_instruction(configured_prompt: &str) -> &str {
    non_empty(configured_prompt).unwrap_or(default_speech_correction_instruction())
}

fn speech_correction_query(instruction: &str, input: &RichChatInput) -> String {
    format!(
        "{instruction}\n<前文>\n{}\n</前文>\n<语音转写>\n{}\n</语音转写>\n<后文>\n{}\n</后文>",
        input.preceding_part, input.query, input.follows_below
    )
}

#[derive(Serialize)]
struct RichChatRequest<'a> {
    scene: u8,
    query: &'a str,
    preceding_part: &'a str,
    follows_below: &'a str,
    format_query: &'a str,
    output_format: u8,
}

#[derive(Serialize)]
struct OpenAiRequest<'a> {
    model: &'a str,
    messages: [OpenAiMessage<'a>; 2],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<Thinking<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
}

#[derive(Serialize)]
struct OpenAiMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Serialize)]
struct Thinking<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
}

#[derive(Deserialize)]
struct EventData {
    #[serde(default)]
    content: String,
    #[serde(default)]
    data: Option<NestedEventData>,
}

#[derive(Deserialize)]
struct NestedEventData {
    #[serde(default)]
    content: String,
}

impl EventData {
    fn content(self) -> String {
        if self.content.is_empty() {
            self.data.map(|data| data.content).unwrap_or_default()
        } else {
            self.content
        }
    }
}

#[derive(Default)]
struct RichChatState {
    delta: String,
    completed: Option<String>,
}

#[derive(Default)]
struct OpenAiStreamState {
    content: String,
}

impl OpenAiStreamState {
    fn apply(&mut self, event: SseEvent) -> Result<(), CloudError> {
        if event.data.trim() == "[DONE]" {
            return Ok(());
        }
        let value: serde_json::Value = serde_json::from_str(&event.data).map_err(|error| {
            CloudError::InvalidResponse(format!("invalid OpenAI-compatible SSE data: {error}"))
        })?;
        if let Some(error) = value.get("error") {
            return Err(CloudError::ApiStatus(error.to_string()));
        }
        if let Some(content) = value
            .get("choices")
            .and_then(serde_json::Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("content"))
            .and_then(serde_json::Value::as_str)
        {
            self.content.push_str(content);
        }
        Ok(())
    }

    fn finish(self) -> Result<RichChatResult, CloudError> {
        if self.content.trim().is_empty() {
            return Err(CloudError::InvalidResponse(
                "OpenAI-compatible LLM produced no content".into(),
            ));
        }
        Ok(RichChatResult {
            content: self.content,
            used_delta_fallback: false,
        })
    }
}

impl RichChatState {
    fn apply(&mut self, event: SseEvent) -> Result<(), CloudError> {
        let event_name = if event.event.is_empty() || event.event == "message" {
            embedded_event_name(&event.data).unwrap_or_else(|| event.event.clone())
        } else {
            event.event.clone()
        };
        match event_name.as_str() {
            "scene.delta" => self.delta.push_str(&parse_content(&event.data)?),
            "scene.completed" => {
                let content = parse_content(&event.data)?;
                if !content.is_empty() {
                    self.completed = Some(content);
                }
            }
            "scene.error" | "format.error" => {
                return Err(CloudError::ApiStatus(format!(
                    "{}: {}",
                    event.event, event.data
                )));
            }
            _ => {}
        }
        Ok(())
    }

    fn completed_result(&self) -> Option<RichChatResult> {
        self.completed.as_ref().map(|content| RichChatResult {
            content: content.clone(),
            used_delta_fallback: false,
        })
    }

    fn finish(self) -> Result<RichChatResult, CloudError> {
        if let Some(content) = self.completed {
            return Ok(RichChatResult {
                content,
                used_delta_fallback: false,
            });
        }
        // `finish` is called only after a clean HTTP EOF. Stream errors return earlier.
        if !self.delta.is_empty() {
            return Ok(RichChatResult {
                content: self.delta,
                used_delta_fallback: true,
            });
        }
        Err(CloudError::InvalidResponse(
            "rich chat produced no content".into(),
        ))
    }
}

fn embedded_event_name(data: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(data).ok()?;
    value
        .get("event")
        .or_else(|| value.get("type"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

fn parse_content(data: &str) -> Result<String, CloudError> {
    serde_json::from_str::<EventData>(data)
        .map(EventData::content)
        .map_err(|error| CloudError::InvalidResponse(format!("invalid SSE data: {error}")))
}
