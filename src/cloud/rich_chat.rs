use std::sync::Arc;

use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::sse::{SseDecoder, SseEvent};
use super::{http_client, CloudEndpoints, CloudError, RICH_CHAT_TIMEOUT, USER_AGENT};

const FILLER_CLEANUP_INSTRUCTION: &str = "任务：清理语音转写中的口水词。只删除口头语、语气词、重复表达和无意义停顿；除此之外逐字保留有效内容，不扩写、不改写、不翻译，不改变专有名词、数字和原意。不要解释，只输出清理后的文本。\n<语音转写>";

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
    endpoints: CloudEndpoints,
    did: Arc<str>,
}

impl RichChatClient {
    pub fn new(did: impl Into<String>) -> Result<Self, CloudError> {
        Self::with_client(http_client()?, did, CloudEndpoints::default())
    }

    pub fn with_client(
        http: Client,
        did: impl Into<String>,
        endpoints: CloudEndpoints,
    ) -> Result<Self, CloudError> {
        let did = did.into();
        if did.trim().is_empty() {
            return Err(CloudError::InvalidResponse("device id is empty".into()));
        }
        Ok(Self {
            http,
            endpoints,
            did: Arc::from(did),
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
        let cleanup_query = filler_cleanup_query(&input.query);
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
            .post(&self.endpoints.rich_chat_url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .header("User-Agent", USER_AGENT)
            .header("x-tt-e-k", format!("{}+W", self.did))
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
}

fn filler_cleanup_query(transcript: &str) -> String {
    format!("{FILLER_CLEANUP_INSTRUCTION}\n{transcript}\n</语音转写>")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_content_wins_over_deltas() {
        let mut state = RichChatState::default();
        state
            .apply(SseEvent {
                event: "scene.delta".into(),
                data: r#"{"content":"part"}"#.into(),
            })
            .unwrap();
        state
            .apply(SseEvent {
                event: "scene.completed".into(),
                data: r#"{"content":"complete"}"#.into(),
            })
            .unwrap();
        assert_eq!(
            state.finish().unwrap(),
            RichChatResult {
                content: "complete".into(),
                used_delta_fallback: false,
            }
        );
    }

    #[test]
    fn completed_result_is_available_before_stream_eof() {
        let mut state = RichChatState::default();
        state
            .apply(SseEvent {
                event: "scene.delta".into(),
                data: r#"{"content":"part"}"#.into(),
            })
            .unwrap();
        assert!(state.completed_result().is_none());

        state
            .apply(SseEvent {
                event: "scene.completed".into(),
                data: r#"{"content":"complete"}"#.into(),
            })
            .unwrap();
        assert_eq!(
            state.completed_result(),
            Some(RichChatResult {
                content: "complete".into(),
                used_delta_fallback: false,
            })
        );
    }

    #[test]
    fn supports_nested_data_content_and_delta_fallback() {
        let mut state = RichChatState::default();
        state
            .apply(SseEvent {
                event: "scene.delta".into(),
                data: r#"{"data":{"content":"nested"}}"#.into(),
            })
            .unwrap();
        assert_eq!(state.finish().unwrap().content, "nested");
    }

    #[test]
    fn supports_event_name_embedded_in_json_data() {
        let mut state = RichChatState::default();
        state
            .apply(SseEvent {
                event: String::new(),
                data: r#"{"event":"scene.completed","content":"complete"}"#.into(),
            })
            .unwrap();
        assert_eq!(state.finish().unwrap().content, "complete");
    }

    #[test]
    fn rejects_server_error_events() {
        let mut state = RichChatState::default();
        let error = state
            .apply(SseEvent {
                event: "scene.error".into(),
                data: r#"{"message":"failed"}"#.into(),
            })
            .unwrap_err();
        assert!(matches!(error, CloudError::ApiStatus(_)));
    }

    #[test]
    fn serializes_scene_five_request() {
        let request = RichChatRequest {
            scene: 5,
            query: "原文",
            preceding_part: "前",
            follows_below: "后",
            format_query: "",
            output_format: 3,
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "scene": 5,
                "query": "原文",
                "preceding_part": "前",
                "follows_below": "后",
                "format_query": "",
                "output_format": 3,
            })
        );
    }

    #[test]
    fn filler_cleanup_prompt_is_narrow_and_preserves_the_transcript() {
        let query = filler_cleanup_query("嗯这个这个方案可以");
        assert!(query.contains("口头语"));
        assert!(query.contains("不扩写"));
        assert!(query.contains("不改写"));
        assert!(query.contains("不翻译"));
        assert!(query.contains("<语音转写>\n嗯这个这个方案可以\n</语音转写>"));
    }
}
