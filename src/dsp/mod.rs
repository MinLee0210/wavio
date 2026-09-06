//! Digital Signal Processing pipeline.
//!
//! This module handles audio ingestion, FFT computation, spectrogram
//! generation, and peak detection.

pub mod audio;
pub mod peaks;
pub mod spectrogram;
pub mod fingerprint;
pub mod streaming;

pub use fingerprint::Fingerprinter;
pub use streaming::{StreamConfig, StreamingFingerprinter};
