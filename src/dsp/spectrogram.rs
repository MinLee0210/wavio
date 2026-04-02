//! Spectrogram generation.
//!
//! Sliding-window FFT over PCM samples to produce a time-frequency
//! power spectrogram stored as `ndarray::Array2<f32>`.

use ndarray::Array2;
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

use crate::error::WavioError;

/// Configuration for spectrogram generation.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SpectrogramConfig {
    /// Number of samples per FFT window. Must be a power of two.
    pub window_size: usize,
    /// Number of samples to advance between consecutive windows.
    pub hop_size: usize,
}

impl Default for SpectrogramConfig {
    fn default() -> Self {
        Self {
            window_size: 2048,
            hop_size: 512,
        }
    }
}

/// Generates a Hann window of the given length.
///
/// The Hann window tapers the edges of each frame to reduce spectral
/// leakage when performing the FFT.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn hann_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|i| {
            let phase = 2.0 * std::f32::consts::PI * i as f32 / size as f32;
            0.5 * (1.0 - phase.cos())
        })
        .collect()
}

/// Computes a power spectrogram from mono PCM samples.
///
/// The spectrogram is produced by:
/// 1. Sliding a Hann-windowed frame across the samples.
/// 2. Computing the FFT of each windowed frame.
/// 3. Taking the magnitude squared (power) of each frequency bin.
/// 4. Converting power to decibels: `10 * log10(power + epsilon)`.
///
/// The returned `Array2<f32>` has shape `[n_frames, n_bins]`, where
/// `n_bins = window_size / 2 + 1` (positive frequencies only).
///
/// # Errors
///
/// - [`WavioError::SpectrogramError`] if the input is shorter than one window.
/// - [`WavioError::SpectrogramError`] if `window_size` is zero or `hop_size` is zero.
pub fn compute_spectrogram(
    samples: &[f32],
    config: &SpectrogramConfig,
) -> Result<Array2<f32>, WavioError> {
    let window_size = config.window_size;
    let hop_size = config.hop_size;

    if window_size == 0 || hop_size == 0 {
        return Err(WavioError::SpectrogramError(
            "window_size and hop_size must be greater than zero".to_string(),
        ));
    }

    if samples.len() < window_size {
        return Err(WavioError::SpectrogramError(format!(
            "input length ({}) is shorter than window_size ({window_size})",
            samples.len()
        )));
    }

    let n_frames = (samples.len() - window_size) / hop_size + 1;
    let n_bins = window_size / 2 + 1;
    let window = hann_window(window_size);

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(window_size);

    let mut spectrogram = Array2::<f32>::zeros((n_frames, n_bins));

    // Scratch buffer reused for each frame.
    let mut buffer = vec![Complex::new(0.0_f32, 0.0_f32); window_size];

    for frame_idx in 0..n_frames {
        let start = frame_idx * hop_size;

        // Apply Hann window and load into complex buffer.
        for (j, val) in buffer.iter_mut().enumerate().take(window_size) {
            val.re = samples[start + j] * window[j];
            val.im = 0.0;
        }

        fft.process(&mut buffer);

        // Compute power in dB for positive frequency bins only.
        for bin in 0..n_bins {
            let power = buffer[bin].norm_sqr();
            let db = 10.0 * (power + 1e-10).log10();
            spectrogram[[frame_idx, bin]] = db;
        }
    }

    Ok(spectrogram)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hann_window_length() {
        let w = hann_window(1024);
        assert_eq!(w.len(), 1024);
    }

    #[test]
    fn test_hann_window_endpoints() {
        let w = hann_window(256);
        // Hann window should be near zero at the endpoints.
        assert!(w[0].abs() < 1e-6);
        assert!(w[255].abs() < 0.01);
    }

    #[test]
    fn test_hann_window_peak() {
        let w = hann_window(256);
        // Peak should be at the center, close to 1.0.
        assert!((w[128] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_spectrogram_shape() {
        let config = SpectrogramConfig {
            window_size: 256,
            hop_size: 128,
        };
        let samples = vec![0.0_f32; 1024];
        let spec = compute_spectrogram(&samples, &config).unwrap();
        // n_frames = (1024 - 256) / 128 + 1 = 7
        // n_bins = 256 / 2 + 1 = 129
        assert_eq!(spec.shape(), &[7, 129]);
    }

    #[test]
    fn test_spectrogram_too_short() {
        let config = SpectrogramConfig {
            window_size: 2048,
            hop_size: 512,
        };
        let samples = vec![0.0_f32; 100];
        let result = compute_spectrogram(&samples, &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_spectrogram_zero_config() {
        let config = SpectrogramConfig {
            window_size: 0,
            hop_size: 512,
        };
        let samples = vec![0.0_f32; 1024];
        let result = compute_spectrogram(&samples, &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_sine_wave_peak_at_correct_bin() {
        // Generate a sine wave at a known frequency and verify the
        // spectrogram shows a peak at the corresponding bin.
        let sample_rate = 22_050.0_f32;
        let freq = 440.0_f32; // A4
        let window_size = 2048;
        let n_samples = window_size * 2;

        let samples: Vec<f32> = (0..n_samples)
            .map(|i| {
                let t = i as f32 / sample_rate;
                (2.0 * std::f32::consts::PI * freq * t).sin()
            })
            .collect();

        let config = SpectrogramConfig {
            window_size,
            hop_size: window_size,
        };

        let spec = compute_spectrogram(&samples, &config).unwrap();
        let n_bins = window_size / 2 + 1;

        // Expected bin for 440 Hz:
        // bin = freq * window_size / sample_rate
        let expected_bin = (freq * window_size as f32 / sample_rate).round() as usize;

        // Find the bin with maximum power in the first frame.
        let first_frame = spec.row(0);
        let max_bin = first_frame
            .iter()
            .enumerate()
            .take(n_bins)
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;

        // Allow a tolerance of +/- 2 bins due to spectral leakage.
        let diff = if max_bin > expected_bin {
            max_bin - expected_bin
        } else {
            expected_bin - max_bin
        };
        assert!(
            diff <= 2,
            "Expected peak near bin {expected_bin}, found at bin {max_bin}"
        );
    }

    #[test]
    fn test_silence_produces_low_power() {
        let config = SpectrogramConfig {
            window_size: 256,
            hop_size: 128,
        };
        let samples = vec![0.0_f32; 1024];
        let spec = compute_spectrogram(&samples, &config).unwrap();

        // All bins should be very low (near -100 dB from epsilon floor).
        for val in spec.iter() {
            assert!(*val < -80.0, "Expected silence dB < -80, got {val}");
        }
    }
}
