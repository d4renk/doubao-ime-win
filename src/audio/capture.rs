//! Audio Capture using cpal

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::thread;
use tokio::sync::mpsc as tokio_mpsc;

use super::aec::{AecProcessor, LoopbackReference, AEC_FRAME_SAMPLES, AEC_SAMPLE_RATE};
use super::encoder::OpusEncoder;
use super::processor::AudioProcessor;
use super::resampler::AudioResampler;
use crate::data::{AudioProcessingConfig, AudioQuality};

// The ASR service is verified with mono Opus at 16kHz and 24kHz.
const OPUS_CHANNELS: u16 = 1;
const FRAME_DURATION_MS: u32 = 20;
const AEC_FRAMES_PER_CAPTURE_FRAME: usize = 2;

pub struct AudioCapture {
    is_recording: Arc<AtomicBool>,
}

impl AudioCapture {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        match host.default_input_device() {
            Some(device) => {
                println!(
                    "[AudioCapture] Default device: {}",
                    device.name().unwrap_or_default()
                );
            }
            None => {
                println!("[AudioCapture] WARNING: No default input device found.");
            }
        }

        Ok(Self {
            is_recording: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    pub fn start(
        &self,
        audio_quality: AudioQuality,
        processing_config: AudioProcessingConfig,
    ) -> Result<tokio_mpsc::Receiver<Vec<u8>>> {
        if self.is_recording.swap(true, Ordering::SeqCst) {
            return Err(anyhow!("Already recording"));
        }

        let (tokio_tx, tokio_rx) = tokio_mpsc::channel::<Vec<u8>>(100);
        let is_recording = self.is_recording.clone();

        thread::spawn(move || {
            #[cfg(target_os = "windows")]
            {
                use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                }
                println!("[AudioCapture] COM initialized");
            }

            println!("[AudioCapture] >>> Thread spawned <<<");
            use std::io::Write;
            let _ = std::io::stdout().flush();

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_audio_capture(
                    tokio_tx,
                    is_recording.clone(),
                    audio_quality,
                    processing_config,
                )
            }));

            match result {
                Ok(Ok(_)) => {
                    println!("[AudioCapture] Completed normally");
                }
                Ok(Err(e)) => {
                    println!("[AudioCapture] ERROR: {}", e);
                }
                Err(panic_info) => {
                    println!("[AudioCapture] PANIC: {:?}", panic_info);
                }
            }

            is_recording.store(false, Ordering::SeqCst);
            println!("[AudioCapture] Thread exiting");
            let _ = std::io::stdout().flush();
        });

        tracing::info!("Audio capture started");
        Ok(tokio_rx)
    }

    pub fn stop(&self) {
        self.is_recording.store(false, Ordering::SeqCst);
        tracing::info!("Audio capture stopped");
    }
}

fn run_audio_capture(
    tokio_tx: tokio_mpsc::Sender<Vec<u8>>,
    is_recording: Arc<AtomicBool>,
    audio_quality: AudioQuality,
    processing_config: AudioProcessingConfig,
) -> Result<()> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("No input device available"))?;

    println!(
        "[AudioCapture] Device: {}",
        device.name().unwrap_or_default()
    );

    // Get the device's default config - USE THIS EXACTLY
    let supported_config = device.default_input_config()?;
    println!("[AudioCapture] Device config: {:?}", supported_config);

    let native_sample_rate = supported_config.sample_rate().0;
    let native_channels = supported_config.channels();
    let sample_format = supported_config.sample_format();

    println!(
        "[AudioCapture] Native: {}Hz, {} channels, {:?}",
        native_sample_rate, native_channels, sample_format
    );

    // Use the device's EXACT config (don't override channels!)
    let config = supported_config.config();
    println!("[AudioCapture] Using config: {:?}", config);

    let opus_sample_rate = audio_quality.sample_rate();

    // Create Opus encoder using the same profile declared to the ASR service.
    let mut encoder = match OpusEncoder::new(opus_sample_rate, OPUS_CHANNELS) {
        Ok(enc) => {
            println!(
                "[AudioCapture] Opus encoder created ({}Hz mono)",
                opus_sample_rate
            );
            enc
        }
        Err(e) => {
            println!("[AudioCapture] Opus encoder FAILED: {}", e);
            return Err(e);
        }
    };

    // Calculate frame sizes
    let samples_per_frame_native =
        (native_sample_rate * FRAME_DURATION_MS / 1000) as usize * native_channels as usize;
    let samples_per_frame_opus = (opus_sample_rate * FRAME_DURATION_MS / 1000) as usize; // mono

    println!(
        "[AudioCapture] Samples/frame: native={} ({}ch), opus={} (mono)",
        samples_per_frame_native, native_channels, samples_per_frame_opus
    );

    let (std_tx, std_rx) = std_mpsc::channel::<Vec<f32>>();

    let is_recording_clone = is_recording.clone();
    let frame_counter = Arc::new(AtomicU64::new(0));
    let frame_counter_clone = frame_counter.clone();
    let native_channels_clone = native_channels;

    let err_fn = |err| {
        println!("[AudioCapture] Stream error: {}", err);
    };

    let stream = match sample_format {
        SampleFormat::I16 => {
            println!("[AudioCapture] Building I16 stream");
            let mut buffer = Vec::<f32>::with_capacity(samples_per_frame_native * 2);

            device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if !is_recording_clone.load(Ordering::SeqCst) {
                        return;
                    }

                    buffer.extend(data.iter().map(|sample| *sample as f32 / 32768.0));

                    while buffer.len() >= samples_per_frame_native {
                        let frame: Vec<f32> = buffer.drain(..samples_per_frame_native).collect();
                        let _ = std_tx.send(frame);
                    }
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::F32 => {
            println!("[AudioCapture] Building F32 stream");
            let mut buffer = Vec::<f32>::with_capacity(samples_per_frame_native * 2);

            device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if !is_recording_clone.load(Ordering::SeqCst) {
                        return;
                    }

                    buffer.extend_from_slice(data);

                    while buffer.len() >= samples_per_frame_native {
                        let frame: Vec<f32> = buffer.drain(..samples_per_frame_native).collect();
                        let _ = std_tx.send(frame);
                    }
                },
                err_fn,
                None,
            )?
        }
        format => {
            return Err(anyhow!("Unsupported format: {:?}", format));
        }
    };

    stream.play()?;
    println!("[AudioCapture] Stream playing!");
    println!("[Mic] Recording started...");

    let mono_samples_per_native_frame = samples_per_frame_native / native_channels_clone as usize;
    let mut aec_runtime = if processing_config.aec_enabled {
        match LoopbackReference::start()
            .and_then(|reference| AecProcessor::new(0).map(|processor| (processor, reference)))
        {
            Ok(runtime) => {
                tracing::info!("AEC3 enabled with WASAPI speaker loopback reference");
                Some(runtime)
            }
            Err(error) => {
                tracing::warn!("AEC3 unavailable; continuing without echo cancellation: {error:#}");
                None
            }
        }
    } else {
        None
    };
    let aec_active = aec_runtime.is_some();
    let processing_sample_rate = if aec_active {
        AEC_SAMPLE_RATE
    } else {
        native_sample_rate
    };
    let processing_frame_size = if aec_active {
        AEC_FRAME_SAMPLES * AEC_FRAMES_PER_CAPTURE_FRAME
    } else {
        mono_samples_per_native_frame
    };
    let mut aec_input_resampler = if aec_active {
        Some(AudioResampler::new(
            native_sample_rate,
            AEC_SAMPLE_RATE,
            mono_samples_per_native_frame,
            processing_frame_size,
        )?)
    } else {
        None
    };
    let mut resampler = AudioResampler::new(
        processing_sample_rate,
        opus_sample_rate,
        processing_frame_size,
        samples_per_frame_opus,
    )?;
    let mut processor = AudioProcessor::new(processing_config);
    let mut last_voice_activity = None;

    // Process frames: convert to mono 16kHz and encode
    while is_recording.load(Ordering::SeqCst) {
        match std_rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(frame) => {
                // Step 1: Convert stereo to mono (if needed)
                let mono_frame: Vec<f32> = if native_channels_clone > 1 {
                    // Average channels
                    frame
                        .chunks(native_channels_clone as usize)
                        .map(|chunk| chunk.iter().sum::<f32>() / native_channels_clone as f32)
                        .collect()
                } else {
                    frame
                };

                let processing_frames = if let Some(input_resampler) = aec_input_resampler.as_mut()
                {
                    input_resampler.process(&mono_frame)?
                } else {
                    vec![mono_frame]
                };

                for processing_frame in processing_frames {
                    let aec_result = aec_runtime
                        .as_mut()
                        .map(|(aec, reference)| cancel_echo(aec, reference, &processing_frame));
                    let echo_cancelled = match aec_result {
                        Some(Ok(frame)) => frame,
                        Some(Err(error)) => {
                            tracing::warn!(
                                "AEC3 processing failed; disabling it for this session: {error:#}"
                            );
                            aec_runtime = None;
                            processing_frame
                        }
                        None => processing_frame,
                    };

                    // Continuously resample with anti-alias filtering. The resampler may
                    // return zero or multiple exact-size Opus input frames per chunk.
                    for mut resampled in resampler.process(&echo_cancelled)? {
                        let voice_activity = processor.process(&mut resampled);
                        if processing_config.vad_enabled
                            && last_voice_activity != Some(voice_activity)
                        {
                            tracing::debug!(active = voice_activity, "Local VAD state changed");
                            last_voice_activity = Some(voice_activity);
                        }
                        let pcm_bytes: Vec<u8> = resampled
                            .iter()
                            .flat_map(|sample| {
                                let pcm =
                                    (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
                                pcm.to_le_bytes()
                            })
                            .collect();

                        match encoder.encode(&pcm_bytes) {
                            Ok(opus_frame) => {
                                let count = frame_counter_clone.fetch_add(1, Ordering::SeqCst);
                                if count == 0 {
                                    println!("[Audio] First frame captured and encoded!");
                                }
                                if count > 0 && count % 50 == 0 {
                                    println!(
                                        "[AudioCapture] Frames: {} ({:.1}s)",
                                        count,
                                        count as f32 * 0.02
                                    );
                                }

                                if tokio_tx.try_send(opus_frame).is_err() {
                                    println!("[AudioCapture] Channel full, dropping frame");
                                }
                            }
                            Err(e) => {
                                if frame_counter_clone.load(Ordering::SeqCst) == 0 {
                                    println!("[AudioCapture] First encode error: {}", e);
                                }
                            }
                        }
                    }
                }
            }
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                // Normal timeout
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                println!("[AudioCapture] Channel disconnected");
                break;
            }
        }
    }

    let total = frame_counter.load(Ordering::SeqCst);
    println!("[AudioCapture] Total frames: {}", total);
    println!(
        "[Mic] Stopped. {} frames ({:.1}s)",
        total,
        total as f32 * 0.02
    );

    Ok(())
}

fn cancel_echo(
    processor: &mut AecProcessor,
    reference: &LoopbackReference,
    capture: &[f32],
) -> Result<Vec<f32>> {
    if capture.len() != AEC_FRAME_SAMPLES * AEC_FRAMES_PER_CAPTURE_FRAME {
        return Err(anyhow!(
            "AEC input must contain {} samples, got {}",
            AEC_FRAME_SAMPLES * AEC_FRAMES_PER_CAPTURE_FRAME,
            capture.len()
        ));
    }

    let mut render_frames = 0;
    while let Some(render) = reference.try_recv() {
        processor.analyze_render(&render)?;
        render_frames += 1;
    }
    let silence = [0.0; AEC_FRAME_SAMPLES];
    while render_frames < AEC_FRAMES_PER_CAPTURE_FRAME {
        processor.analyze_render(&silence)?;
        render_frames += 1;
    }

    let mut output = Vec::with_capacity(capture.len());
    for frame in capture.chunks_exact(AEC_FRAME_SAMPLES) {
        output.extend(processor.process_capture(frame)?);
    }
    Ok(output)
}

#[cfg(all(test, target_os = "windows"))]
mod hardware_tests {
    use super::AudioCapture;
    use crate::data::{AudioProcessingConfig, AudioQuality};

    #[tokio::test]
    #[ignore = "requires the local Windows microphone and default render endpoint"]
    async fn production_capture_pipeline_emits_opus_with_aec() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();
        let capture = AudioCapture::new().expect("default microphone must be available");
        let mut receiver = capture
            .start(
                AudioQuality::Standard,
                AudioProcessingConfig {
                    vad_enabled: true,
                    aec_enabled: true,
                    end_smooth_window_ms: 800,
                    post_ratio_gain: 1.0,
                },
            )
            .expect("AEC capture pipeline must start");

        let encoded = tokio::time::timeout(std::time::Duration::from_secs(5), receiver.recv())
            .await
            .expect("capture pipeline produced no frame within five seconds")
            .expect("capture pipeline closed before producing a frame");
        capture.stop();

        assert!(!encoded.is_empty());
    }
}
