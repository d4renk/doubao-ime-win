//! Audio Capture using cpal

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
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
    meter_level: Arc<AtomicU64>,
}

impl AudioCapture {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        match host.default_input_device() {
            Some(device) => {
                println!(
                    "[AudioCapture] Default device: {}",
                    device
                        .description()
                        .map(|description| description.name().to_owned())
                        .unwrap_or_else(|_| "Unknown".to_owned())
                );
            }
            None => {
                println!("[AudioCapture] WARNING: No default input device found.");
            }
        }

        Ok(Self {
            is_recording: Arc::new(AtomicBool::new(false)),
            meter_level: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    /// Current normalized microphone level for display-only UI feedback.
    pub fn meter_level(&self) -> f32 {
        f32::from_bits(self.meter_level.load(Ordering::Relaxed) as u32)
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
        let meter_level = self.meter_level.clone();

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
                    meter_level,
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
    meter_level: Arc<AtomicU64>,
    audio_quality: AudioQuality,
    processing_config: AudioProcessingConfig,
) -> Result<()> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("No input device available"))?;

    println!(
        "[AudioCapture] Device: {}",
        device
            .description()
            .map(|description| description.name().to_owned())
            .unwrap_or_else(|_| "Unknown".to_owned())
    );

    // Get the device's default config - USE THIS EXACTLY
    let supported_config = device.default_input_config()?;
    println!("[AudioCapture] Device config: {:?}", supported_config);

    let native_sample_rate = supported_config.sample_rate();
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

    let stream = match sample_format {
        SampleFormat::I8 => build_pcm_input_stream::<i8>(
            &device,
            config,
            samples_per_frame_native,
            is_recording_clone,
            std_tx,
        )?,
        SampleFormat::I16 => build_pcm_input_stream::<i16>(
            &device,
            config,
            samples_per_frame_native,
            is_recording_clone,
            std_tx,
        )?,
        SampleFormat::I24 => build_pcm_input_stream::<cpal::I24>(
            &device,
            config,
            samples_per_frame_native,
            is_recording_clone,
            std_tx,
        )?,
        SampleFormat::I32 => build_pcm_input_stream::<i32>(
            &device,
            config,
            samples_per_frame_native,
            is_recording_clone,
            std_tx,
        )?,
        SampleFormat::I64 => build_pcm_input_stream::<i64>(
            &device,
            config,
            samples_per_frame_native,
            is_recording_clone,
            std_tx,
        )?,
        SampleFormat::U8 => build_pcm_input_stream::<u8>(
            &device,
            config,
            samples_per_frame_native,
            is_recording_clone,
            std_tx,
        )?,
        SampleFormat::U16 => build_pcm_input_stream::<u16>(
            &device,
            config,
            samples_per_frame_native,
            is_recording_clone,
            std_tx,
        )?,
        SampleFormat::U24 => build_pcm_input_stream::<cpal::U24>(
            &device,
            config,
            samples_per_frame_native,
            is_recording_clone,
            std_tx,
        )?,
        SampleFormat::U32 => build_pcm_input_stream::<u32>(
            &device,
            config,
            samples_per_frame_native,
            is_recording_clone,
            std_tx,
        )?,
        SampleFormat::U64 => build_pcm_input_stream::<u64>(
            &device,
            config,
            samples_per_frame_native,
            is_recording_clone,
            std_tx,
        )?,
        SampleFormat::F32 => build_pcm_input_stream::<f32>(
            &device,
            config,
            samples_per_frame_native,
            is_recording_clone,
            std_tx,
        )?,
        SampleFormat::F64 => build_pcm_input_stream::<f64>(
            &device,
            config,
            samples_per_frame_native,
            is_recording_clone,
            std_tx,
        )?,
        format => return Err(anyhow!("Unsupported non-PCM sample format: {format}")),
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

                    let rms = (echo_cancelled
                        .iter()
                        .map(|sample| sample * sample)
                        .sum::<f32>()
                        / echo_cancelled.len().max(1) as f32)
                        .sqrt();
                    meter_level.store(
                        (rms * 12.0).clamp(0.0, 1.0).to_bits() as u64,
                        Ordering::Relaxed,
                    );

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
    meter_level.store(0, Ordering::Relaxed);

    Ok(())
}

fn build_pcm_input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    samples_per_frame: usize,
    is_recording: Arc<AtomicBool>,
    sender: std_mpsc::Sender<Vec<f32>>,
) -> Result<cpal::Stream>
where
    T: Sample + SizedSample,
    f32: FromSample<T>,
{
    let mut buffer = Vec::<f32>::with_capacity(samples_per_frame * 2);
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            if !is_recording.load(Ordering::SeqCst) {
                return;
            }
            buffer.extend(data.iter().map(|sample| sample.to_sample::<f32>()));
            while buffer.len() >= samples_per_frame {
                let frame = buffer.drain(..samples_per_frame).collect();
                let _ = sender.send(frame);
            }
        },
        |error| println!("[AudioCapture] Stream error: {error}"),
        None,
    )?)
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
