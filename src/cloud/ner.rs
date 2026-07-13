use std::sync::Arc;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::{
    http_client, CloudEndpoints, CloudError, APP_ID, APP_VERSION, NER_TASK_BUDGET, SAMI_APP_KEY,
    USER_AGENT,
};

const NER_SUCCESS_STATUS: &str = "20000000";
const MAX_NER_TEXT_CHARS: usize = 500;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct NerWord {
    pub word: String,
    pub freq: u32,
}

#[derive(Clone)]
pub struct NerClient {
    http: Client,
    endpoints: CloudEndpoints,
    did: Arc<str>,
    token: Arc<Mutex<Option<String>>>,
}

impl NerClient {
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
            token: Arc::new(Mutex::new(None)),
        })
    }

    /// Extracts words without ever exceeding the complete two-second NER budget.
    pub async fn extract_words(&self, text: &str) -> Result<Vec<NerWord>, CloudError> {
        tokio::time::timeout(NER_TASK_BUDGET, self.extract_words_inner(text))
            .await
            .map_err(|_| CloudError::Timeout)?
    }

    /// Warms the in-memory SAMI token cache without sending recognized text.
    pub async fn prefetch_token(&self) -> Result<(), CloudError> {
        tokio::time::timeout(NER_TASK_BUDGET, self.token())
            .await
            .map_err(|_| CloudError::Timeout)??;
        Ok(())
    }

    pub async fn clear_cached_token(&self) {
        *self.token.lock().await = None;
    }

    async fn extract_words_inner(&self, text: &str) -> Result<Vec<NerWord>, CloudError> {
        let text = trailing_chars(text, MAX_NER_TEXT_CHARS);
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }

        let mut token = self.token().await?;
        let mut response = self.request_ner(&text, &token).await?;
        if matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            self.clear_cached_token().await;
            token = self.token().await?;
            response = self.request_ner(&text, &token).await?;
        }
        parse_ner_response(response).await
    }

    async fn token(&self) -> Result<String, CloudError> {
        let mut cached = self.token.lock().await;
        if let Some(token) = cached.as_ref() {
            return Ok(token.clone());
        }

        let response = self
            .http
            .post(&self.endpoints.ner_token_url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("app_version", APP_VERSION)
            .header("app_id", APP_ID)
            .header("os_type", "Android")
            .header("did", self.did.as_ref())
            .header("User-Agent", USER_AGENT)
            .json(&TokenRequest {
                sami_app_key: SAMI_APP_KEY,
            })
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(CloudError::ApiStatus(format!(
                "token HTTP {}",
                response.status()
            )));
        }
        let body: TokenResponse = response.json().await?;
        let token = body
            .data
            .and_then(|data| data.sami_token)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CloudError::InvalidResponse("missing SAMI token".into()))?;
        *cached = Some(token.clone());
        Ok(token)
    }

    async fn request_ner(&self, text: &str, token: &str) -> Result<reqwest::Response, CloudError> {
        Ok(self
            .http
            .post(&self.endpoints.ner_url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("app_version", APP_VERSION)
            .header("app_id", APP_ID)
            .header("os_type", "Android")
            .header("X-Api-Resource-Id", "asr.user.context")
            .header("X-Api-App-Key", SAMI_APP_KEY)
            .header("X-Api-Token", token)
            .header("X-Api-Request-Id", Uuid::new_v4().to_string())
            .header("X-Api-Sequence", "-1")
            .header("x-tt-e-k", format!("{}+W", self.did))
            .header("x-tt-e-b", "1")
            .header("x-metasec-bp-body-compress", "1")
            .header("User-Agent", USER_AGENT)
            .json(&NerRequest {
                user: NerUser {
                    uid: "",
                    did: &self.did,
                    app_name: "doubaoime",
                    app_version: APP_VERSION,
                    sdk_version: "",
                    platform: "android",
                    experience_improve: false,
                },
                text,
                additions: serde_json::Map::new(),
            })
            .send()
            .await?)
    }
}

#[derive(Serialize)]
struct TokenRequest<'a> {
    sami_app_key: &'a str,
}

#[derive(Deserialize)]
struct TokenResponse {
    data: Option<TokenData>,
}

#[derive(Deserialize)]
struct TokenData {
    sami_token: Option<String>,
}

#[derive(Serialize)]
struct NerRequest<'a> {
    user: NerUser<'a>,
    text: &'a str,
    additions: serde_json::Map<String, serde_json::Value>,
}

#[derive(Serialize)]
struct NerUser<'a> {
    uid: &'a str,
    did: &'a str,
    app_name: &'a str,
    app_version: &'a str,
    sdk_version: &'a str,
    platform: &'a str,
    experience_improve: bool,
}

#[derive(Deserialize)]
struct NerResponse {
    #[serde(default)]
    results: Vec<NerResult>,
}

#[derive(Deserialize)]
struct NerResult {
    #[serde(default)]
    words: Vec<NerWord>,
}

async fn parse_ner_response(response: reqwest::Response) -> Result<Vec<NerWord>, CloudError> {
    if !response.status().is_success() {
        return Err(CloudError::ApiStatus(format!(
            "NER HTTP {}",
            response.status()
        )));
    }
    let api_status = response
        .headers()
        .get("x-api-status-code")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if api_status != NER_SUCCESS_STATUS {
        return Err(CloudError::ApiStatus(format!("NER status {api_status}")));
    }
    let bytes = response.bytes().await?;
    if bytes.is_empty() {
        return Err(CloudError::InvalidResponse("empty NER body".into()));
    }
    let body: NerResponse = serde_json::from_slice(&bytes)
        .map_err(|error| CloudError::InvalidResponse(format!("invalid NER JSON: {error}")))?;
    Ok(body
        .results
        .into_iter()
        .flat_map(|result| result.words)
        .filter(|word| !word.word.trim().is_empty() && word.freq > 0)
        .collect())
}

fn trailing_chars(value: &str, limit: usize) -> String {
    let start = value
        .char_indices()
        .rev()
        .nth(limit.saturating_sub(1))
        .map(|(index, _)| index)
        .unwrap_or(0);
    value[start..].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_by_unicode_scalar_value() {
        let input = format!("prefix{}", "好".repeat(500));
        let truncated = trailing_chars(&input, 500);
        assert_eq!(truncated.chars().count(), 500);
        assert!(truncated.chars().all(|value| value == '好'));
    }

    #[test]
    fn deserializes_only_words_from_results() {
        let response: NerResponse = serde_json::from_str(
            r#"{"results":[{"text":"do not use this","words":[{"word":"北京","freq":2}]}]}"#,
        )
        .unwrap();
        assert_eq!(response.results[0].words[0].word, "北京");
    }

    #[test]
    fn serializes_compatible_request_shape() {
        let body = NerRequest {
            user: NerUser {
                uid: "",
                did: "did",
                app_name: "doubaoime",
                app_version: APP_VERSION,
                sdk_version: "",
                platform: "android",
                experience_improve: false,
            },
            text: "测试",
            additions: serde_json::Map::new(),
        };
        let value = serde_json::to_value(body).unwrap();
        assert_eq!(value["text"], "测试");
        assert_eq!(value["user"]["did"], "did");
        assert_eq!(value["additions"], serde_json::json!({}));
    }
}
