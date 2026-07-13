use anyhow::{Context, Result};
use rubato::{
    calculate_cutoff, Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
use std::collections::VecDeque;

const SINC_LENGTH: usize = 128;
const OVERSAMPLING_FACTOR: usize = 256;

/// Stateful mono resampler that preserves filter history across capture chunks.
pub struct AudioResampler {
    resampler: Option<SincFixedIn<f32>>,
    input_frames: usize,
    output_frame_size: usize,
    output_buffer: VecDeque<f32>,
}

impl AudioResampler {
    pub fn new(
        input_sample_rate: u32,
        output_sample_rate: u32,
        input_frames: usize,
        output_frame_size: usize,
    ) -> Result<Self> {
        let resampler = if input_sample_rate == output_sample_rate {
            None
        } else {
            let parameters = SincInterpolationParameters {
                sinc_len: SINC_LENGTH,
                f_cutoff: calculate_cutoff(SINC_LENGTH, WindowFunction::Blackman2),
                interpolation: SincInterpolationType::Quadratic,
                oversampling_factor: OVERSAMPLING_FACTOR,
                window: WindowFunction::Blackman2,
            };

            Some(
                SincFixedIn::<f32>::new(
                    output_sample_rate as f64 / input_sample_rate as f64,
                    1.0,
                    parameters,
                    input_frames,
                    1,
                )
                .context("failed to create audio resampler")?,
            )
        };

        Ok(Self {
            resampler,
            input_frames,
            output_frame_size,
            output_buffer: VecDeque::with_capacity(output_frame_size * 2),
        })
    }

    /// Process one native-rate mono chunk and return zero or more exact-size output frames.
    pub fn process(&mut self, input: &[f32]) -> Result<Vec<Vec<f32>>> {
        if input.len() != self.input_frames {
            anyhow::bail!(
                "invalid resampler input size: got {}, expected {}",
                input.len(),
                self.input_frames
            );
        }

        if let Some(resampler) = self.resampler.as_mut() {
            let output = resampler
                .process(&[input], None)
                .context("audio resampling failed")?;
            self.output_buffer.extend(output[0].iter().copied());
        } else {
            self.output_buffer.extend(input.iter().copied());
        }

        let frame_count = self.output_buffer.len() / self.output_frame_size;
        let mut frames = Vec::with_capacity(frame_count);
        for _ in 0..frame_count {
            frames.push(self.output_buffer.drain(..self.output_frame_size).collect());
        }
        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::AudioResampler;
    use std::f32::consts::TAU;

    const INPUT_RATE: u32 = 48_000;
    const OUTPUT_RATE: u32 = 16_000;
    const INPUT_FRAMES: usize = 960;
    const OUTPUT_FRAMES: usize = 320;

    fn resample_tone(frequency: f32) -> Vec<f32> {
        let mut resampler =
            AudioResampler::new(INPUT_RATE, OUTPUT_RATE, INPUT_FRAMES, OUTPUT_FRAMES).unwrap();
        let mut output = Vec::new();

        for chunk_index in 0..100 {
            let input: Vec<f32> = (0..INPUT_FRAMES)
                .map(|sample_index| {
                    let absolute_index = chunk_index * INPUT_FRAMES + sample_index;
                    (TAU * frequency * absolute_index as f32 / INPUT_RATE as f32).sin()
                })
                .collect();

            for frame in resampler.process(&input).unwrap() {
                assert_eq!(frame.len(), OUTPUT_FRAMES);
                output.extend(frame);
            }
        }
        output
    }

    fn steady_state_rms(samples: &[f32]) -> f32 {
        let settled = &samples[samples.len() / 2..];
        (settled.iter().map(|sample| sample * sample).sum::<f32>() / settled.len() as f32).sqrt()
    }

    #[test]
    fn emits_only_complete_opus_frames() {
        let output = resample_tone(1_000.0);
        assert!(!output.is_empty());
        assert_eq!(output.len() % OUTPUT_FRAMES, 0);
    }

    #[test]
    fn preserves_speech_band_signal() {
        let rms = steady_state_rms(&resample_tone(1_000.0));
        assert!(rms > 0.6, "1kHz RMS was unexpectedly low: {rms}");
    }

    #[test]
    fn attenuates_frequencies_above_output_nyquist() {
        let passband_rms = steady_state_rms(&resample_tone(1_000.0));
        let stopband_rms = steady_state_rms(&resample_tone(12_000.0));
        assert!(
            stopband_rms < passband_rms * 0.02,
            "12kHz signal was not sufficiently attenuated: passband={passband_rms}, stopband={stopband_rms}"
        );
    }

    #[test]
    fn supports_24khz_high_quality_frames() {
        let mut resampler = AudioResampler::new(INPUT_RATE, 24_000, INPUT_FRAMES, 480).unwrap();
        let input = vec![0.0; INPUT_FRAMES];
        let mut emitted_frames = 0;

        for _ in 0..10 {
            for frame in resampler.process(&input).unwrap() {
                assert_eq!(frame.len(), 480);
                emitted_frames += 1;
            }
        }
        assert!(emitted_frames > 0);
    }
}
