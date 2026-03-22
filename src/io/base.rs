//! Base I/O trait for reading audio samples.
//!
//! Defines the [`IOReader`] trait that all audio source backends implement.

use crate::error::WavioError;

/// Trait for reading audio data from a source into normalized `f32` samples.
pub trait IOReader {
    /// Reads audio data and returns a vector of `f32` samples.
    ///
    /// # Errors
    ///
    /// Returns [`WavioError`] if the source cannot be read or decoded.
    fn read(&self) -> Result<Vec<f32>, WavioError>;
}