//! Peak detection over spectrograms.
//!
//! Extracts constellation points (local maxima) from a spectrogram
//! using 2D neighborhood filtering and amplitude thresholding.

use ndarray::Array2;

#[cfg(feature = "parallel")]
use rayon::prelude::*;


/// A single spectral peak (constellation point).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Peak {
    /// Time position in seconds within the audio.
    pub time: f32,
    /// Frequency in Hz.
    pub freq: f32,
    /// Amplitude in dB at this time-frequency location.
    pub amplitude: f32,
}

impl Peak {
    /// Creates a new `Peak` with the specified time, frequency, and amplitude.
    ///
    /// # Arguments
    ///
    /// * `time` - Time position in seconds.
    /// * `freq` - Frequency in Hz.
    /// * `amplitude` - Amplitude in dB.
    ///
    /// # Examples
    ///
    /// ```
    /// use wavio::dsp::peaks::Peak;
    ///
    /// let peak = Peak::new(1.0, 440.0, -10.0);
    /// assert_eq!(peak.time, 1.0);
    /// assert_eq!(peak.freq, 440.0);
    /// assert_eq!(peak.amplitude, -10.0);
    /// ```
    #[must_use]
    pub fn new(time: f32, freq: f32, amplitude: f32) -> Self {
        Self {
            time,
            freq,
            amplitude,
        }
    }
}


/// Configuration for the peak extraction algorithm.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PeakExtractorConfig {
    /// Half-width of the neighborhood window along the time axis (frames).
    pub time_neighborhood: usize,
    /// Half-width of the neighborhood window along the frequency axis (bins).
    pub freq_neighborhood: usize,
    /// Minimum amplitude (dB) for a bin to qualify as a peak.
    pub threshold_db: f32,
    /// Sample rate of the audio (used to convert frame/bin indices to time/freq).
    pub sample_rate: u32,
    /// Hop size used during spectrogram generation (samples per frame advance).
    pub hop_size: usize,
    /// FFT window size (used to compute frequency resolution).
    pub window_size: usize,
}

impl Default for PeakExtractorConfig {
    fn default() -> Self {
        Self {
            time_neighborhood: 10,
            freq_neighborhood: 10,
            threshold_db: -40.0,
            sample_rate: 22_050,
            hop_size: 512,
            window_size: 2048,
        }
    }
}

impl PeakExtractorConfig {
    /// Frequency resolution: Hz per bin.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    fn freq_per_bin(&self) -> f32 {
        self.sample_rate as f32 / self.window_size as f32
    }

    /// Time resolution: seconds per frame.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    fn time_per_frame(&self) -> f32 {
        self.hop_size as f32 / self.sample_rate as f32
    }
}

/// Extracts constellation peaks from a spectrogram using 2D local max filtering.
///
/// A bin is a peak if:
/// 1. Its amplitude is above `config.threshold_db`.
/// 2. It is the strict maximum within the rectangular neighborhood
///    defined by `config.time_neighborhood` and `config.freq_neighborhood`.
///
/// Returns peaks sorted by time, then by frequency.
///
/// # Arguments
///
/// * `spectrogram` -- dB power spectrogram of shape `[n_frames, n_bins]`.
/// * `config` -- tuning parameters for extraction.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn extract_peaks(spectrogram: &Array2<f32>, config: &PeakExtractorConfig) -> Vec<Peak> {
    let (n_frames, n_bins) = spectrogram.dim();
    let t_neigh = config.time_neighborhood;
    let f_neigh = config.freq_neighborhood;
    let threshold = config.threshold_db;
    let freq_res = config.freq_per_bin();
    let time_res = config.time_per_frame();

    let mut peaks = Vec::new();

    for frame in 0..n_frames {
        for bin in 0..n_bins {
            let val = spectrogram[[frame, bin]];

            // Threshold gate.
            if val < threshold {
                continue;
            }

            // Check local neighborhood -- must be strictly the maximum.
            let t_start = frame.saturating_sub(t_neigh);
            let t_end = (frame + t_neigh + 1).min(n_frames);
            let f_start = bin.saturating_sub(f_neigh);
            let f_end = (bin + f_neigh + 1).min(n_bins);

            let mut is_max = true;
            'outer: for t in t_start..t_end {
                for f in f_start..f_end {
                    if (t, f) == (frame, bin) {
                        continue;
                    }
                    if spectrogram[[t, f]] >= val {
                        is_max = false;
                        break 'outer;
                    }
                }
            }

            if is_max {
                peaks.push(Peak {
                    time: frame as f32 * time_res,
                    freq: bin as f32 * freq_res,
                    amplitude: val,
                });
            }
        }
    }

    // Sort by time, then by frequency for deterministic output.
    peaks.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.freq
                    .partial_cmp(&b.freq)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    peaks
}

/// Parallel variant of [`extract_peaks`] using `rayon`.
///
/// Distributes frame-level local-max computation across the thread pool.
/// Each frame is independent (read-only spectrogram access), making this
/// embarrassingly parallel. Results are merged and sorted identically to
/// the serial version, guaranteeing the same output.
///
/// Requires the `parallel` feature flag.
///
/// # Arguments
///
/// * `spectrogram` -- dB power spectrogram of shape `[n_frames, n_bins]`.
/// * `config` -- tuning parameters for extraction.
#[cfg(feature = "parallel")]
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn extract_peaks_parallel(spectrogram: &Array2<f32>, config: &PeakExtractorConfig) -> Vec<Peak> {
    let (n_frames, n_bins) = spectrogram.dim();
    let t_neigh = config.time_neighborhood;
    let f_neigh = config.freq_neighborhood;
    let threshold = config.threshold_db;
    let freq_res = config.freq_per_bin();
    let time_res = config.time_per_frame();

    // Each frame is processed independently: read-only access to `spectrogram`.
    let mut peaks: Vec<Peak> = (0..n_frames)
        .into_par_iter()
        .flat_map(|frame| {
            let mut frame_peaks = Vec::new();

            for bin in 0..n_bins {
                let val = spectrogram[[frame, bin]];

                if val < threshold {
                    continue;
                }

                let t_start = frame.saturating_sub(t_neigh);
                let t_end = (frame + t_neigh + 1).min(n_frames);
                let f_start = bin.saturating_sub(f_neigh);
                let f_end = (bin + f_neigh + 1).min(n_bins);

                let mut is_max = true;
                'outer: for t in t_start..t_end {
                    for f in f_start..f_end {
                        if (t, f) == (frame, bin) {
                            continue;
                        }
                        if spectrogram[[t, f]] >= val {
                            is_max = false;
                            break 'outer;
                        }
                    }
                }

                if is_max {
                    frame_peaks.push(Peak {
                        time: frame as f32 * time_res,
                        freq: bin as f32 * freq_res,
                        amplitude: val,
                    });
                }
            }

            frame_peaks
        })
        .collect();

    peaks.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.freq
                    .partial_cmp(&b.freq)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    peaks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a silent spectrogram and inject known peaks.
    fn make_test_spectrogram(
        n_frames: usize,
        n_bins: usize,
        injected: &[(usize, usize, f32)],
    ) -> Array2<f32> {
        let mut spec = Array2::<f32>::from_elem((n_frames, n_bins), -100.0);
        for &(t, f, amp) in injected {
            spec[[t, f]] = amp;
        }
        spec
    }

    #[test]
    fn test_single_peak_recovery() {
        let spec = make_test_spectrogram(50, 129, &[(25, 64, -10.0)]);
        let config = PeakExtractorConfig {
            time_neighborhood: 5,
            freq_neighborhood: 5,
            threshold_db: -40.0,
            ..PeakExtractorConfig::default()
        };
        let peaks = extract_peaks(&spec, &config);
        assert_eq!(peaks.len(), 1);
        assert!((peaks[0].amplitude - (-10.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn test_multiple_separated_peaks() {
        // Two peaks far enough apart to not interfere.
        let spec = make_test_spectrogram(100, 129, &[(10, 20, -5.0), (60, 100, -15.0)]);
        let config = PeakExtractorConfig {
            time_neighborhood: 5,
            freq_neighborhood: 5,
            threshold_db: -40.0,
            ..PeakExtractorConfig::default()
        };
        let peaks = extract_peaks(&spec, &config);
        assert_eq!(peaks.len(), 2);
    }

    #[test]
    fn test_threshold_filters_low_peaks() {
        // Peak at -50 dB should be below the -40 dB threshold.
        let spec = make_test_spectrogram(50, 129, &[(25, 64, -50.0)]);
        let config = PeakExtractorConfig {
            time_neighborhood: 5,
            freq_neighborhood: 5,
            threshold_db: -40.0,
            ..PeakExtractorConfig::default()
        };
        let peaks = extract_peaks(&spec, &config);
        assert!(peaks.is_empty());
    }

    #[test]
    fn test_adjacent_peak_suppression() {
        // Two peaks within each other's neighborhood -- only the stronger survives.
        let spec = make_test_spectrogram(50, 129, &[(25, 64, -10.0), (26, 65, -20.0)]);
        let config = PeakExtractorConfig {
            time_neighborhood: 5,
            freq_neighborhood: 5,
            threshold_db: -40.0,
            ..PeakExtractorConfig::default()
        };
        let peaks = extract_peaks(&spec, &config);
        assert_eq!(peaks.len(), 1);
        assert!((peaks[0].amplitude - (-10.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn test_empty_spectrogram() {
        let spec = Array2::<f32>::from_elem((0, 0), -100.0);
        let config = PeakExtractorConfig::default();
        let peaks = extract_peaks(&spec, &config);
        assert!(peaks.is_empty());
    }

    #[test]
    fn test_peaks_sorted_by_time() {
        let spec = make_test_spectrogram(100, 129, &[(80, 10, -5.0), (20, 90, -5.0)]);
        let config = PeakExtractorConfig {
            time_neighborhood: 5,
            freq_neighborhood: 5,
            threshold_db: -40.0,
            ..PeakExtractorConfig::default()
        };
        let peaks = extract_peaks(&spec, &config);
        assert_eq!(peaks.len(), 2);
        assert!(peaks[0].time <= peaks[1].time);
    }
}
