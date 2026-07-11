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

    // Resample if the sample rate doesn't match INTERNAL_SAMPLE_RATE.
    let resampled_samples = if sample_rate == INTERNAL_SAMPLE_RATE {
        mono_samples
    } else {
        resample(&mono_samples, sample_rate, INTERNAL_SAMPLE_RATE)
    };

    Ok(AudioData {
        samples: resampled_samples,
        sample_rate: INTERNAL_SAMPLE_RATE,
        original_channels: channels,
    })
}

/// Load an audio file from disk, downmix it to mono, and resample it to `INTERNAL_SAMPLE_RATE`.
///
/// Under the hood, if the `symphonia` feature is enabled, this uses Symphonia to support
/// MP3, FLAC, AAC, OGG, and WAV. Otherwise, it delegates to the native `load_wav` (WAV only).
pub fn load_audio(path: &str) -> Result<AudioData, WavioError> {
    #[cfg(feature = "symphonia")]
    {
        load_with_symphonia(path)
    }
    #[cfg(not(feature = "symphonia"))]
    {
        load_wav(path)
    }
}

/// Decodes any audio file using `symphonia` and resamples to `INTERNAL_SAMPLE_RATE`.
///
/// Requires the `symphonia` feature flag.
#[cfg(feature = "symphonia")]
pub fn load_with_symphonia(path: &str) -> Result<AudioData, WavioError> {
    use std::fs::File;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::probe::Hint;
    use symphonia::core::audio::SampleBuffer;
    use std::path::Path;

    let file_path = Path::new(path);
    if !file_path.exists() {
        return Err(WavioError::FileNotFound(path.to_string()));
    }

    let file = File::open(file_path).map_err(|e| WavioError::FileNotFound(format!("{path}: {e}")))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = file_path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &Default::default(), &Default::default())
        .map_err(|e| WavioError::UnsupportedAudioFormat(format!("probe error: {e}")))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| WavioError::UnsupportedAudioFormat("no supported tracks found".to_string()))?;

    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.ok_or_else(|| {
        WavioError::UnsupportedAudioFormat("sample rate missing from track metadata".to_string())
    })?;

    let channels = track.codec_params.channels.ok_or_else(|| {
        WavioError::UnsupportedAudioFormat("channel info missing from track metadata".to_string())
    })?.count() as u16;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &Default::default())
        .map_err(|e| WavioError::UnsupportedAudioFormat(format!("decoder creation failed: {e}")))?;

    let mut raw_samples = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::IoError(ref err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(WavioError::InvalidWavFormat(format!("packet read failed: {e}"))),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = decoder
            .decode(&packet)
            .map_err(|e| WavioError::InvalidWavFormat(format!("decode failed: {e}")))?;

        let spec = *decoded.spec();
        let mut sample_buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);
        raw_samples.extend_from_slice(sample_buf.samples());
    }

    if raw_samples.is_empty() {
        return Err(WavioError::AudioTooShort);
    }

    // Down-mix to mono by averaging across channels.
    let mono_samples = downmix_to_mono(&raw_samples, channels);

    // Resample if the sample rate doesn't match INTERNAL_SAMPLE_RATE.
    let resampled_samples = if sample_rate == INTERNAL_SAMPLE_RATE {
        mono_samples
    } else {
        resample(&mono_samples, sample_rate, INTERNAL_SAMPLE_RATE)
    };

    Ok(AudioData {
        samples: resampled_samples,
        sample_rate: INTERNAL_SAMPLE_RATE,
        original_channels: channels,
    })
}

/// Resamples mono `f32` samples from `from_rate` to `to_rate` using linear interpolation.
///
/// If `from_rate == to_rate`, this returns a copy of the input.
#[must_use]
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }

    let scale = from_rate as f64 / to_rate as f64;
    let num_samples = samples.len();

    // Calculate new length based on duration.
    let duration = num_samples as f64 / from_rate as f64;
    let target_len = (duration * to_rate as f64).round() as usize;

    if target_len == 0 {
        return Vec::new();
    }

    let mut resampled = Vec::with_capacity(target_len);

    for i in 0..target_len {
        let src_idx_f = i as f64 * scale;
        let src_idx = src_idx_f.floor() as usize;
        let frac = (src_idx_f - src_idx as f64) as f32;

        if src_idx >= num_samples {
            break;
        }

        if src_idx + 1 < num_samples {
            let val = (1.0 - frac) * samples[src_idx] + frac * samples[src_idx + 1];
            resampled.push(val);
        } else {
            resampled.push(samples[src_idx]);
        }
    }

    resampled
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

    #[test]
    fn test_resample_noop() {
        let samples = vec![0.0, 0.5, 1.0, -0.5];
        let result = resample(&samples, 22_050, 22_050);
        assert_eq!(result, samples);
    }

    #[test]
    fn test_resample_empty() {
        let samples: Vec<f32> = Vec::new();
        let result = resample(&samples, 44_100, 22_050);
        assert!(result.is_empty());
    }

    #[test]
    fn test_resample_downsample() {
        // Downsample from 44,100 to 22,050 (exactly 2x downsampling)
        let samples = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let result = resample(&samples, 44_100, 22_050);
        // Expect length to be 3.
        assert_eq!(result.len(), 3);
        // Expected indices in source: 0, 2, 4
        assert!((result[0] - 1.0).abs() < f32::EPSILON);
        assert!((result[1] - 3.0).abs() < f32::EPSILON);
        assert!((result[2] - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_resample_upsample() {
        // Upsample from 11,025 to 22,050 (exactly 2x upsampling)
        let samples = vec![1.0, 2.0];
        let result = resample(&samples, 11_025, 22_050);
        // Expect length to be 4.
        assert_eq!(result.len(), 4);
        // Expected values:
        // i = 0: src_idx_f = 0.0, src_idx = 0, frac = 0.0 -> 1.0
        // i = 1: src_idx_f = 0.5, src_idx = 0, frac = 0.5 -> 0.5 * 1.0 + 0.5 * 2.0 = 1.5
        // i = 2: src_idx_f = 1.0, src_idx = 1, frac = 0.0 -> 2.0
        // i = 3: src_idx_f = 1.5, src_idx = 1 -> boundary, value 2.0
        assert!((result[0] - 1.0).abs() < f32::EPSILON);
        assert!((result[1] - 1.5).abs() < f32::EPSILON);
        assert!((result[2] - 2.0).abs() < f32::EPSILON);
        assert!((result[3] - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    #[cfg(feature = "symphonia")]
    fn test_load_with_symphonia_success() {
        let path = "data/sample.wav";
        let result_sym = load_with_symphonia(path).expect("failed to load WAV via symphonia");

        assert_eq!(result_sym.sample_rate, INTERNAL_SAMPLE_RATE);
        assert!(!result_sym.samples.is_empty());

        // If native load_wav also succeeds, verify equivalence.
        if let Ok(result_wav) = load_wav(path) {
            assert_eq!(result_wav.sample_rate, result_sym.sample_rate);
            assert_eq!(result_wav.original_channels, result_sym.original_channels);
            assert_eq!(result_wav.samples.len(), result_sym.samples.len());

            for (s1, s2) in result_wav.samples.iter().zip(result_sym.samples.iter()) {
                assert!((s1 - s2).abs() < 1e-4);
            }
        }
    }
}
