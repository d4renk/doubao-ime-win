//! Audio capture and processing module

mod aec;
mod capture;
mod encoder;
mod processor;
mod resampler;

pub use aec::{AecProcessor, LoopbackReference, AEC_FRAME_SAMPLES, AEC_SAMPLE_RATE};
pub use capture::AudioCapture;
pub use encoder::OpusEncoder;
