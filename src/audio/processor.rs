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
