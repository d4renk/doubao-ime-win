//! Voice Controller
//!
//! Coordinates voice input between audio capture, ASR, and text insertion.

use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::task::JoinHandle;

use crate::asr::{AsrClient, ResponseType};
use crate::audio::AudioCapture;
use crate::business::punctuation::{format_transcript, TranscriptBoundary};
use crate::business::{
    capture_context, ContextSnapshot, TargetWindow, TextInserter, VoiceSessionRecord,
    VoiceSessionStore,
};
use crate::cloud::{NerClient, NerLexicon, RichChatClient, RichChatInput};
use crate::data::{AppConfig, AudioProcessingConfig, PunctuationMode};

const ASR_SESSION_FINISH_TIMEOUT: Duration = Duration::from_secs(30);
const EMPTY_ASR_SESSION_FINISH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct CloudRuntime {
    ner_client: Arc<NerClient>,
    ner_lexicon: Arc<StdMutex<NerLexicon>>,
    rich_chat_client: Arc<RichChatClient>,
    sessions: Arc<VoiceSessionStore>,
}

/// Voice input controller
pub struct VoiceController {
    asr_client: Arc<AsrClient>,
    audio_capture: Arc<AudioCapture>,
    text_inserter: Arc<TextInserter>,
    is_recording: Arc<AtomicBool>,
    finish_requested: Arc<AtomicBool>,
    session_has_text: Arc<AtomicBool>,
    result_task: Option<JoinHandle<()>>,
    polish_task: Arc<StdMutex<Option<JoinHandle<()>>>>,
    cloud: Option<CloudRuntime>,
}

impl VoiceController {
    /// Create a new voice controller
    pub fn new(
        asr_client: Arc<AsrClient>,
        audio_capture: Arc<AudioCapture>,
        text_inserter: Arc<TextInserter>,
    ) -> Self {
        Self {
            asr_client,
            audio_capture,
            text_inserter,
            is_recording: Arc::new(AtomicBool::new(false)),
            finish_requested: Arc::new(AtomicBool::new(false)),
            session_has_text: Arc::new(AtomicBool::new(false)),
            result_task: None,
            polish_task: Arc::new(StdMutex::new(None)),
            cloud: None,
        }
    }

    pub fn with_cloud(
        mut self,
        ner_client: Arc<NerClient>,
        ner_lexicon: Arc<StdMutex<NerLexicon>>,
        rich_chat_client: Arc<RichChatClient>,
        sessions: Arc<VoiceSessionStore>,
    ) -> Self {
        self.cloud = Some(CloudRuntime {
            ner_client,
            ner_lexicon,
            rich_chat_client,
            sessions,
        });
        self
    }

    /// Replace the text-polishing backend after settings are saved.
    pub fn reconfigure_rich_chat(&mut self, rich_chat_client: Arc<RichChatClient>) {
        if let Some(cloud) = self.cloud.as_mut() {
            cloud.rich_chat_client = rich_chat_client;
        }
    }

    /// Check if currently recording
    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    /// Toggle voice input on/off
    pub async fn toggle(&mut self) -> Result<()> {
        if self.is_recording() {
            self.stop().await
        } else {
            self.start().await
        }
    }

    /// Start voice input
    pub async fn start(&mut self) -> Result<()> {
        if self.is_recording() {
            return Ok(());
        }

        if let Some(previous_task) = self.result_task.take() {
            if !previous_task.is_finished() {
                tracing::warn!("Cancelling an unfinished ASR result task before starting");
                previous_task.abort();
            }
        }

        if let Some(previous_task) = self
            .polish_task
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            previous_task.abort();
        }

        tracing::info!("Starting voice input...");
        self.finish_requested.store(false, Ordering::SeqCst);
        self.session_has_text.store(false, Ordering::SeqCst);
        let config = AppConfig::load_or_default()?;
        let asr_config = config.asr;
        let audio_quality = asr_config.audio_quality;
        let audio_processing = AudioProcessingConfig::from(&asr_config);
        let punctuation_mode = asr_config.punctuation_mode;
        let ner_enabled = config.cloud.ner_enabled;
        let auto_polish_enabled = config.cloud.auto_polish_enabled;
        let llm_context_enabled = config.cloud.llm_context_enabled;
        let session_generation = self
            .cloud
            .as_ref()
            .map(|cloud| cloud.sessions.begin_session())
            .unwrap_or_default();
        let context = if auto_polish_enabled && self.cloud.is_some() {
            if llm_context_enabled {
                capture_context()
            } else {
                ContextSnapshot {
                    target: TargetWindow::capture_foreground(),
                    ..ContextSnapshot::default()
                }
            }
        } else {
            ContextSnapshot::default()
        };

        // Start audio capture
        tracing::debug!("Starting audio capture...");
        let audio_rx = self.audio_capture.start(audio_quality, audio_processing)?;
        self.is_recording.store(true, Ordering::SeqCst);
        tracing::info!("Audio capture started, frames will be sent to ASR");

        // Start ASR
        tracing::debug!("Connecting to ASR server...");
        let mut result_rx = match self
            .asr_client
            .start_realtime(audio_rx, audio_quality, asr_config.end_smooth_window_ms)
            .await
        {
            Ok(result_rx) => result_rx,
            Err(error) => {
                self.audio_capture.stop();
                self.is_recording.store(false, Ordering::SeqCst);
                return Err(error);
            }
        };
        tracing::info!("ASR connection established");

        // Clone for the task
        let text_inserter = self.text_inserter.clone();
        let is_recording = self.is_recording.clone();
        let finish_requested = self.finish_requested.clone();
        let session_has_text = self.session_has_text.clone();
        let audio_capture = self.audio_capture.clone();
        let cloud = self.cloud.clone();
        let polish_task = self.polish_task.clone();

        // Spawn result processing task
        self.result_task = Some(tokio::spawn(async move {
            let mut last_text = String::new();
            let mut response_count = 0u32;
            let mut completed_normally = false;
            let mut pending_smart_comma = false;
            let mut session_text = String::new();

            tracing::info!("ASR result processing task started");

            while let Some(response) = result_rx.recv().await {
                response_count += 1;
                if !response.text.trim().is_empty() {
                    session_has_text.store(true, Ordering::SeqCst);
                }
                match response.response_type {
                    ResponseType::InterimResult => {
                        tracing::debug!(
                            "[REVISION #{} stream_finished={} nonstream={} offline={}] {}",
                            response_count,
                            response.stream_asr_finished,
                            response.nonstream_result,
                            response.is_offline_result,
                            response.text
                        );
                        println!("📝 [识别中] {}", response.text);
                        if !response.text.is_empty() {
                            let displayed_text = format_transcript(
                                &response.text,
                                punctuation_mode,
                                TranscriptBoundary::Interim,
                            );
                            if let Err(e) = update_text(&text_inserter, &last_text, &displayed_text)
                            {
                                tracing::error!("Failed to update text: {}", e);
                            }
                            last_text = displayed_text;
                        }
                    }
                    ResponseType::FinalResult => {
                        tracing::info!("[FINAL #{}] {}", response_count, response.text);
                        println!("✅ [确认] {}", response.text);
                        if !response.text.is_empty() {
                            let boundary = if finish_requested.load(Ordering::SeqCst) {
                                TranscriptBoundary::SessionFinal
                            } else {
                                TranscriptBoundary::ClauseFinal
                            };
                            let displayed_text =
                                format_transcript(&response.text, punctuation_mode, boundary);
                            if let Err(e) = update_text(&text_inserter, &last_text, &displayed_text)
                            {
                                tracing::error!("Failed to update text: {}", e);
                            } else {
                                session_text.push_str(&displayed_text);
                            }
                            pending_smart_comma = punctuation_mode == PunctuationMode::Smart
                                && boundary == TranscriptBoundary::ClauseFinal
                                && displayed_text.ends_with('，');
                            // A later sentence starts a new incremental text range.
                            last_text = String::new();
                            if ner_enabled {
                                if let Some(cloud) = cloud.as_ref() {
                                    spawn_ner_update(
                                        cloud.ner_client.clone(),
                                        cloud.ner_lexicon.clone(),
                                        response.text.clone(),
                                    );
                                }
                            }
                        }
                    }
                    ResponseType::SessionFinished => {
                        if punctuation_mode == PunctuationMode::Smart {
                            if !last_text.is_empty() {
                                let final_text = format_transcript(
                                    &last_text,
                                    punctuation_mode,
                                    TranscriptBoundary::SessionFinal,
                                );
                                if let Err(error) =
                                    update_text(&text_inserter, &last_text, &final_text)
                                {
                                    tracing::error!("Failed to finalize punctuation: {}", error);
                                } else {
                                    session_text.push_str(&final_text);
                                }
                                last_text.clear();
                            } else if pending_smart_comma {
                                if let Err(error) = replace_last_character(&text_inserter, "。") {
                                    tracing::error!("Failed to finalize punctuation: {}", error);
                                } else if session_text.ends_with('，') {
                                    session_text.pop();
                                    session_text.push('。');
                                }
                            }
                        }
                        tracing::info!("ASR session finished (total {} responses)", response_count);
                        println!("🏁 [会话结束]");
                        completed_normally = true;
                        break;
                    }
                    ResponseType::Error => {
                        tracing::error!("ASR error: {}", response.error_msg);
                        println!("❌ [错误] {}", response.error_msg);
                        break;
                    }
                    _ => {
                        tracing::trace!("Other response type: {:?}", response.response_type);
                    }
                }
            }

            if !completed_normally {
                tracing::warn!(
                    "ASR result stream ended before SessionFinished (processed {} responses)",
                    response_count
                );
            }

            if completed_normally && auto_polish_enabled && !session_text.trim().is_empty() {
                if let (Some(cloud), Some(target)) = (cloud.as_ref(), context.target) {
                    let record = VoiceSessionRecord {
                        generation: session_generation,
                        target_window: target,
                        inserted_chars: session_text.chars().count(),
                        text: session_text,
                        preceding_part: context.preceding_part,
                        follows_below: context.follows_below,
                    };
                    if cloud.sessions.publish(record.clone()) {
                        let task = spawn_polish(cloud.clone(), text_inserter.clone(), record);
                        *polish_task
                            .lock()
                            .unwrap_or_else(|error| error.into_inner()) = Some(task);
                    }
                } else {
                    tracing::debug!("Skipping automatic polish because no target was captured");
                }
            }

            audio_capture.stop();
            is_recording.store(false, Ordering::SeqCst);
        }));

        Ok(())
    }

    /// Stop capturing audio and drain responses through the server's SessionFinished event.
    pub async fn stop(&mut self) -> Result<()> {
        if !self.is_recording() {
            return Ok(());
        }

        tracing::info!("Stopping voice input and waiting for ASR SessionFinished...");
        self.finish_requested.store(true, Ordering::SeqCst);
        self.audio_capture.stop();

        if let Some(mut result_task) = self.result_task.take() {
            let started_without_text = !self.session_has_text.load(Ordering::SeqCst);
            let mut finish_timeout = asr_finish_timeout(!started_without_text);
            loop {
                match tokio::time::timeout(finish_timeout, &mut result_task).await {
                    Ok(Ok(())) => {
                        tracing::info!("Final ASR result processing completed");
                        break;
                    }
                    Ok(Err(error)) => {
                        tracing::warn!("ASR result task failed: {}", error);
                        break;
                    }
                    Err(_)
                        if finish_timeout == EMPTY_ASR_SESSION_FINISH_TIMEOUT
                            && self.session_has_text.load(Ordering::SeqCst) =>
                    {
                        tracing::info!(
                            "ASR text arrived during empty-session grace period; extending finalization"
                        );
                        finish_timeout = ASR_SESSION_FINISH_TIMEOUT;
                    }
                    Err(_) => {
                        tracing::warn!(
                            "Timed out after {:?} waiting for ASR SessionFinished",
                            finish_timeout
                        );
                        result_task.abort();
                        let _ = result_task.await;
                        break;
                    }
                }
            }
        }

        self.is_recording.store(false, Ordering::SeqCst);
        Ok(())
    }
}

fn asr_finish_timeout(has_text: bool) -> Duration {
    if has_text {
        ASR_SESSION_FINISH_TIMEOUT
    } else {
        EMPTY_ASR_SESSION_FINISH_TIMEOUT
    }
}

fn spawn_ner_update(client: Arc<NerClient>, lexicon: Arc<StdMutex<NerLexicon>>, text: String) {
    tokio::spawn(async move {
        match client.extract_words(&text).await {
            Ok(words) => {
                lexicon
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .update(words);
            }
            Err(error) => tracing::debug!("NER side request did not update context: {}", error),
        }
    });
}

fn spawn_polish(
    cloud: CloudRuntime,
    text_inserter: Arc<TextInserter>,
    record: VoiceSessionRecord,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let input = RichChatInput {
            query: record.text.clone(),
            preceding_part: record.preceding_part.clone(),
            follows_below: record.follows_below.clone(),
        };
        match cloud.rich_chat_client.polish(input).await {
            Ok(result) => {
                if result.content.trim().is_empty() {
                    tracing::warn!(
                        "Automatic speech correction returned empty text; keeping ASR text"
                    );
                    return;
                }
                if result.content == record.text {
                    tracing::debug!("Automatic speech correction did not change the ASR text");
                    return;
                }

                let sessions = cloud.sessions.clone();
                let generation = record.generation;
                let target = record.target_window;
                let inserted_chars = record.inserted_chars;
                let replacement = result.content;
                let replace_result = tokio::task::spawn_blocking(move || {
                    sessions.run_if_current(generation, || {
                        text_inserter.replace_recent(target, inserted_chars, &replacement)
                    })
                })
                .await;

                match replace_result {
                    Ok(Some(Ok(()))) => {
                        tracing::info!("Automatically replaced ASR text after speech correction")
                    }
                    Ok(Some(Err(error))) => {
                        tracing::warn!("Could not automatically replace ASR text: {}", error)
                    }
                    Ok(None) => {
                        tracing::debug!("Discarding a polish result from an expired voice session")
                    }
                    Err(error) => {
                        tracing::warn!("Automatic text replacement task failed: {}", error)
                    }
                }
            }
            Err(error) => tracing::warn!("Automatic polish failed: {}", error),
        }
    })
}

fn replace_last_character(text_inserter: &TextInserter, replacement: &str) -> Result<()> {
    text_inserter.delete_chars(1)?;
    text_inserter.insert(replacement)
}

/// Update text in the focused window using incremental updates
///
/// Uses prefix matching to minimize deletions and insertions:
/// 1. Find the common prefix between old and new text
/// 2. Only delete characters beyond the common prefix
/// 3. Only append the new suffix
///
/// This significantly reduces visual flickering compared to full replacement.
fn update_text(text_inserter: &TextInserter, old_text: &str, new_text: &str) -> Result<()> {
    // 找到公共前缀长度（无需删除和重新输入的部分）
    let common_prefix_len = old_text
        .chars()
        .zip(new_text.chars())
        .take_while(|(a, b)| a == b)
        .count();

    // 计算需要删除的字符数 = 旧文本超出公共前缀的部分
    let chars_to_delete = old_text.chars().count() - common_prefix_len;

    // 需要追加的文本 = 新文本超出公共前缀的部分
    let text_to_append: String = new_text.chars().skip(common_prefix_len).collect();

    // 执行增量更新
    if chars_to_delete > 0 {
        text_inserter.delete_chars(chars_to_delete)?;
    }
    if !text_to_append.is_empty() {
        text_inserter.insert(&text_to_append)?;
    }

    tracing::debug!(
        "Updated text incrementally: '{}' -> '{}' (kept {} chars, deleted {}, appended '{}')",
        old_text,
        new_text,
        common_prefix_len,
        chars_to_delete,
        text_to_append
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{asr_finish_timeout, ASR_SESSION_FINISH_TIMEOUT, EMPTY_ASR_SESSION_FINISH_TIMEOUT};

    #[test]
    fn empty_asr_session_has_a_short_finish_budget() {
        assert_eq!(asr_finish_timeout(false), EMPTY_ASR_SESSION_FINISH_TIMEOUT);
        assert_eq!(asr_finish_timeout(true), ASR_SESSION_FINISH_TIMEOUT);
        assert!(EMPTY_ASR_SESSION_FINISH_TIMEOUT < ASR_SESSION_FINISH_TIMEOUT);
    }
}
