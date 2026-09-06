//! Pitch-shift / time-stretch-robust identification.
//!
//! Extracts real spectral peaks from a synthetic track, then applies a
//! *uniform* pitch shift (scale every peak's frequency by a constant
//! factor) and a *uniform* time stretch (scale every peak's time by a
//! constant factor) directly to the peak constellation -- the cleanest way
//! to isolate exactly the transform triplet hashing is designed to survive,
//! without needing a full audio resampler/pitch-shifter. Compares default
//! pairwise hashing (expected to fail, per `ARCHITECTURE.md`'s documented
//! limitation) against `--robust`-style triplet hashing
//! (`Fingerprinter::with_triplet_hashing`).
//!
//! This also surfaces a real nuance documented in `ARCHITECTURE.md`: triplet
//! hashing's *raw hash overlap* improves under both distortions, but
//! `Index::query`'s reported `confidence` -- which depends on its
//! time-offset histogram concentrating hits into a single bin -- only fully
//! reflects that under pitch shift. Under time stretch, matching anchors'
//! absolute times drift apart across the track, spreading hits over many
//! offset bins and diluting the histogram-based score even though the
//! underlying hashes matched far better than pairwise's would have.
//!
//! Run with: `cargo run --example 05_robust_pitch_shift`

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

#[path = "common/mod.rs"]
mod common;

use wavio::dsp::peaks::{Peak, PeakExtractorConfig, extract_peaks};
use wavio::dsp::spectrogram::{SpectrogramConfig, compute_spectrogram};
use wavio::hash::{HashConfig, generate_hashes};
use wavio::index::Index;
use wavio::triplet::{TripletHashConfig, generate_triplet_hashes};

const SAMPLE_RATE: u32 = 22_050;

fn main() {
    let samples = common::synth_track(15.0, SAMPLE_RATE, 2.0);

    let spectrogram_config = SpectrogramConfig::default();
    let peak_config = PeakExtractorConfig::default();
    let spec = compute_spectrogram(&samples, &spectrogram_config).expect("spectrogram failed");
    let peaks = extract_peaks(&spec, &peak_config);
    println!("Extracted {} real peaks from the track.\n", peaks.len());

    // A uniform +4% pitch shift (every frequency scaled by the same factor)
    // and a uniform +3% time stretch (every peak's timing scaled by the
    // same factor) -- exactly the distortions triplet hashing's
    // ratio-based encoding is invariant to.
    let pitch_shifted: Vec<Peak> = peaks
        .iter()
        .map(|p| Peak::new(p.time, p.freq * 1.04, p.amplitude))
        .collect();
    let time_stretched: Vec<Peak> = peaks
        .iter()
        .map(|p| Peak::new(p.time * 1.03, p.freq, p.amplitude))
        .collect();

    println!("=== Default (pairwise) hashing ===\n");
    run_scenario(&peaks, &pitch_shifted, &time_stretched, |p| {
        generate_hashes(p, &HashConfig::default())
    });

    println!("\n=== Robust (triplet) hashing ===\n");
    run_scenario(&peaks, &pitch_shifted, &time_stretched, |p| {
        generate_triplet_hashes(p, &TripletHashConfig::default())
    });
}

fn run_scenario(
    original: &[Peak],
    pitch_shifted: &[Peak],
    time_stretched: &[Peak],
    hash_fn: impl Fn(&[Peak]) -> Vec<wavio::hash::Fingerprint>,
) {
    let original_fps = hash_fn(original);
    println!("  ({} original fingerprints indexed)", original_fps.len());

    let mut index = Index::default();
    index.insert("original_track", &original_fps);

    let original_hashes: std::collections::HashSet<u64> =
        original_fps.iter().map(|fp| fp.hash).collect();

    for (label, transformed_peaks) in [
        ("+4% pitch shift", pitch_shifted),
        ("+3% time stretch", time_stretched),
    ] {
        let query_fps = hash_fn(transformed_peaks);

        // Raw hash-level overlap -- ignores timing/offset entirely, purely
        // "did the same hash values reappear".
        let query_hashes: std::collections::HashSet<u64> =
            query_fps.iter().map(|fp| fp.hash).collect();
        let overlap = original_hashes.intersection(&query_hashes).count();
        let overlap_pct = 100.0 * overlap as f64 / query_hashes.len().max(1) as f64;

        // Index-level result -- depends on the offset histogram concentrating
        // hits into one bin, not just on hashes matching (see module docs).
        let index_summary = match index.query(&query_fps) {
            Some(result) => format!("confidence {:.1}%", result.confidence * 100.0),
            None => "no match".to_string(),
        };

        println!(
            "  {label}: raw hash overlap {overlap_pct:5.1}%  |  Index::query -> {index_summary}"
        );
    }
}
