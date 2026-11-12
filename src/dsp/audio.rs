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
            .map(|s| s.map_err(|e| WavioError::InvalidWavFormat(format!("sample read error: {e}"))))
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
///
/// # Errors
///
/// Returns a [`WavioError`] if the file is missing, unreadable, in an
/// unsupported format, or corrupt.
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
///
/// # Errors
///
/// Returns a [`WavioError`] if the file is missing, unreadable, in an
/// unsupported format, or fails to decode.
#[cfg(feature = "symphonia")]
pub fn load_with_symphonia(path: &str) -> Result<AudioData, WavioError> {
    use std::fs::File;
    use std::path::Path;
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file_path = Path::new(path);
    if !file_path.exists() {
        return Err(WavioError::FileNotFound(path.to_string()));
    }

    let file =
        File::open(file_path).map_err(|e| WavioError::FileNotFound(format!("{path}: {e}")))?;
    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

    let mut hint = Hint::new();
    if let Some(ext) = file_path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| WavioError::UnsupportedAudioFormat(format!("probe error: {e}")))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| {
            WavioError::UnsupportedAudioFormat("no supported tracks found".to_string())
        })?;

    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.ok_or_else(|| {
        WavioError::UnsupportedAudioFormat("sample rate missing from track metadata".to_string())
    })?;

    let channels = u16::try_from(
        track
            .codec_params
            .channels
            .ok_or_else(|| {
                WavioError::UnsupportedAudioFormat(
                    "channel info missing from track metadata".to_string(),
                )
            })?
            .count(),
    )
    .map_err(|_| WavioError::UnsupportedAudioFormat("too many channels".to_string()))?;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
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
            Err(e) => {
                return Err(WavioError::InvalidWavFormat(format!(
                    "packet read failed: {e}"
                )));
            }
        };

        if packet.track_id() != track_id {
            continue;
        }

        let audio_buf = decoder
            .decode(&packet)
            .map_err(|e| WavioError::InvalidWavFormat(format!("decode failed: {e}")))?;

        let spec = *audio_buf.spec();
        let mut sample_buf = SampleBuffer::<f32>::new(audio_buf.capacity() as u64, spec);
        sample_buf.copy_interleaved_ref(audio_buf);
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

/// Resamples mono `f32` samples with **linear interpolation and no low-pass
/// filter**.
///
/// Fast, but when `to_rate < from_rate` everything above the new Nyquist
/// frequency folds back into the audible band as aliasing. Kept for
/// comparison and for callers who know their signal is already band-limited;
/// the pipeline uses [`resample`].
///
/// If `from_rate == to_rate`, this returns a copy of the input.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn resample_linear(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }

    let scale = f64::from(from_rate) / f64::from(to_rate);
    let num_samples = samples.len();

    // Calculate new length based on duration.
    let duration = num_samples as f64 / f64::from(from_rate);
    let target_len = (duration * f64::from(to_rate)).round() as usize;

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

/// Half-width of the resampling kernel, in zero crossings of the (output-rate)
/// sinc. More crossings give a sharper transition band at higher cost.
const SINC_ZERO_CROSSINGS: f64 = 8.0;
/// Kaiser window shape: ~80 dB stop-band attenuation.
const KAISER_BETA: f64 = 8.0;
/// Above this many distinct fractional phases the kernel is evaluated on the
/// fly instead of precomputed (only reachable with unusual rate pairs).
const MAX_PRECOMPUTED_PHASES: usize = 16_384;

/// Modified Bessel function of the first kind, order 0 (power series).
fn bessel_i0(x: f64) -> f64 {
    let q = x * x / 4.0;
    let (mut sum, mut term) = (1.0_f64, 1.0_f64);
    for k in 1..60 {
        term *= q / f64::from(k * k);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Kaiser-windowed sinc evaluated at `x` (already scaled so that the first
/// zero crossing is at `x = 1`); zero outside `|x| < SINC_ZERO_CROSSINGS`.
fn windowed_sinc(x: f64) -> f64 {
    if x.abs() >= SINC_ZERO_CROSSINGS {
        return 0.0;
    }
    let sinc = if x.abs() < 1e-12 {
        1.0
    } else {
        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
    };
    let u = x / SINC_ZERO_CROSSINGS;
    sinc * bessel_i0(KAISER_BETA * (1.0 - u * u).sqrt()) / bessel_i0(KAISER_BETA)
}

const fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Resamples mono `f32` samples from `from_rate` to `to_rate` with a
/// **Kaiser-windowed-sinc** filter.
///
/// When downsampling, the kernel is stretched so its cutoff sits at the *new*
/// Nyquist frequency: content above it is removed instead of aliasing into
/// the band (about 80 dB of stop-band attenuation). The passband is flat to
/// within a fraction of a dB up to ~90% of the new Nyquist. Output sample
/// `i` is centered at input time `i * from_rate / to_rate`, so there is no
/// added delay, and levels near the edges are renormalized so a constant
/// signal stays constant.
///
/// For rate pairs with a small reduced ratio (44.1 kHz or 48 kHz to
/// 22.05 kHz) the filter taps for each fractional phase are precomputed, so
/// the inner loop is a plain dot product.
///
/// If `from_rate == to_rate`, this returns a copy of the input; a zero rate
/// yields an empty vector. See [`resample_linear`] for the cheap, aliasing
/// alternative.
///
/// # Examples
///
/// ```
/// use wavio::dsp::audio::resample;
///
/// // One second of a 15 kHz tone at 44.1 kHz lies above the 11.025 kHz
/// // Nyquist of the 22.05 kHz target; a proper resampler removes it.
/// let tone: Vec<f32> = (0..44_100)
///     .map(|i| (std::f32::consts::TAU * 15_000.0 * i as f32 / 44_100.0).sin())
///     .collect();
/// let out = resample(&tone, 44_100, 22_050);
/// assert_eq!(out.len(), 22_050);
/// let rms = (out[1000..21_000].iter().map(|x| x * x).sum::<f32>() / 20_000.0).sqrt();
/// assert!(rms < 0.01, "aliased energy leaked through: rms = {rms}");
/// ```
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names
)]
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    resample_impl(samples, from_rate, to_rate, cfg!(feature = "parallel"))
}

/// Shared implementation of [`resample`]; `parallel` is only honored when the
/// `parallel` feature is compiled in (output samples are independent, so the
/// result is identical either way).
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names,
    clippy::too_many_lines,
    unused_variables
)]
fn resample_impl(samples: &[f32], from_rate: u32, to_rate: u32, parallel: bool) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }
    if from_rate == 0 || to_rate == 0 {
        return Vec::new();
    }

    let n = samples.len() as i64;
    let target_len =
        (samples.len() as f64 * f64::from(to_rate) / f64::from(from_rate)).round() as usize;
    if target_len == 0 {
        return Vec::new();
    }

    // Reduced ratio: output i sits at input position i * m / l.
    let g = gcd(u64::from(from_rate), u64::from(to_rate));
    let (m, l) = (u64::from(from_rate) / g, (u64::from(to_rate) / g) as usize);
    // Cutoff relative to the input Nyquist (< 1 when downsampling), and the
    // kernel half-width in input samples.
    let fc = (f64::from(to_rate) / f64::from(from_rate)).min(1.0);
    let half = (SINC_ZERO_CROSSINGS / fc).ceil() as i64;
    let taps = (2 * half) as usize;

    // Weights for the output at integer base `b` with fractional offset `frac`:
    // taps over input indices b - half + 1 ..= b + half.
    let weights_for = |frac: f64, out: &mut Vec<f64>| {
        out.clear();
        out.extend((0..taps).map(|j| {
            let k = (j as i64 - half + 1) as f64;
            windowed_sinc((frac - k) * fc)
        }));
    };

    let precomputed: Option<Vec<f32>> = (l <= MAX_PRECOMPUTED_PHASES).then(|| {
        let mut table = Vec::with_capacity(l * taps);
        let mut w = Vec::with_capacity(taps);
        for p in 0..l {
            weights_for(p as f64 / l as f64, &mut w);
            let sum: f64 = w.iter().sum();
            table.extend(w.iter().map(|&x| (x / sum) as f32));
        }
        table
    });

    // One output sample. `scratch` is reusable per-thread storage for the
    // (rare) edge path.
    let compute = |i: usize, scratch: &mut Vec<f64>| -> f32 {
        let pos = i as u64 * m;
        let base = (pos / l as u64) as i64;
        let phase = (pos % l as u64) as usize;
        let start = base - half + 1;

        let interior = start >= 0 && start + taps as i64 <= n;
        let fast_table = precomputed.as_ref().filter(|_| interior);
        if let Some(table) = fast_table {
            let w = &table[phase * taps..(phase + 1) * taps];
            let x = &samples[start as usize..start as usize + taps];
            w.iter().zip(x).map(|(a, b)| a * b).sum::<f32>()
        } else {
            // Edges (or no table): renormalize over the taps that exist.
            weights_for(phase as f64 / l as f64, scratch);
            let (mut acc, mut wsum) = (0.0_f64, 0.0_f64);
            for (j, &w) in scratch.iter().enumerate() {
                let idx = start + j as i64;
                if (0..n).contains(&idx) {
                    acc += w * f64::from(samples[idx as usize]);
                    wsum += w;
                }
            }
            if wsum.abs() > 1e-9 {
                (acc / wsum) as f32
            } else {
                0.0
            }
        }
    };

    #[cfg(feature = "parallel")]
    if parallel {
        use rayon::prelude::*;
        return (0..target_len)
            .into_par_iter()
            .map_init(
                || Vec::with_capacity(taps),
                |scratch, i| compute(i, scratch),
            )
            .collect();
    }

    let mut scratch = Vec::with_capacity(taps);
    (0..target_len).map(|i| compute(i, &mut scratch)).collect()
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
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_cmp,
    clippy::similar_names,
    clippy::needless_pass_by_value
)]
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

    // ---- Windowed-sinc `resample` ----

    fn sine(freq: f32, rate: u32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn test_resample_noop_empty_and_zero_rates() {
        let samples = vec![0.0, 0.5, 1.0, -0.5];
        assert_eq!(resample(&samples, 22_050, 22_050), samples);
        assert!(resample(&[], 44_100, 22_050).is_empty());
        assert!(resample(&samples, 0, 22_050).is_empty());
        assert!(resample(&samples, 44_100, 0).is_empty());
    }

    #[test]
    fn test_resample_lengths() {
        for (from, to) in [
            (44_100, 22_050),
            (48_000, 22_050),
            (11_025, 22_050),
            (8_000, 22_050),
        ] {
            let out = resample(&sine(440.0, from, 1.0), from, to);
            assert_eq!(out.len(), to as usize, "{from} -> {to}");
        }
    }

    #[test]
    fn test_resample_removes_content_above_new_nyquist() {
        // 15 kHz is above the 11.025 kHz Nyquist of the target.
        for from in [44_100_u32, 48_000] {
            let tone = sine(15_000.0, from, 1.0);
            let sinc = resample(&tone, from, 22_050);
            let linear = resample_linear(&tone, from, 22_050);
            let mid = |v: &[f32]| rms(&v[2000..v.len() - 2000]);
            assert!(mid(&sinc) < 0.01, "{from}: sinc rms {}", mid(&sinc));
            assert!(
                mid(&linear) > 0.2,
                "{from}: linear aliasing rms {}",
                mid(&linear)
            );
        }
    }

    #[test]
    fn test_resample_preserves_passband_tone_and_timing() {
        for (from, to, f) in [
            (44_100_u32, 22_050_u32, 3_000.0_f32),
            (48_000, 22_050, 2_000.0),
            (11_025, 22_050, 1_000.0),
        ] {
            let out = resample(&sine(f, from, 1.0), from, to);
            let ideal = sine(f, to, 1.0);
            let margin = 600;
            let max_err = out[margin..out.len() - margin]
                .iter()
                .zip(&ideal[margin..])
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(max_err < 0.01, "{from}->{to} @ {f} Hz: max error {max_err}");
        }
    }

    #[test]
    fn test_resample_keeps_constant_signal_constant_even_at_edges() {
        for (from, to) in [(44_100, 22_050), (48_000, 22_050), (8_000, 22_050)] {
            let out = resample(&vec![0.7_f32; 4_000], from, to);
            assert!(out.iter().all(|v| (v - 0.7).abs() < 1e-3), "{from}->{to}");
        }
    }

    #[cfg(feature = "parallel")]
    #[test]
    fn test_resample_parallel_is_bit_identical_to_serial() {
        for (from, to) in [
            (44_100, 22_050),
            (48_000, 22_050),
            (8_000, 22_050),
            (44_101, 32_003),
        ] {
            let x = sine(777.0, from, 2.0);
            assert_eq!(
                resample_impl(&x, from, to, true),
                resample_impl(&x, from, to, false),
                "{from} -> {to}"
            );
        }
    }

    #[test]
    fn test_resample_unusual_ratio_uses_on_the_fly_path() {
        // Coprime rates whose reduced ratio exceeds the precompute limit.
        let out = resample(&sine(1_000.0, 44_101, 0.3), 44_101, 32_003);
        let ideal = sine(1_000.0, 32_003, 0.3);
        let n = out.len().min(ideal.len());
        let max_err = (400..n - 400)
            .map(|i| (out[i] - ideal[i]).abs())
            .fold(0.0_f32, f32::max);
        assert!(max_err < 0.02, "max error {max_err}");
    }

    #[test]
    fn test_resample_linear_noop() {
        let samples = vec![0.0, 0.5, 1.0, -0.5];
        let result = resample_linear(&samples, 22_050, 22_050);
        assert_eq!(result, samples);
    }

    #[test]
    fn test_resample_linear_empty() {
        let samples: Vec<f32> = Vec::new();
        let result = resample_linear(&samples, 44_100, 22_050);
        assert!(result.is_empty());
    }

    #[test]
    fn test_resample_linear_downsample() {
        // Downsample from 44,100 to 22,050 (exactly 2x downsampling)
        let samples = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let result = resample_linear(&samples, 44_100, 22_050);
        // Expect length to be 3.
        assert_eq!(result.len(), 3);
        // Expected indices in source: 0, 2, 4
        assert!((result[0] - 1.0).abs() < f32::EPSILON);
        assert!((result[1] - 3.0).abs() < f32::EPSILON);
        assert!((result[2] - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_resample_linear_upsample() {
        // Upsample from 11,025 to 22,050 (exactly 2x upsampling)
        let samples = vec![1.0, 2.0];
        let result = resample_linear(&samples, 11_025, 22_050);
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
