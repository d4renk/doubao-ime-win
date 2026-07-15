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
