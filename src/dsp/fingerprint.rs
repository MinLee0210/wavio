//! High-level audio fingerprinter pipeline coordination.

use crate::dsp::audio::load_audio;
use crate::dsp::peaks::PeakExtractorConfig;
#[cfg(not(feature = "parallel"))]
use crate::dsp::peaks::extract_peaks;
#[cfg(feature = "parallel")]
use crate::dsp::peaks::extract_peaks_parallel;

use crate::dsp::spectrogram::{compute_spectrogram, SpectrogramConfig};

use crate::hash::{Fingerprint, HashConfig};
#[cfg(not(feature = "parallel"))]
use crate::hash::generate_hashes;
#[cfg(feature = "parallel")]
use crate::hash::generate_hashes_parallel;
use crate::error::WavioError;

/// High-level orchestration engine for the audio fingerprinting pipeline.
///
/// Combines configurations for spectrogram generation, peak extraction,
/// and combinatorial hashing. Offers a single entry point to extract
/// fingerprints from samples in-memory or directly from files on disk.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Fingerprinter {
    /// Configuration for spectrogram generation.
    pub spectrogram_config: SpectrogramConfig,
    /// Configuration for peak detection.
    pub peak_config: PeakExtractorConfig,
    /// Configuration for combinatorial hashing.
    pub hash_config: HashConfig,
}

impl Fingerprinter {
    /// Creates a new `Fingerprinter` with custom configurations.
    #[must_use]
    pub fn new(
        spectrogram_config: SpectrogramConfig,
        peak_config: PeakExtractorConfig,
        hash_config: HashConfig,
    ) -> Self {
        Self {
            spectrogram_config,
            peak_config,
            hash_config,
        }
    }

    /// Generates audio fingerprints from normalized mono samples in-memory.
    ///
    /// Depending on feature flags and compile options, this automatically
    /// uses parallel processing if the `parallel` feature is active.
    ///
    /// # Errors
    ///
    /// Returns [`WavioError`] if spectrogram computation fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use wavio::dsp::Fingerprinter;
    ///
    /// let fingerprinter = Fingerprinter::default();
    /// let samples = vec![0.0; 22050]; // 1 second of silence
    /// let fingerprints = fingerprinter.fingerprint(&samples).unwrap();
    /// // Silence should produce zero fingerprints
    /// assert!(fingerprints.is_empty());
    /// ```
    pub fn fingerprint(&self, samples: &[f32]) -> Result<Vec<Fingerprint>, WavioError> {
        let spec = compute_spectrogram(samples, &self.spectrogram_config)?;

        #[cfg(feature = "parallel")]
        let peaks = extract_peaks_parallel(&spec, &self.peak_config);
        #[cfg(not(feature = "parallel"))]
        let peaks = extract_peaks(&spec, &self.peak_config);

        #[cfg(feature = "parallel")]
        let hashes = generate_hashes_parallel(&peaks, &self.hash_config);
        #[cfg(not(feature = "parallel"))]
        let hashes = generate_hashes(&peaks, &self.hash_config);

        Ok(hashes)
    }

    /// Loads an audio file from disk, processes it, and generates its fingerprints.
    ///
    /// Automatically decodes any supported format (WAV, MP3, FLAC, AAC, etc.,
    /// depending on whether the `symphonia` feature is active) and resamples
    /// to the standard internal sample rate.
    ///
    /// # Errors
    ///
    /// Returns [`WavioError`] if reading, decoding, or processing fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use wavio::dsp::Fingerprinter;
    ///
    /// let fingerprinter = Fingerprinter::default();
    /// let fingerprints = fingerprinter.fingerprint_file("track.mp3").unwrap();
    /// ```
    pub fn fingerprint_file(&self, path: &str) -> Result<Vec<Fingerprint>, WavioError> {
        let audio = load_audio(path)?;
        self.fingerprint(&audio.samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fingerprint_synthetic() {
        let samples = vec![0.0_f32; 1000];
        
        let spectrogram_config = SpectrogramConfig {
            window_size: 256,
            hop_size: 128,
        };
        let peak_config = PeakExtractorConfig {
            time_neighborhood: 2,
            freq_neighborhood: 2,
            threshold_db: -50.0,
            ..PeakExtractorConfig::default()
        };
        let hash_config = HashConfig::default();

        let fp = Fingerprinter::new(spectrogram_config, peak_config, hash_config);
        let result = fp.fingerprint(&samples);
        assert!(result.is_ok());
    }
}
