//! Unified error types for the wavio crate.

use thiserror::Error;

/// Represents all possible errors that can occur in the wavio library.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WavioError {
    /// The specified file does not exist.
    #[error("File not found: {0}")]
    FileNotFound(String),
    /// The file exists but remains inaccessible due to permissions or locking.
    #[error("File not readable: {0}")]
    FileNotReadable(String),
    /// The file is not a valid WAV file or is corrupted.
    #[error("Invalid WAV format: {0}")]
    InvalidWavFormat(String),
    /// The audio format (e.g., sample rate or bit depth) is not supported by the DSP pipeline.
    #[error("Unsupported audio format: {0}")]
    UnsupportedAudioFormat(String),
    /// The audio sample buffer is too short to generate a meaningful spectrogram.
    #[error("Audio data is too short for fingerprinting")]
    AudioTooShort,
    /// An error occurred during the Fast Fourier Transform (FFT) computation.
    #[error("FFT error: {0}")]
    FftError(String),
    /// The spectrogram generation failed (e.g., invalid window size or overlap).
    #[error("Spectrogram error: {0}")]
    SpectrogramError(String),
    /// No peaks were detected in the audio, making fingerprinting impossible.
    #[error("No constellation peaks found in audio")]
    NoPeaksFound,
    /// An error occurred during combinatorial hashing of peak pairs.
    #[error("Hashing error: {0}")]
    HashingError(String),
    /// Failure while reading from or writing to the fingerprint index/database.
    #[error("Index error: {0}")]
    IndexError(String),
    /// A general I/O error occurred during file operations.
    #[error("I/O error: {0}")]
    IoError(String),
}