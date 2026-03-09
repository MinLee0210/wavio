//! Spectrogram generation.
//!
//! Sliding-window FFT over PCM samples to produce a time-frequency
//! power spectrogram stored as `ndarray::Array2<f32>`.
