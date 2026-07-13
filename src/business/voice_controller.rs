//! Voice Controller
//!
//! Coordinates voice input between audio capture, ASR, and text insertion.

use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

use crate::asr::{AsrClient, ResponseType};
use crate::audio::AudioCapture;
use crate::business::punctuation::{format_transcript, TranscriptBoundary};
use crate::business::TextInserter;
use crate::data::{AppConfig, PunctuationMode};

const FINAL_RESULT_TIMEOUT: Duration = Duration::from_secs(2);

/// Voice input controller
pub struct VoiceController {
    asr_client: Arc<AsrClient>,
    audio_capture: Arc<AudioCapture>,
    text_inserter: Arc<TextInserter>,
    is_recording: Arc<AtomicBool>,
    finish_requested: Arc<AtomicBool>,
    result_task: Option<JoinHandle<()>>,
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
            result_task: None,
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

        tracing::info!("Starting voice input...");
        self.finish_requested.store(false, Ordering::SeqCst);
        let asr_config = AppConfig::load_or_default()?.asr;
        let audio_quality = asr_config.audio_quality;
        let punctuation_mode = asr_config.punctuation_mode;

        // Start audio capture
        tracing::debug!("Starting audio capture...");
        let audio_rx = self.audio_capture.start(audio_quality)?;
        self.is_recording.store(true, Ordering::SeqCst);
        tracing::info!("Audio capture started, frames will be sent to ASR");

        // Start ASR
        tracing::debug!("Connecting to ASR server...");
        let mut result_rx = match self
            .asr_client
            .start_realtime(audio_rx, audio_quality)
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
        let audio_capture = self.audio_capture.clone();

        // Spawn result processing task
        self.result_task = Some(tokio::spawn(async move {
            let mut last_text = String::new();
            let mut response_count = 0u32;
            let mut completed_normally = false;
            let mut pending_smart_comma = false;

            tracing::info!("ASR result processing task started");

            while let Some(response) = result_rx.recv().await {
                response_count += 1;
                match response.response_type {
                    ResponseType::InterimResult => {
                        tracing::debug!("[INTERIM #{}] {}", response_count, response.text);
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
                            }
                            pending_smart_comma = punctuation_mode == PunctuationMode::Smart
                                && boundary == TranscriptBoundary::ClauseFinal
                                && displayed_text.ends_with('，');
                            // A later sentence starts a new incremental text range.
                            last_text = String::new();
                        }
                        if finish_requested.load(Ordering::SeqCst) {
                            tracing::info!("Final ASR result received while stopping");
                            completed_normally = true;
                            break;
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
                                }
                                last_text.clear();
                            } else if pending_smart_comma {
                                if let Err(error) = replace_last_character(&text_inserter, "。") {
                                    tracing::error!("Failed to finalize punctuation: {}", error);
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

            audio_capture.stop();
            is_recording.store(false, Ordering::SeqCst);
        }));

        Ok(())
    }

    /// Stop capturing audio and drain the server's final recognition result.
    pub async fn stop(&mut self) -> Result<()> {
        if !self.is_recording() {
            return Ok(());
        }

        tracing::info!("Stopping voice input and waiting for the final ASR result...");
        self.finish_requested.store(true, Ordering::SeqCst);
        self.audio_capture.stop();

        if let Some(mut result_task) = self.result_task.take() {
            match tokio::time::timeout(FINAL_RESULT_TIMEOUT, &mut result_task).await {
                Ok(Ok(())) => {
                    tracing::info!("Final ASR result processing completed");
                }
                Ok(Err(error)) => {
                    tracing::warn!("ASR result task failed: {}", error);
                }
                Err(_) => {
                    tracing::warn!(
                        "Timed out after {:?} waiting for the final ASR result",
                        FINAL_RESULT_TIMEOUT
                    );
                    result_task.abort();
                    let _ = result_task.await;
                }
            }
        }

        self.is_recording.store(false, Ordering::SeqCst);
        Ok(())
    }
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
