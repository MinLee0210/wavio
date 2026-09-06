//! Integration tests demonstrating that triplet (pitch/time-stretch-robust)
//! hashing actually delivers on its invariance claim, in contrast to the
//! default pairwise hashing which does not.
//!
//! These operate directly on synthetic `Peak` lists (rather than real
//! resampled/pitch-shifted audio) so the applied transform factor is exact
//! and the test is fast and deterministic. Comparisons are *relative*
//! (triplet overlap must clearly exceed pairwise overlap) rather than a
//! knife's-edge absolute threshold on pairwise alone, since coarse
//! quantization can let a fraction of pairwise hashes survive a small
//! perturbation purely by chance.

use std::collections::HashSet;

use wavio::dsp::peaks::Peak;
use wavio::hash::{HashConfig, generate_hashes};
use wavio::triplet::{TripletHashConfig, generate_triplet_hashes};

fn make_peak(time: f32, freq: f32) -> Peak {
    Peak::new(time, freq, -10.0)
}

/// A synthetic constellation with enough peaks to form many pairs/triplets.
fn synthetic_peaks() -> Vec<Peak> {
    (0..80)
        .map(|i| {
            let t = i as f32 * 0.05;
            let freq = 200.0 + 50.0 * (i as f32 * 0.3).sin().abs() + (i as f32) * 8.0;
            make_peak(t, freq)
        })
        .collect()
}

fn overlap_ratio(a: &HashSet<u64>, b: &HashSet<u64>) -> f64 {
    let union = a.union(b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(b).count() as f64 / union as f64
}

#[test]
fn triplet_hashing_is_far_more_pitch_shift_robust_than_pairwise() {
    let peaks = synthetic_peaks();
    let shifted: Vec<Peak> = peaks
        .iter()
        .map(|p| make_peak(p.time, p.freq * 1.05))
        .collect();

    let pairwise_config = HashConfig::default();
    let pairwise_original: HashSet<u64> = generate_hashes(&peaks, &pairwise_config)
        .into_iter()
        .map(|fp| fp.hash)
        .collect();
    let pairwise_shifted: HashSet<u64> = generate_hashes(&shifted, &pairwise_config)
        .into_iter()
        .map(|fp| fp.hash)
        .collect();
    let pairwise_ratio = overlap_ratio(&pairwise_original, &pairwise_shifted);

    let triplet_config = TripletHashConfig::default();
    let triplet_original: HashSet<u64> = generate_triplet_hashes(&peaks, &triplet_config)
        .into_iter()
        .map(|fp| fp.hash)
        .collect();
    let triplet_shifted: HashSet<u64> = generate_triplet_hashes(&shifted, &triplet_config)
        .into_iter()
        .map(|fp| fp.hash)
        .collect();
    let triplet_ratio = overlap_ratio(&triplet_original, &triplet_shifted);

    assert!(
        triplet_ratio > 0.8,
        "expected triplet hashing to survive a 5% pitch shift, overlap was {triplet_ratio:.3}"
    );
    assert!(
        triplet_ratio - pairwise_ratio > 0.5,
        "expected triplet hashing to clearly outperform pairwise under pitch shift: \
         triplet={triplet_ratio:.3}, pairwise={pairwise_ratio:.3}"
    );
}

#[test]
fn triplet_hashing_is_far_more_time_stretch_robust_than_pairwise() {
    let peaks = synthetic_peaks();
    let stretched: Vec<Peak> = peaks
        .iter()
        .map(|p| make_peak(p.time * 1.03, p.freq))
        .collect();

    let pairwise_config = HashConfig::default();
    let pairwise_original: HashSet<u64> = generate_hashes(&peaks, &pairwise_config)
        .into_iter()
        .map(|fp| fp.hash)
        .collect();
    let pairwise_stretched: HashSet<u64> = generate_hashes(&stretched, &pairwise_config)
        .into_iter()
        .map(|fp| fp.hash)
        .collect();
    let pairwise_ratio = overlap_ratio(&pairwise_original, &pairwise_stretched);

    let triplet_config = TripletHashConfig::default();
    let triplet_original: HashSet<u64> = generate_triplet_hashes(&peaks, &triplet_config)
        .into_iter()
        .map(|fp| fp.hash)
        .collect();
    let triplet_stretched: HashSet<u64> = generate_triplet_hashes(&stretched, &triplet_config)
        .into_iter()
        .map(|fp| fp.hash)
        .collect();
    let triplet_ratio = overlap_ratio(&triplet_original, &triplet_stretched);

    assert!(
        triplet_ratio > 0.8,
        "expected triplet hashing to survive a 3% time stretch, overlap was {triplet_ratio:.3}"
    );
    assert!(
        triplet_ratio - pairwise_ratio > 0.5,
        "expected triplet hashing to clearly outperform pairwise under time stretch: \
         triplet={triplet_ratio:.3}, pairwise={pairwise_ratio:.3}"
    );
}
