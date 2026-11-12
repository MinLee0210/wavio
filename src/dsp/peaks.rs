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
    /// Creates a new `Peak`.
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
    /// Refine each peak's **frequency** to sub-bin precision by fitting a
    /// parabola through the peak and its two frequency neighbors (in dB).
    /// Off by default, so hashes match earlier releases.
    ///
    /// Without it, peaks sit on exact FFT-bin centers (~10.8 Hz apart at the
    /// defaults): negligible for absolute-bin pairwise hashes, but a large
    /// error relative to the frequency *ratios* that [`crate::triplet`]
    /// hashes (e.g. +-2.5% at 220 Hz). Interpolation removes most of it.
    ///
    /// Time is deliberately **not** interpolated: measured end to end, doing
    /// so made triplet matching markedly worse (it turns frame-aligned peaks
    /// into near-coincident ones whose time ratios are pure jitter).
    pub interpolate: bool,
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
            interpolate: false,
        }
    }
}

impl PeakExtractorConfig {
    /// Creates a new `PeakExtractorConfig` with custom parameters.
    #[must_use]
    pub fn new(
        time_neighborhood: usize,
        freq_neighborhood: usize,
        threshold_db: f32,
        sample_rate: u32,
        hop_size: usize,
        window_size: usize,
    ) -> Self {
        Self {
            time_neighborhood,
            freq_neighborhood,
            threshold_db,
            sample_rate,
            hop_size,
            window_size,
            interpolate: false,
        }
    }

    /// Enables or disables sub-bin peak interpolation
    /// (see [`PeakExtractorConfig::interpolate`]).
    #[must_use]
    pub fn with_interpolation(mut self, interpolate: bool) -> Self {
        self.interpolate = interpolate;
        self
    }

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
/// 1. Its amplitude is at least `config.threshold_db`.
/// 2. It is the strict maximum within the rectangular neighborhood
///    defined by `config.time_neighborhood` and `config.freq_neighborhood`
///    (a plateau of equal values therefore yields no peak).
///
/// Returns peaks sorted by time, then by frequency.
///
/// # Performance
///
/// The neighborhood scan exits at the first neighbor that is at least as
/// large as the candidate, so a typical bin costs only a couple of
/// comparisons; the full `(2t+1)(2f+1)` window is only paid by actual
/// peaks. The same-frame row is scanned first because adjacent bins are the
/// most likely to dominate (spectral leakage), and rows are scanned as
/// contiguous slices. A separable max-filter was benchmarked and was ~2x
/// *slower* on real-sized inputs, since it must touch every bin.
///
/// # Arguments
///
/// * `spectrogram` -- dB power spectrogram of shape `[n_frames, n_bins]`.
/// * `config` -- tuning parameters for extraction.
///
/// # Examples
///
/// ```
/// use ndarray::Array2;
/// use wavio::dsp::peaks::{extract_peaks, PeakExtractorConfig};
///
/// // Create an empty spectrogram with a single injected peak
/// let mut spec = Array2::<f32>::from_elem((50, 129), -100.0);
/// spec[[25, 64]] = -10.0;
///
/// let config = PeakExtractorConfig::new(5, 5, -40.0, 22050, 512, 2048);
/// let peaks = extract_peaks(&spec, &config);
/// assert_eq!(peaks.len(), 1);
/// assert_eq!(peaks[0].amplitude, -10.0);
/// ```
#[must_use]
pub fn extract_peaks(spectrogram: &Array2<f32>, config: &PeakExtractorConfig) -> Vec<Peak> {
    let spec = spectrogram.as_standard_layout();
    let (n_frames, n_bins) = spec.dim();
    let Some(data) = spec.as_slice() else {
        return Vec::new();
    };

    let mut peaks: Vec<Peak> = (0..n_frames)
        .flat_map(|frame| frame_peaks(data, n_frames, n_bins, frame, config))
        .collect();
    sort_peaks(&mut peaks);
    peaks
}

/// Parallel variant of [`extract_peaks`] using `rayon`.
///
/// Each frame is independent (read-only spectrogram access), making this
/// embarrassingly parallel. Output is identical to [`extract_peaks`].
///
/// Requires the `parallel` feature flag.
///
/// # Arguments
///
/// * `spectrogram` -- dB power spectrogram of shape `[n_frames, n_bins]`.
/// * `config` -- tuning parameters for extraction.
#[cfg(feature = "parallel")]
#[must_use]
pub fn extract_peaks_parallel(
    spectrogram: &Array2<f32>,
    config: &PeakExtractorConfig,
) -> Vec<Peak> {
    let spec = spectrogram.as_standard_layout();
    let (n_frames, n_bins) = spec.dim();
    let Some(data) = spec.as_slice() else {
        return Vec::new();
    };

    let mut peaks: Vec<Peak> = (0..n_frames)
        .into_par_iter()
        .flat_map(|frame| frame_peaks(data, n_frames, n_bins, frame, config))
        .collect();
    sort_peaks(&mut peaks);
    peaks
}

/// Sorts peaks by time, then by frequency, for deterministic output.
fn sort_peaks(peaks: &mut [Peak]) {
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
}

/// Whether `val` at `(frame, bin)` is strictly greater than every other bin
/// in its neighborhood. Exits at the first neighbor that is `>= val`.
fn is_strict_local_max(
    data: &[f32],
    n_frames: usize,
    n_bins: usize,
    (frame, bin): (usize, usize),
    val: f32,
    config: &PeakExtractorConfig,
) -> bool {
    let f_lo = bin.saturating_sub(config.freq_neighborhood);
    let f_hi = (bin + config.freq_neighborhood + 1).min(n_bins);
    let t_lo = frame.saturating_sub(config.time_neighborhood);
    let t_hi = (frame + config.time_neighborhood + 1).min(n_frames);

    // Same frame first: adjacent bins are the likeliest to dominate.
    let row = &data[frame * n_bins..(frame + 1) * n_bins];
    if row[f_lo..bin].iter().any(|&v| v >= val) || row[bin + 1..f_hi].iter().any(|&v| v >= val) {
        return false;
    }
    (t_lo..t_hi).filter(|&t| t != frame).all(|t| {
        data[t * n_bins + f_lo..t * n_bins + f_hi]
            .iter()
            .all(|&v| v < val)
    })
}

/// Vertex offset of the parabola through three equally spaced samples
/// `(y0, y1, y2)` where `y1` is the maximum: a value in `(-0.5, 0.5)`.
fn parabolic_offset(y0: f32, y1: f32, y2: f32) -> f32 {
    let denom = y0 - 2.0 * y1 + y2;
    if denom < -f32::EPSILON {
        (0.5 * (y0 - y2) / denom).clamp(-0.5, 0.5)
    } else {
        0.0
    }
}

/// Sub-bin frequency offset (in bins) of the peak at `(frame, bin)`. Peaks
/// on the first or last bin are left uninterpolated.
fn freq_interpolation_offset(data: &[f32], n_bins: usize, (frame, bin): (usize, usize)) -> f32 {
    if bin == 0 || bin + 1 >= n_bins {
        return 0.0;
    }
    let row = &data[frame * n_bins..(frame + 1) * n_bins];
    parabolic_offset(row[bin - 1], row[bin], row[bin + 1])
}

/// Peaks in one frame: bins at or above the threshold that are the strict
/// maximum of their neighborhood.
#[allow(clippy::cast_precision_loss)]
fn frame_peaks(
    data: &[f32],
    n_frames: usize,
    n_bins: usize,
    frame: usize,
    config: &PeakExtractorConfig,
) -> Vec<Peak> {
    let (freq_res, time_res) = (config.freq_per_bin(), config.time_per_frame());
    let base = frame * n_bins;
    let mut out = Vec::new();

    for bin in 0..n_bins {
        let val = data[base + bin];
        if val < config.threshold_db {
            continue;
        }
        if is_strict_local_max(data, n_frames, n_bins, (frame, bin), val, config) {
            let df = if config.interpolate {
                freq_interpolation_offset(data, n_bins, (frame, bin))
            } else {
                0.0
            };
            out.push(Peak {
                time: frame as f32 * time_res,
                freq: (bin as f32 + df) * freq_res,
                amplitude: val,
            });
        }
    }
    out
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

    /// The original brute-force implementation, kept as a test oracle.
    #[allow(clippy::cast_precision_loss)]
    fn extract_peaks_naive(spectrogram: &Array2<f32>, config: &PeakExtractorConfig) -> Vec<Peak> {
        let (n_frames, n_bins) = spectrogram.dim();
        let (t_neigh, f_neigh) = (config.time_neighborhood, config.freq_neighborhood);
        let (freq_res, time_res) = (config.freq_per_bin(), config.time_per_frame());
        let mut peaks = Vec::new();
        for frame in 0..n_frames {
            for bin in 0..n_bins {
                let val = spectrogram[[frame, bin]];
                if val < config.threshold_db {
                    continue;
                }
                let mut is_max = true;
                'outer: for t in frame.saturating_sub(t_neigh)..(frame + t_neigh + 1).min(n_frames)
                {
                    for f in bin.saturating_sub(f_neigh)..(bin + f_neigh + 1).min(n_bins) {
                        if (t, f) != (frame, bin) && spectrogram[[t, f]] >= val {
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
        sort_peaks(&mut peaks);
        peaks
    }

    /// Pseudo-random spectrogram with coarse quantization so ties are common.
    fn random_spectrogram(n_frames: usize, n_bins: usize, seed: u64, levels: u32) -> Array2<f32> {
        let mut state = seed | 1;
        Array2::from_shape_fn((n_frames, n_bins), |_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            -60.0 + ((state >> 33) as u32 % levels) as f32 * (60.0 / levels as f32)
        })
    }

    #[test]
    fn test_matches_naive_oracle_including_ties_and_edges() {
        for (seed, levels, tn, fn_) in [
            (1_u64, 4_u32, 3_usize, 3_usize),
            (2, 4, 10, 10),
            (3, 1000, 5, 7),
            (4, 3, 1, 1),
            (5, 8, 0, 4),
            (6, 8, 4, 0),
            (7, 6, 40, 40), // window larger than the whole spectrogram
        ] {
            let spec = random_spectrogram(37, 53, seed, levels);
            let config = PeakExtractorConfig {
                time_neighborhood: tn,
                freq_neighborhood: fn_,
                threshold_db: -40.0,
                ..PeakExtractorConfig::default()
            };
            let expected = extract_peaks_naive(&spec, &config);
            assert_eq!(
                extract_peaks(&spec, &config),
                expected,
                "serial mismatch (seed={seed}, levels={levels}, tn={tn}, fn={fn_})"
            );
            #[cfg(feature = "parallel")]
            assert_eq!(
                extract_peaks_parallel(&spec, &config),
                expected,
                "parallel mismatch (seed={seed}, levels={levels}, tn={tn}, fn={fn_})"
            );
        }
    }

    #[test]
    fn test_non_standard_layout_input() {
        let spec = random_spectrogram(30, 40, 9, 50);
        let transposed_twice = spec.t().to_owned().reversed_axes(); // F-order view of same data
        let config = PeakExtractorConfig::default();
        assert_eq!(
            extract_peaks(&transposed_twice, &config),
            extract_peaks_naive(&spec, &config)
        );
    }

    #[test]
    fn test_parabolic_offset_recovers_vertex() {
        // y = -(x - 0.3)^2 sampled at x = -1, 0, 1.
        let f = |x: f32| -(x - 0.3) * (x - 0.3);
        let off = parabolic_offset(f(-1.0), f(0.0), f(1.0));
        assert!((off - 0.3).abs() < 1e-5, "offset = {off}");
        assert_eq!(parabolic_offset(1.0, 1.0, 1.0), 0.0); // flat -> no shift
    }

    #[test]
    fn test_interpolation_recovers_subbin_frequency() {
        // A Gaussian-shaped ridge whose true center is 0.35 bins above bin 40.
        let center = 40.35_f32;
        let spec = Array2::from_shape_fn((30, 100), |(t, f)| {
            let d = f as f32 - center;
            let dt = t as f32 - 15.0;
            -10.0 - 4.0 * d * d - 2.0 * dt * dt
        });
        let base = PeakExtractorConfig::new(5, 5, -60.0, 22_050, 512, 2048);
        let plain = extract_peaks(&spec, &base);
        let refined = extract_peaks(&spec, &base.clone().with_interpolation(true));
        assert_eq!((plain.len(), refined.len()), (1, 1));

        let bin_hz = 22_050.0 / 2048.0;
        assert!((plain[0].freq - 40.0 * bin_hz).abs() < 1e-3);
        assert!(
            (refined[0].freq - center * bin_hz).abs() < 0.05 * bin_hz,
            "refined freq {} vs true {}",
            refined[0].freq,
            center * bin_hz
        );
        // Time is never interpolated.
        assert!((refined[0].time - plain[0].time).abs() < f32::EPSILON);
    }

    #[test]
    fn test_interpolation_never_moves_a_peak_more_than_half_a_bin_or_in_time() {
        let spec = random_spectrogram(40, 60, 11, 500);
        let cfg = PeakExtractorConfig::new(3, 3, -40.0, 22_050, 512, 2048);
        let plain = extract_peaks(&spec, &cfg);
        let refined = extract_peaks(&spec, &cfg.clone().with_interpolation(true));
        assert_eq!(plain.len(), refined.len());
        let bin_hz = 22_050.0 / 2048.0;
        // (Interpolation can reorder peaks, so match each one by proximity.)
        for a in &plain {
            assert!(
                refined.iter().any(|b| {
                    (a.freq - b.freq).abs() <= 0.5 * bin_hz + 1e-3
                        && (a.time - b.time).abs() < f32::EPSILON
                        && (a.amplitude - b.amplitude).abs() < f32::EPSILON
                }),
                "no refined counterpart within half a bin for {a:?}"
            );
        }
    }

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
    fn test_tied_peaks_are_both_suppressed() {
        // Two peaks with the exact same amplitude within each other's
        // neighborhood: neither is a *strict* maximum, so both must be
        // suppressed.
        let spec = make_test_spectrogram(50, 129, &[(25, 64, -10.0), (26, 65, -10.0)]);
        let config = PeakExtractorConfig {
            time_neighborhood: 5,
            freq_neighborhood: 5,
            threshold_db: -40.0,
            ..PeakExtractorConfig::default()
        };
        let peaks = extract_peaks(&spec, &config);
        assert!(
            peaks.is_empty(),
            "tied peaks should both be suppressed, got {peaks:?}"
        );
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
