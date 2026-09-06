//! Pitch-shift / time-stretch-robust combinatorial hashing.
//!
//! The pairwise hashes in [`crate::hash`] encode **absolute** frequency bins
//! and an absolute time delta. Any uniform pitch shift (all frequencies
//! scaled by a constant factor) or time stretch (all time deltas scaled by a
//! constant factor) changes every one of those values, so the hash changes —
//! this is why peak-hashing fingerprinters are traditionally brittle against
//! those two distortions.
//!
//! This module instead hashes **ratios** within a triplet of peaks
//! `(anchor, t1, t2)`:
//!
//! - `freq_ratio_1 = t1.freq / anchor.freq` and
//!   `freq_ratio_2 = t2.freq / anchor.freq` — under a uniform pitch shift by
//!   factor `s`, numerator and denominator both scale by `s`, so the ratio is
//!   unchanged (modulo FFT bin quantization noise on the underlying peak
//!   frequencies).
//! - `time_ratio = (t2.time - anchor.time) / (t1.time - anchor.time)` — under
//!   a uniform time stretch by factor `r`, both deltas scale by `r`, so this
//!   ratio is likewise unchanged.
//!
//! This is the same principle behind Panako's Constant-Q triplet hashing.
//! The output is still a plain [`crate::hash::Fingerprint`] with `anchor_time
//! = anchor.time`, so it slots into [`crate::index::Index`] /
//! [`crate::persist::PersistentIndex`] without any changes to those types.
//!
//! **Caveat**: the index's time-offset histogram still correlates on
//! absolute `anchor_time`, which drifts under real time stretch (though not
//! under pitch shift alone). Triplet hashing improves hash-level match
//! recall under both distortions, but offset/timing estimation accuracy
//! under time stretch is not addressed by this module. See
//! `ARCHITECTURE.md` for details.

use crate::dsp::peaks::Peak;
use crate::hash::Fingerprint;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

/// Frequencies below this are excluded from ratio computation to avoid
/// near-zero denominators blowing up the ratio.
const MIN_FREQ_HZ: f32 = 1.0;

/// Configuration for triplet (pitch/time-stretch-robust) hashing.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TripletHashConfig {
    /// Maximum number of target peaks to consider per anchor peak.
    /// Each adjacent pair among the selected targets forms one triplet, so
    /// an anchor produces at most `fan_value - 1` hashes.
    pub fan_value: usize,
    /// Minimum time difference (seconds) between anchor and a target peak.
    pub min_dt: f32,
    /// Maximum time difference (seconds) between anchor and a target peak.
    pub max_dt: f32,
    /// Quantization step, in `log2(ratio)` units, for the two frequency
    /// ratios. Smaller values give finer discrimination but a larger hash
    /// space.
    pub freq_ratio_resolution: f32,
    /// Quantization step, in `log2(ratio)` units, for the time ratio.
    pub time_ratio_resolution: f32,
}

impl Default for TripletHashConfig {
    fn default() -> Self {
        Self {
            fan_value: 15,
            min_dt: 0.0,
            max_dt: 1.0,
            freq_ratio_resolution: 0.01,
            time_ratio_resolution: 0.01,
        }
    }
}

impl TripletHashConfig {
    /// Creates a new `TripletHashConfig` with custom parameters.
    #[must_use]
    pub fn new(
        fan_value: usize,
        min_dt: f32,
        max_dt: f32,
        freq_ratio_resolution: f32,
        time_ratio_resolution: f32,
    ) -> Self {
        Self {
            fan_value,
            min_dt,
            max_dt,
            freq_ratio_resolution,
            time_ratio_resolution,
        }
    }
}

/// Quantizes a strictly-positive ratio into a 20-bit unsigned bucket by
/// log-scaling and bias-shifting it into range.
///
/// Ratios are quantized in `log2` space so that the *step size* is relative
/// rather than absolute — appropriate since the invariance property this
/// module relies on is multiplicative (a constant scale factor), not
/// additive.
#[inline]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn quantize_ratio(ratio: f32, resolution: f32) -> u32 {
    const BIAS: i32 = 1 << 19; // centers log2(ratio) == 0 (ratio == 1.0) at the middle of the 20-bit range

    // Guard against `ratio <= 0.0` producing `-inf`/NaN from `log2`, and clamp
    // the step count into a safe `i32` range *before* adding `BIAS` so the
    // addition itself can never overflow.
    let log_ratio = ratio.max(f32::MIN_POSITIVE).log2();
    let steps = (log_ratio / resolution)
        .round()
        .clamp(-f64::from(BIAS) as f32, f64::from(BIAS) as f32 - 1.0) as i32;
    (steps + BIAS).clamp(0, (1 << 20) - 1) as u32
}

/// Encodes two quantized frequency ratios and a quantized time ratio into a
/// single `u64`, using the same bit budget as [`crate::hash`]'s pairwise
/// hash for symmetry.
///
/// Bit layout (from MSB to LSB):
/// - Bits 40..59: `freq_ratio_1` (20 bits)
/// - Bits 20..39: `freq_ratio_2` (20 bits)
/// - Bits  0..19: `time_ratio`   (20 bits)
#[inline]
#[must_use]
fn pack_triplet_hash(freq_ratio_1: u32, freq_ratio_2: u32, time_ratio: u32) -> u64 {
    let f1 = u64::from(freq_ratio_1 & 0xF_FFFF);
    let f2 = u64::from(freq_ratio_2 & 0xF_FFFF);
    let tr = u64::from(time_ratio & 0xF_FFFF);
    (f1 << 40) | (f2 << 20) | tr
}

/// Computes the hash for one `(anchor, t1, t2)` triplet, or `None` if the
/// triplet is degenerate (near-zero frequency or near-zero time delta).
#[inline]
fn hash_triplet(anchor: &Peak, t1: &Peak, t2: &Peak, config: &TripletHashConfig) -> Option<u64> {
    if anchor.freq < MIN_FREQ_HZ || t1.freq < MIN_FREQ_HZ {
        return None;
    }

    let dt1 = t1.time - anchor.time;
    if dt1 <= f32::EPSILON {
        return None;
    }

    let freq_ratio_1 = t1.freq / anchor.freq;
    let freq_ratio_2 = t2.freq / anchor.freq;
    let time_ratio = (t2.time - anchor.time) / dt1;

    let f1_q = quantize_ratio(freq_ratio_1, config.freq_ratio_resolution);
    let f2_q = quantize_ratio(freq_ratio_2, config.freq_ratio_resolution);
    let tr_q = quantize_ratio(time_ratio, config.time_ratio_resolution);

    Some(pack_triplet_hash(f1_q, f2_q, tr_q))
}

/// Generates pitch-shift / time-stretch-robust fingerprint hashes from a set
/// of spectral peaks.
///
/// For each anchor peak, up to `config.fan_value` subsequent peaks within
/// `[min_dt, max_dt]` are collected as candidates; each adjacent pair among
/// them forms one triplet `(anchor, t1, t2)`, hashed via ratio quantization
/// (see the module docs for the invariance rationale).
///
/// Peaks are sorted by time before pairing to guarantee determinism.
///
/// # Examples
///
/// ```
/// use wavio::dsp::peaks::Peak;
/// use wavio::triplet::{generate_triplet_hashes, TripletHashConfig};
///
/// let peaks = vec![
///     Peak::new(0.0, 440.0, -10.0),
///     Peak::new(0.2, 880.0, -10.0),
///     Peak::new(0.4, 660.0, -10.0),
/// ];
/// let config = TripletHashConfig::default();
/// let fingerprints = generate_triplet_hashes(&peaks, &config);
///
/// // Three peaks -> one anchor with two candidates -> one triplet.
/// assert_eq!(fingerprints.len(), 1);
/// assert_eq!(fingerprints[0].anchor_time, 0.0);
/// ```
#[must_use]
pub fn generate_triplet_hashes(peaks: &[Peak], config: &TripletHashConfig) -> Vec<Fingerprint> {
    if peaks.len() < 3 {
        return Vec::new();
    }

    let mut sorted_peaks = peaks.to_vec();
    sorted_peaks.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut fingerprints = Vec::new();

    for (i, anchor) in sorted_peaks.iter().enumerate() {
        let candidates = collect_candidates(&sorted_peaks, i, config);

        for pair in candidates.windows(2) {
            if let Some(hash) = hash_triplet(anchor, pair[0], pair[1], config) {
                fingerprints.push(Fingerprint {
                    hash,
                    anchor_time: anchor.time,
                });
            }
        }
    }

    fingerprints
}

/// Collects up to `config.fan_value` candidate target peaks for the anchor
/// at `anchor_idx`, within `[config.min_dt, config.max_dt]` of it.
fn collect_candidates<'a>(
    sorted_peaks: &'a [Peak],
    anchor_idx: usize,
    config: &TripletHashConfig,
) -> Vec<&'a Peak> {
    let anchor = &sorted_peaks[anchor_idx];
    let mut candidates = Vec::with_capacity(config.fan_value);

    for target in sorted_peaks.iter().skip(anchor_idx + 1) {
        if candidates.len() >= config.fan_value {
            break;
        }

        let dt = target.time - anchor.time;
        if dt < config.min_dt {
            continue;
        }
        if dt > config.max_dt {
            break; // Sorted by time, so all subsequent targets are also too far.
        }

        candidates.push(target);
    }

    candidates
}

/// Parallel variant of [`generate_triplet_hashes`] using `rayon`.
///
/// Each anchor's candidate collection and triplet hashing is independent
/// (read-only access to `sorted_peaks`), making this safely parallelizable.
///
/// Requires the `parallel` feature flag.
#[cfg(feature = "parallel")]
#[must_use]
pub fn generate_triplet_hashes_parallel(
    peaks: &[Peak],
    config: &TripletHashConfig,
) -> Vec<Fingerprint> {
    if peaks.len() < 3 {
        return Vec::new();
    }

    let mut sorted_peaks = peaks.to_vec();
    sorted_peaks.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    sorted_peaks
        .par_iter()
        .enumerate()
        .flat_map(|(i, anchor)| {
            let candidates = collect_candidates(&sorted_peaks, i, config);
            let mut local = Vec::new();

            for pair in candidates.windows(2) {
                if let Some(hash) = hash_triplet(anchor, pair[0], pair[1], config) {
                    local.push(Fingerprint {
                        hash,
                        anchor_time: anchor.time,
                    });
                }
            }

            local
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_peak(time: f32, freq: f32) -> Peak {
        Peak {
            time,
            freq,
            amplitude: -10.0,
        }
    }

    #[test]
    fn test_generate_triplet_hashes_empty() {
        let config = TripletHashConfig::default();
        assert!(generate_triplet_hashes(&[], &config).is_empty());
    }

    #[test]
    fn test_generate_triplet_hashes_too_few_peaks() {
        let config = TripletHashConfig::default();
        let peaks = vec![make_peak(0.0, 440.0), make_peak(0.2, 880.0)];
        assert!(generate_triplet_hashes(&peaks, &config).is_empty());
    }

    #[test]
    fn test_generate_triplet_hashes_determinism() {
        let peaks = vec![
            make_peak(0.0, 440.0),
            make_peak(0.2, 880.0),
            make_peak(0.5, 660.0),
            make_peak(0.8, 330.0),
        ];
        let config = TripletHashConfig::default();

        let a = generate_triplet_hashes(&peaks, &config);
        let b = generate_triplet_hashes(&peaks, &config);

        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.hash, y.hash);
            assert!((x.anchor_time - y.anchor_time).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn test_pack_triplet_hash_bit_layout() {
        let h = pack_triplet_hash(1, 2, 3);
        assert_eq!((h >> 40) & 0xF_FFFF, 1);
        assert_eq!((h >> 20) & 0xF_FFFF, 2);
        assert_eq!(h & 0xF_FFFF, 3);
    }

    #[test]
    fn test_quantize_ratio_identity_centers_at_bias() {
        // ratio == 1.0 -> log2(ratio) == 0.0 -> centered exactly at the bias.
        let q = quantize_ratio(1.0, 0.01);
        assert_eq!(q, 1 << 19);
    }

    #[test]
    fn test_quantize_ratio_monotonic() {
        let q_low = quantize_ratio(0.5, 0.01);
        let q_mid = quantize_ratio(1.0, 0.01);
        let q_high = quantize_ratio(2.0, 0.01);
        assert!(q_low < q_mid);
        assert!(q_mid < q_high);
    }

    #[test]
    fn test_fan_value_bounds_triplet_count() {
        // Many target peaks within the window -- candidates capped at fan_value,
        // so triplets per anchor are capped at fan_value - 1.
        let mut peaks = vec![make_peak(0.0, 440.0)];
        for i in 1..=20 {
            #[allow(clippy::cast_precision_loss)]
            let t = i as f32 * 0.04; // all within 0.8s < default max_dt of 1.0
            peaks.push(make_peak(t, 440.0 + t * 100.0));
        }
        let config = TripletHashConfig {
            fan_value: 5,
            ..TripletHashConfig::default()
        };
        let hashes = generate_triplet_hashes(&peaks, &config);

        let mut counts = std::collections::HashMap::<u64, usize>::new();
        for fp in &hashes {
            let key = u64::from(fp.anchor_time.to_bits());
            *counts.entry(key).or_insert(0) += 1;
        }
        for &count in counts.values() {
            assert!(
                count < config.fan_value,
                "Anchor exceeded fan_value - 1 triplets: {count} > {}",
                config.fan_value - 1
            );
        }
    }

    #[test]
    fn test_pitch_shift_invariance() {
        // A uniform pitch shift (all frequencies scaled by a constant factor)
        // must leave the triplet hash unchanged, since freq ratios cancel it.
        let peaks = vec![
            make_peak(0.0, 440.0),
            make_peak(0.1, 660.0),
            make_peak(0.3, 880.0),
            make_peak(0.5, 550.0),
        ];
        let shifted: Vec<Peak> = peaks
            .iter()
            .map(|p| make_peak(p.time, p.freq * 1.05))
            .collect();

        let config = TripletHashConfig::default();
        let original = generate_triplet_hashes(&peaks, &config);
        let shifted_hashes = generate_triplet_hashes(&shifted, &config);

        assert_eq!(original.len(), shifted_hashes.len());
        for (a, b) in original.iter().zip(shifted_hashes.iter()) {
            assert_eq!(a.hash, b.hash);
        }
    }

    #[test]
    fn test_time_stretch_invariance() {
        // A uniform time stretch (all time deltas scaled by a constant factor)
        // must leave the triplet hash unchanged, since time ratios cancel it.
        let peaks = vec![
            make_peak(0.0, 440.0),
            make_peak(0.1, 660.0),
            make_peak(0.3, 880.0),
            make_peak(0.5, 550.0),
        ];
        let stretched: Vec<Peak> = peaks
            .iter()
            .map(|p| make_peak(p.time * 1.03, p.freq))
            .collect();

        let config = TripletHashConfig::default();
        let original = generate_triplet_hashes(&peaks, &config);
        let stretched_hashes = generate_triplet_hashes(&stretched, &config);

        assert_eq!(original.len(), stretched_hashes.len());
        for (a, b) in original.iter().zip(stretched_hashes.iter()) {
            assert_eq!(a.hash, b.hash);
        }
    }
}
