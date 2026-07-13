//! Audio capture and processing module

mod capture;
mod encoder;
mod resampler;

pub use capture::AudioCapture;
pub use encoder::OpusEncoder;
