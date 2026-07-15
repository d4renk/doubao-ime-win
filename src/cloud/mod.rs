//! Cloud-side context extraction and text polishing services.

mod lexicon;
mod ner;
mod rich_chat;
mod sse;

use std::time::Duration;

pub use lexicon::NerLexicon;
pub use ner::{NerClient, NerWord};
pub use rich_chat::{
    test_custom_llm, CustomLlmTestResult, RichChatClient, RichChatInput, RichChatResult,
};

pub const NER_TASK_BUDGET: Duration = Duration::from_secs(2);
pub const RICH_CHAT_TIMEOUT: Duration = Duration::from_secs(10);

const APP_VERSION: &str = "1.3.7";
const APP_ID: &str = "401734";
const SAMI_APP_KEY: &str = "SYlxZr6LnvBaIVmF";
const USER_AGENT: &str = "com.bytedance.android.doubaoime/1.3.7";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloudEndpoints {
    pub ner_token_url: String,
    pub ner_url: String,
    pub rich_chat_url: String,
}

impl Default for CloudEndpoints {
    fn default() -> Self {
        Self {
            ner_token_url: "https://ime.oceancloudapi.com/api/v1/user/get_config".into(),
            ner_url: "https://speech.bytedance.com/api/v3/context/ime/ner".into(),
            rich_chat_url: "https://ime.doubao.com/api/v1/bot/rich_chat".into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error("cloud request timed out")]
    Timeout,
    #[error("cloud request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("cloud API rejected the request: {0}")]
    ApiStatus(String),
    #[error("invalid cloud response: {0}")]
    InvalidResponse(String),
}

fn http_client() -> Result<reqwest::Client, CloudError> {
    crate::init_crypto_provider();
    Ok(reqwest::Client::builder().no_proxy().build()?)
}
