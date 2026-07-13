//! Conservative local microphone preprocessing before Opus encoding.

use crate::data::AudioProcessingConfig;

const FRAME_DURATION_MS: u32 = 20;
// Kept deliberately low so quiet speech is not mistaken for background noise.
const VOICE_RMS_THRESHOLD: f32 = 0.002;
const VOICE_PEAK_THRESHOLD: f32 = 0.008;
const OUTPUT_LIMIT: f32 = 0.98;

pub(super) struct AudioProcessor {
    config: AudioProcessingConfig,
    hangover_frames: u32,
    remaining_hangover_frames: u32,
}

impl AudioProcessor {
    pub(super) fn new(config: AudioProcessingConfig) -> Self {
        let hangover_frames = config.end_smooth_window_ms.div_ceil(FRAME_DURATION_MS);
        Self {
            config,
            hangover_frames,
            remaining_hangover_frames: 0,
        }
    }

    pub(super) fn process(&mut self, samples: &mut [f32]) -> bool {
        if samples.is_empty() {
            return false;
        }

        let (square_sum, peak) = samples.iter().fold((0.0_f32, 0.0_f32), |acc, sample| {
            (acc.0 + sample * sample, acc.1.max(sample.abs()))
        });
        let rms = (square_sum / samples.len() as f32).sqrt();
        let voice_detected = rms >= VOICE_RMS_THRESHOLD || peak >= VOICE_PEAK_THRESHOLD;

        let active = if !self.config.vad_enabled {
            true
        } else if voice_detected {
            self.remaining_hangover_frames = self.hangover_frames;
            true
        } else if self.remaining_hangover_frames > 0 {
            self.remaining_hangover_frames -= 1;
            true
        } else {
            false
        };

        // The cloud ASR service owns segmentation. Local VAD is observational,
        // therefore quiet frames keep both their content and cadence.
        for sample in samples {
            *sample = (*sample * self.config.post_ratio_gain).clamp(-OUTPUT_LIMIT, OUTPUT_LIMIT);
        }

        active
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioProcessor, OUTPUT_LIMIT};
    use crate::data::AudioProcessingConfig;

    fn config(vad_enabled: bool, end_smooth_window_ms: u32, gain: f32) -> AudioProcessingConfig {
        AudioProcessingConfig {
            vad_enabled,
            aec_enabled: false,
            end_smooth_window_ms,
            post_ratio_gain: gain,
        }
    }

    #[test]
    fn vad_reports_background_without_changing_audio() {
        let mut processor = AudioProcessor::new(config(true, 800, 1.0));
        let mut frame = vec![0.0005; 320];
        assert!(!processor.process(&mut frame));
        assert!(frame.iter().all(|sample| *sample == 0.0005));
    }

    #[test]
    fn end_smoothing_keeps_exact_hangover_window() {
        let mut processor = AudioProcessor::new(config(true, 40, 1.0));
        let mut speech = vec![0.02; 320];
        assert!(processor.process(&mut speech));

        for _ in 0..2 {
            let mut quiet = vec![0.0005; 320];
            assert!(processor.process(&mut quiet));
            assert!(quiet.iter().all(|sample| *sample == 0.0005));
        }

        let mut after_window = vec![0.0005; 320];
        assert!(!processor.process(&mut after_window));
        assert!(after_window.iter().all(|sample| *sample == 0.0005));
    }

    #[test]
    fn gain_is_applied_and_output_is_limited() {
        let mut processor = AudioProcessor::new(config(false, 800, 2.0));
        let mut frame = vec![0.25, -0.75];
        assert!(processor.process(&mut frame));
        assert_eq!(frame[0], 0.5);
        assert_eq!(frame[1], -OUTPUT_LIMIT);
    }

    #[test]
    fn disabled_vad_never_gates_quiet_audio() {
        let mut processor = AudioProcessor::new(config(false, 800, 1.0));
        let mut frame = vec![0.0001; 320];
        assert!(processor.process(&mut frame));
        assert!(frame.iter().all(|sample| *sample != 0.0));
    }
}
