//! Acoustic echo cancellation backed by a real speaker loopback reference.

use anyhow::{bail, Context, Result};
use sonora::config::EchoCanceller;
use sonora::{AudioProcessing, Config, StreamConfig};

pub const AEC_SAMPLE_RATE: u32 = 48_000;
pub const AEC_FRAME_SAMPLES: usize = (AEC_SAMPLE_RATE / 100) as usize;

/// WebRTC AEC3 processor for 10 ms, 48 kHz mono frames.
pub struct AecProcessor {
    processor: AudioProcessing,
}

impl AecProcessor {
    pub fn new(stream_delay_ms: i32) -> Result<Self> {
        let stream = StreamConfig::new(AEC_SAMPLE_RATE, 1);
        let config = Config {
            echo_canceller: Some(EchoCanceller::default()),
            ..Config::default()
        };
        let mut processor = AudioProcessing::builder()
            .config(config)
            .capture_config(stream)
            .render_config(stream)
            .echo_detector(true)
            .build();
        processor
            .set_stream_delay_ms(stream_delay_ms)
            .map_err(|error| anyhow::anyhow!("invalid AEC stream delay: {error}"))?;
        Ok(Self { processor })
    }

    /// Feed one far-end frame captured from the active speaker endpoint.
    pub fn analyze_render(&mut self, render: &[f32]) -> Result<()> {
        validate_frame(render)?;
        let mut output = [0.0; AEC_FRAME_SAMPLES];
        self.processor
            .process_render_f32(&[render], &mut [&mut output])
            .map_err(|error| anyhow::anyhow!("AEC render processing failed: {error}"))
    }

    /// Remove the learned far-end echo from one near-end microphone frame.
    pub fn process_capture(&mut self, capture: &[f32]) -> Result<Vec<f32>> {
        validate_frame(capture)?;
        let mut output = vec![0.0; AEC_FRAME_SAMPLES];
        self.processor
            .process_capture_f32(&[capture], &mut [&mut output])
            .map_err(|error| anyhow::anyhow!("AEC capture processing failed: {error}"))?;
        Ok(output)
    }
}

fn validate_frame(frame: &[f32]) -> Result<()> {
    if frame.len() != AEC_FRAME_SAMPLES {
        bail!(
            "AEC requires {} samples (10 ms at {} Hz), got {}",
            AEC_FRAME_SAMPLES,
            AEC_SAMPLE_RATE,
            frame.len()
        );
    }
    Ok(())
}

/// Owns a WASAPI loopback capture thread and yields 10 ms mono render frames.
pub struct LoopbackReference {
    receiver: std::sync::mpsc::Receiver<Vec<f32>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LoopbackReference {
    #[cfg(target_os = "windows")]
    pub fn start() -> Result<Self> {
        use std::sync::atomic::AtomicBool;
        use std::sync::{mpsc, Arc};

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let (frame_tx, frame_rx) = mpsc::sync_channel(20);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("aec-loopback".into())
            .spawn(move || run_loopback(frame_tx, ready_tx, thread_stop))
            .context("failed to start WASAPI loopback thread")?;

        match ready_rx.recv_timeout(std::time::Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Self {
                receiver: frame_rx,
                stop,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                stop.store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = thread.join();
                Err(anyhow::anyhow!(error))
            }
            Err(error) => {
                stop.store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = thread.join();
                Err(anyhow::anyhow!(
                    "WASAPI loopback initialization timed out: {error}"
                ))
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub fn start() -> Result<Self> {
        bail!("WASAPI loopback is only available on Windows")
    }

    pub fn try_recv(&self) -> Option<Vec<f32>> {
        self.receiver.try_recv().ok()
    }

    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Option<Vec<f32>> {
        self.receiver.recv_timeout(timeout).ok()
    }
}

impl Drop for LoopbackReference {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(target_os = "windows")]
fn run_loopback(
    sender: std::sync::mpsc::SyncSender<Vec<f32>>,
    ready: std::sync::mpsc::SyncSender<std::result::Result<(), String>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    if let Err(error) = run_loopback_inner(sender, ready.clone(), stop) {
        let _ = ready.try_send(Err(format!("{error:#}")));
        tracing::warn!("WASAPI AEC loopback stopped: {error}");
    }
}

#[cfg(target_os = "windows")]
fn run_loopback_inner(
    sender: std::sync::mpsc::SyncSender<Vec<f32>>,
    ready: std::sync::mpsc::SyncSender<std::result::Result<(), String>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    use std::collections::VecDeque;
    use std::sync::atomic::Ordering;
    use wasapi::{initialize_mta, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

    initialize_mta()
        .ok()
        .context("failed to initialize COM for WASAPI loopback")?;
    let enumerator =
        DeviceEnumerator::new().context("failed to create WASAPI device enumerator")?;
    let device = enumerator
        .get_default_device(&Direction::Render)
        .context("no default render device for AEC reference")?;
    let mut client = device
        .get_iaudioclient()
        .context("failed to open default render device for loopback")?;
    let format = WaveFormat::new(
        32,
        32,
        &SampleType::Float,
        AEC_SAMPLE_RATE as usize,
        1,
        None,
    );
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: 200_000,
    };
    client
        .initialize_client(&format, &Direction::Capture, &mode)
        .context("failed to initialize WASAPI speaker loopback")?;
    let event = client
        .set_get_eventhandle()
        .context("failed to create WASAPI loopback event")?;
    let capture = client
        .get_audiocaptureclient()
        .context("failed to create WASAPI loopback capture client")?;
    client
        .start_stream()
        .context("failed to start WASAPI speaker loopback")?;
    ready
        .send(Ok(()))
        .map_err(|_| anyhow::anyhow!("AEC loopback owner stopped during initialization"))?;

    let mut bytes = VecDeque::with_capacity(AEC_FRAME_SAMPLES * 4 * 4);
    while !stop.load(Ordering::SeqCst) {
        let _ = event.wait_for_event(50);
        capture
            .read_from_device_to_deque(&mut bytes)
            .context("failed to read WASAPI loopback samples")?;
        while bytes.len() >= AEC_FRAME_SAMPLES * 4 {
            let mut frame = Vec::with_capacity(AEC_FRAME_SAMPLES);
            for _ in 0..AEC_FRAME_SAMPLES {
                let raw = [
                    bytes.pop_front().unwrap(),
                    bytes.pop_front().unwrap(),
                    bytes.pop_front().unwrap(),
                    bytes.pop_front().unwrap(),
                ];
                frame.push(f32::from_le_bytes(raw).clamp(-1.0, 1.0));
            }
            match sender.try_send(frame) {
                Ok(()) => {}
                Err(std::sync::mpsc::TrySendError::Full(_)) => {
                    // A stale render reference is worse than a dropped one for AEC delay
                    // estimation. Bound both the channel and the local byte backlog.
                    bytes.clear();
                    break;
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => return Ok(()),
            }
        }
    }
    client
        .stop_stream()
        .context("failed to stop WASAPI speaker loopback")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{AecProcessor, AEC_FRAME_SAMPLES};
    use std::collections::VecDeque;

    #[test]
    fn rejects_non_ten_millisecond_frames() {
        let mut processor = AecProcessor::new(0).unwrap();
        assert!(processor.analyze_render(&[0.0; 100]).is_err());
        assert!(processor.process_capture(&[0.0; 100]).is_err());
    }

    #[test]
    fn processes_render_and_capture_frames() {
        let mut processor = AecProcessor::new(0).unwrap();
        let silence = [0.0; AEC_FRAME_SAMPLES];
        processor.analyze_render(&silence).unwrap();
        let output = processor.process_capture(&silence).unwrap();
        assert_eq!(output.len(), AEC_FRAME_SAMPLES);
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn attenuates_a_delayed_synthetic_echo() {
        let mut processor = AecProcessor::new(50).unwrap();
        let mut delay = VecDeque::from(vec![vec![0.0; AEC_FRAME_SAMPLES]; 5]);
        let mut state = 0x1234_5678_u32;
        let mut input_energy = 0.0_f64;
        let mut output_energy = 0.0_f64;

        for frame_index in 0..500 {
            let render: Vec<f32> = (0..AEC_FRAME_SAMPLES)
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    ((state >> 8) as f32 / 16_777_216.0 - 0.5) * 0.5
                })
                .collect();
            processor.analyze_render(&render).unwrap();
            delay.push_back(render);
            let capture: Vec<f32> = delay
                .pop_front()
                .unwrap()
                .into_iter()
                .map(|sample| sample * 0.6)
                .collect();
            let output = processor.process_capture(&capture).unwrap();

            if frame_index >= 400 {
                input_energy += capture
                    .iter()
                    .map(|sample| f64::from(*sample).powi(2))
                    .sum::<f64>();
                output_energy += output
                    .iter()
                    .map(|sample| f64::from(*sample).powi(2))
                    .sum::<f64>();
            }
        }

        assert!(
            output_energy < input_energy * 0.8,
            "AEC did not converge: input={input_energy}, output={output_energy}"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires the local Windows default render endpoint"]
    fn opens_real_wasapi_loopback_reference() {
        let reference =
            super::LoopbackReference::start().unwrap_or_else(|error| panic!("{error:#}"));
        let frame = reference
            .recv_timeout(std::time::Duration::from_secs(3))
            .expect("default render endpoint produced no loopback frame");
        assert_eq!(frame.len(), AEC_FRAME_SAMPLES);
        assert!(frame.iter().all(|sample| sample.is_finite()));
        drop(reference);
    }
}
