//! Audio ingestion and decoding.
//!
//! Handles WAV loading (via `hound`), stereo-to-mono downmixing,
//! and normalization to `f32` samples.

use std::path::Path;

use hound::WavReader;

use crate::error::WavioError;

/// Internal standard sample rate used throughout the DSP pipeline.
///
/// All loaded audio is expected at this rate. Resampling (to be added later)
/// will convert non-matching sample rates to this value.
pub const INTERNAL_SAMPLE_RATE: u32 = 22_050;

/// Raw audio data loaded into memory, ready for DSP processing.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AudioData {
    /// Mono PCM samples normalized to the range `[-1.0, 1.0]`.
    pub samples: Vec<f32>,
    /// Sample rate of the loaded audio in Hz.
    pub sample_rate: u32,
    /// Number of channels in the original file before downmixing.
    pub original_channels: u16,
}

impl AudioData {
    /// Returns the duration of the audio in seconds.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn duration_secs(&self) -> f32 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.samples.len() as f32 / self.sample_rate as f32
    }

    /// Returns the total number of mono samples.
    #[must_use]
    pub fn num_samples(&self) -> usize {
        self.samples.len()
    }
}

/// Loads a WAV file from disk and returns normalized mono `f32` samples.
///
/// The loader performs the following steps:
/// 1. Opens the WAV file using `hound`.
/// 2. Reads all samples as `f32`, normalizing integer formats to `[-1.0, 1.0]`.
/// 3. Down-mixes multi-channel audio to mono by averaging across channels.
///
/// # Errors
///
/// - [`WavioError::FileNotFound`] if the path does not exist.
/// - [`WavioError::InvalidWavFormat`] if `hound` cannot parse the file.
/// - [`WavioError::UnsupportedAudioFormat`] if the sample format is unrecognized.
/// - [`WavioError::AudioTooShort`] if the file contains zero samples.
pub fn load_wav(path: &str) -> Result<AudioData, WavioError> {
    let file_path = Path::new(path);
    if !file_path.exists() {
        return Err(WavioError::FileNotFound(path.to_string()));
    }

    let reader = WavReader::open(file_path)
        .map_err(|e| WavioError::InvalidWavFormat(format!("{path}: {e}")))?;

    let spec = reader.spec();
    let channels = spec.channels;
    let sample_rate = spec.sample_rate;
    let bits = spec.bits_per_sample;
    let sample_format = spec.sample_format;

    // Read all samples and normalize to f32 in [-1.0, 1.0].
    let raw_samples: Vec<f32> = match sample_format {
        hound::SampleFormat::Int => {
            #[allow(clippy::cast_precision_loss)]
            let max_val = (1_i64 << (bits - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| {
                    let sample = s.map_err(|e| {
                        WavioError::InvalidWavFormat(format!("sample read error: {e}"))
                    })?;
                    #[allow(clippy::cast_precision_loss)]
                    let normalized = sample as f32 / max_val;
                    Ok(normalized)
                })
                .collect::<Result<Vec<f32>, WavioError>>()?
        }
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .map(|s| {
                s.map_err(|e| {
                    WavioError::InvalidWavFormat(format!("sample read error: {e}"))
                })
            })
            .collect::<Result<Vec<f32>, WavioError>>()?,
    };

    if raw_samples.is_empty() {
        return Err(WavioError::AudioTooShort);
    }

    // Down-mix to mono by averaging across channels.
    let mono_samples = downmix_to_mono(&raw_samples, channels);

    Ok(AudioData {
        samples: mono_samples,
        sample_rate,
        original_channels: channels,
    })
}

/// Averages interleaved multi-channel samples into a single mono channel.
///
/// For single-channel audio this is a no-op copy.
fn downmix_to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    if channels == 1 {
        return interleaved.to_vec();
    }

    let ch = channels as usize;
    let num_frames = interleaved.len() / ch;
    let mut mono = Vec::with_capacity(num_frames);

    for frame_idx in 0..num_frames {
        let start = frame_idx * ch;
        let sum: f32 = interleaved[start..start + ch].iter().sum();
        mono.push(sum / f32::from(channels));
    }

    mono
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_downmix_mono_passthrough() {
        let samples = vec![0.1, 0.2, 0.3, 0.4];
        let result = downmix_to_mono(&samples, 1);
        assert_eq!(result, samples);
    }

    #[test]
    fn test_downmix_stereo() {
        // Stereo interleaved: (L, R) pairs
        let samples = vec![0.5, -0.5, 1.0, 0.0, -1.0, 1.0];
        let result = downmix_to_mono(&samples, 2);
        assert_eq!(result.len(), 3);
        assert!((result[0] - 0.0).abs() < f32::EPSILON); // (0.5 + -0.5) / 2
        assert!((result[1] - 0.5).abs() < f32::EPSILON); // (1.0 + 0.0) / 2
        assert!((result[2] - 0.0).abs() < f32::EPSILON); // (-1.0 + 1.0) / 2
    }

    #[test]
    fn test_load_wav_file_not_found() {
        let result = load_wav("/nonexistent/path/file.wav");
        assert!(result.is_err());
        match result.unwrap_err() {
            WavioError::FileNotFound(p) => {
                assert_eq!(p, "/nonexistent/path/file.wav");
            }
            other => panic!("Expected FileNotFound, got: {other:?}"),
        }
    }

    #[test]
    fn test_audio_data_duration() {
        let audio = AudioData {
            samples: vec![0.0; 22_050],
            sample_rate: 22_050,
            original_channels: 1,
        };
        assert!((audio.duration_secs() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_audio_data_zero_sample_rate() {
        let audio = AudioData {
            samples: vec![0.0; 100],
            sample_rate: 0,
            original_channels: 1,
        };
        assert!((audio.duration_secs() - 0.0).abs() < f32::EPSILON);
    }
}
