//! Combinatorial hashing of spectral peaks.
//!
//! This module generates deterministic fingerprint hashes from
//! peak constellations using time-frequency pair encoding.
//!
//! Each hash is a `u64` encoding `(freq1_bin, freq2_bin, delta_t_quantized)`,
//! paired with the anchor time `t1` of the first peak.

use crate::dsp::peaks::Peak;

#[cfg(feature = "parallel")]
use rayon::prelude::*;


/// Configuration for the combinatorial hashing algorithm.
#[derive(Debug, Clone)]
pub struct HashConfig {
    /// Maximum number of target peaks to pair with each anchor peak.
    pub fan_value: usize,
    /// Minimum time difference (seconds) between anchor and target peaks.
    pub min_dt: f32,
    /// Maximum time difference (seconds) between anchor and target peaks.
    pub max_dt: f32,
    /// Number of frequency bins used for quantization.
    /// Peaks are quantized to `freq_bins` discrete levels before hashing.
    pub freq_bins: u32,
    /// Frequency resolution (Hz per bin) of the spectrogram that produced
    /// the peaks. Used to convert Hz values to bin indices.
    pub freq_resolution: f32,
    /// Time resolution (seconds per step) for quantizing `delta_t`.
    /// Smaller values give finer timing precision but larger hash space.
    pub dt_resolution: f32,
}

impl Default for HashConfig {
    fn default() -> Self {
        Self {
            fan_value: 15,
            min_dt: 0.0,
            max_dt: 1.0,
            freq_bins: 1024,
            freq_resolution: 22_050.0 / 2048.0, // ~10.77 Hz per bin
            dt_resolution: 0.01,                 // 10 ms quantization
        }
    }
}

/// A single fingerprint hash paired with its anchor time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fingerprint {
    /// The combinatorial hash value.
    pub hash: u64,
    /// Anchor time in seconds (the time of the first peak in the pair).
    pub anchor_time: f32,
}

/// Quantizes a frequency value (Hz) to a bin index.
#[inline]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn freq_to_bin(freq: f32, freq_resolution: f32, max_bins: u32) -> u32 {
    let bin = (freq / freq_resolution).round() as u32;
    bin.min(max_bins.saturating_sub(1))
}

/// Quantizes a time delta (seconds) to an integer step.
#[inline]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn quantize_dt(dt: f32, dt_resolution: f32) -> u32 {
    (dt / dt_resolution).round() as u32
}

/// Encodes two frequency bins and a quantized delta-t into a single `u64`.
///
/// Bit layout (from MSB to LSB):
/// - Bits 40..59: `freq1_bin` (20 bits, max 1,048,575)
/// - Bits 20..39: `freq2_bin` (20 bits, max 1,048,575)
/// - Bits  0..19: `delta_t`   (20 bits, max 1,048,575)
#[inline]
#[must_use]
fn pack_hash(freq1_bin: u32, freq2_bin: u32, delta_t: u32) -> u64 {
    let f1 = u64::from(freq1_bin & 0xF_FFFF);
    let f2 = u64::from(freq2_bin & 0xF_FFFF);
    let dt = u64::from(delta_t & 0xF_FFFF);
    (f1 << 40) | (f2 << 20) | dt
}

/// Generates combinatorial fingerprint hashes from a set of spectral peaks.
///
/// For each anchor peak, the algorithm pairs it with up to `config.fan_value`
/// subsequent peaks that fall within the `[min_dt, max_dt]` time window.
/// Each pair produces a deterministic `u64` hash via bit-packing.
///
/// Peaks are sorted by time before pairing to guarantee determinism.
///
/// # Arguments
///
/// * `peaks` -- Constellation peaks extracted from a spectrogram.
/// * `config` -- Tuning parameters for hash generation.
///
/// # Returns
///
/// A vector of `Fingerprint` values, each containing the hash and its anchor time.
#[must_use]
pub fn generate_hashes(peaks: &[Peak], config: &HashConfig) -> Vec<Fingerprint> {
    if peaks.is_empty() {
        return Vec::new();
    }

    // Sort peaks by time for deterministic pairing.
    let mut sorted_peaks = peaks.to_vec();
    sorted_peaks.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut fingerprints = Vec::new();

    for (i, anchor) in sorted_peaks.iter().enumerate() {
        let mut fan_count = 0;

        for target in sorted_peaks.iter().skip(i + 1) {
            if fan_count >= config.fan_value {
                break;
            }

            let dt = target.time - anchor.time;

            // Skip pairs outside the time window.
            if dt < config.min_dt {
                continue;
            }
            if dt > config.max_dt {
                break; // Sorted by time, so all subsequent targets are also too far.
            }

            let f1_bin = freq_to_bin(anchor.freq, config.freq_resolution, config.freq_bins);
            let f2_bin = freq_to_bin(target.freq, config.freq_resolution, config.freq_bins);
            let dt_q = quantize_dt(dt, config.dt_resolution);
            let hash = pack_hash(f1_bin, f2_bin, dt_q);

            fingerprints.push(Fingerprint {
                hash,
                anchor_time: anchor.time,
            });

            fan_count += 1;
        }
    }

    fingerprints
}

/// Parallel variant of [`generate_hashes`] using `rayon`.
///
/// Distributes anchor-level pairing across the thread pool. Each anchor's
/// target-pair computation is independent (read-only access to `sorted_peaks`),
/// making this safely parallelizable. The final hash list is identical to the
/// serial version modulo ordering (both are unsorted; callers should not rely
/// on order).
///
/// Requires the `parallel` feature flag.
///
/// # Arguments
///
/// * `peaks` -- Constellation peaks extracted from a spectrogram.
/// * `config` -- Tuning parameters for hash generation.
#[cfg(feature = "parallel")]
#[must_use]
pub fn generate_hashes_parallel(peaks: &[Peak], config: &HashConfig) -> Vec<Fingerprint> {
    if peaks.is_empty() {
        return Vec::new();
    }

    let mut sorted_peaks = peaks.to_vec();
    sorted_peaks.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Each anchor produces its own sub-vec of Fingerprints independently.
    sorted_peaks
        .par_iter()
        .enumerate()
        .flat_map(|(i, anchor)| {
            let mut local = Vec::new();
            let mut fan_count = 0;

            for target in sorted_peaks.iter().skip(i + 1) {
                if fan_count >= config.fan_value {
                    break;
                }

                let dt = target.time - anchor.time;

                if dt < config.min_dt {
                    continue;
                }
                if dt > config.max_dt {
                    break;
                }

                let f1_bin = freq_to_bin(anchor.freq, config.freq_resolution, config.freq_bins);
                let f2_bin = freq_to_bin(target.freq, config.freq_resolution, config.freq_bins);
                let dt_q = quantize_dt(dt, config.dt_resolution);
                let hash = pack_hash(f1_bin, f2_bin, dt_q);

                local.push(Fingerprint {
                    hash,
                    anchor_time: anchor.time,
                });

                fan_count += 1;
            }

            local
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::peaks::Peak;
    use std::collections::HashSet;

    fn make_peak(time: f32, freq: f32) -> Peak {
        Peak {
            time,
            freq,
            amplitude: -10.0,
        }
    }

    #[test]
    fn test_pack_hash_determinism() {
        let h1 = pack_hash(100, 200, 50);
        let h2 = pack_hash(100, 200, 50);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_pack_hash_different_inputs() {
        let h1 = pack_hash(100, 200, 50);
        let h2 = pack_hash(100, 201, 50);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_pack_hash_bit_layout() {
        let h = pack_hash(1, 2, 3);
        // freq1=1 at bits 40..59, freq2=2 at bits 20..39, dt=3 at bits 0..19
        assert_eq!((h >> 40) & 0xF_FFFF, 1);
        assert_eq!((h >> 20) & 0xF_FFFF, 2);
        assert_eq!(h & 0xF_FFFF, 3);
    }

    #[test]
    fn test_generate_hashes_empty() {
        let config = HashConfig::default();
        let hashes = generate_hashes(&[], &config);
        assert!(hashes.is_empty());
    }

    #[test]
    fn test_generate_hashes_single_peak() {
        // A single peak cannot form any pairs.
        let peaks = vec![make_peak(0.0, 440.0)];
        let config = HashConfig::default();
        let hashes = generate_hashes(&peaks, &config);
        assert!(hashes.is_empty());
    }

    #[test]
    fn test_generate_hashes_determinism() {
        let peaks = vec![
            make_peak(0.0, 440.0),
            make_peak(0.2, 880.0),
            make_peak(0.5, 660.0),
            make_peak(0.8, 330.0),
        ];
        let config = HashConfig::default();

        let hashes_a = generate_hashes(&peaks, &config);
        let hashes_b = generate_hashes(&peaks, &config);

        assert_eq!(hashes_a.len(), hashes_b.len());
        for (a, b) in hashes_a.iter().zip(hashes_b.iter()) {
            assert_eq!(a.hash, b.hash);
            assert!((a.anchor_time - b.anchor_time).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn test_generate_hashes_respects_max_dt() {
        let peaks = vec![
            make_peak(0.0, 440.0),
            make_peak(2.0, 880.0), // dt = 2.0 > default max_dt of 1.0
        ];
        let config = HashConfig::default();
        let hashes = generate_hashes(&peaks, &config);
        assert!(hashes.is_empty());
    }

    #[test]
    fn test_generate_hashes_fan_value_limit() {
        // Create many target peaks -- fan_value should limit pairings.
        let mut peaks = vec![make_peak(0.0, 440.0)];
        for i in 1..=20 {
            #[allow(clippy::cast_precision_loss)]
            let t = i as f32 * 0.04; // All within 0.8s < max_dt
            peaks.push(make_peak(t, 440.0 + t * 100.0));
        }
        let config = HashConfig {
            fan_value: 5,
            ..HashConfig::default()
        };
        let hashes = generate_hashes(&peaks, &config);

        // The anchor at t=0 should produce exactly 5 pairs (fan_value),
        // but other anchors also pair forward, so total > 5.
        // Just check that no anchor produced MORE than fan_value pairings.
        let mut counts = std::collections::HashMap::<u64, usize>::new();
        for fp in &hashes {
            // Use anchor_time bits as key (approximate grouping).
            let key = fp.anchor_time.to_bits() as u64;
            *counts.entry(key).or_insert(0) += 1;
        }
        for &count in counts.values() {
            assert!(
                count <= config.fan_value,
                "Anchor exceeded fan_value: {count} > {}",
                config.fan_value
            );
        }
    }

    #[test]
    fn test_different_peak_sets_produce_different_hashes() {
        let peaks_a = vec![
            make_peak(0.0, 100.0),
            make_peak(0.3, 200.0),
            make_peak(0.6, 300.0),
        ];
        let peaks_b = vec![
            make_peak(0.0, 500.0),
            make_peak(0.3, 900.0),
            make_peak(0.6, 1200.0),
        ];
        let config = HashConfig::default();

        let set_a: HashSet<u64> = generate_hashes(&peaks_a, &config)
            .iter()
            .map(|fp| fp.hash)
            .collect();
        let set_b: HashSet<u64> = generate_hashes(&peaks_b, &config)
            .iter()
            .map(|fp| fp.hash)
            .collect();

        let intersection = set_a.intersection(&set_b).count();
        let total = set_a.len().max(set_b.len());

        // Collision rate should be below 5%.
        let collision_rate = if total > 0 {
            intersection as f64 / total as f64
        } else {
            0.0
        };
        assert!(
            collision_rate < 0.05,
            "Collision rate {collision_rate:.2} exceeds 5%"
        );
    }

    #[test]
    fn test_anchor_times_are_correct() {
        let peaks = vec![
            make_peak(0.1, 440.0),
            make_peak(0.5, 880.0),
        ];
        let config = HashConfig::default();
        let hashes = generate_hashes(&peaks, &config);
        assert_eq!(hashes.len(), 1);
        assert!((hashes[0].anchor_time - 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn test_unsorted_input_still_deterministic() {
        // Provide peaks in reverse time order -- should still produce same output.
        let peaks_fwd = vec![
            make_peak(0.0, 440.0),
            make_peak(0.3, 880.0),
            make_peak(0.6, 660.0),
        ];
        let peaks_rev = vec![
            make_peak(0.6, 660.0),
            make_peak(0.0, 440.0),
            make_peak(0.3, 880.0),
        ];
        let config = HashConfig::default();
        let hashes_fwd = generate_hashes(&peaks_fwd, &config);
        let hashes_rev = generate_hashes(&peaks_rev, &config);

        assert_eq!(hashes_fwd.len(), hashes_rev.len());
        for (a, b) in hashes_fwd.iter().zip(hashes_rev.iter()) {
            assert_eq!(a.hash, b.hash);
        }
    }
}
