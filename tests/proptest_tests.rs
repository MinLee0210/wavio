//! Property-based tests for the wavio library.
//!
//! Uses the `proptest` crate to verify invariants that must hold for
//! any valid input, not just hand-crafted examples.
//!
//! Properties verified:
//! 1. Fingerprinting is deterministic across runs.
//! 2. Querying an empty index always returns `None`.
//! 3. Score is monotonically higher for longer matching clips.

use proptest::prelude::*;
use wavio::dsp::peaks::Peak;
use wavio::hash::{generate_hashes, HashConfig};
use wavio::index::Index;

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// Generates an arbitrary `Peak` with plausible time/freq/amplitude values.
fn arb_peak() -> impl Strategy<Value = Peak> {
    (
        0.0_f32..60.0_f32,       // time: 0–60 seconds
        50.0_f32..11_000.0_f32,  // freq: 50 Hz – 11 kHz
        -80.0_f32..-5.0_f32,     // amplitude: realistic dB range
    )
        .prop_map(|(time, freq, amplitude)| Peak::new(time, freq, amplitude))
}

/// Generates a non-empty sorted-by-time Vec of peaks.
fn arb_peaks(min: usize, max: usize) -> impl Strategy<Value = Vec<Peak>> {
    proptest::collection::vec(arb_peak(), min..=max).prop_map(|mut v| {
        v.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
        v
    })
}

// ---------------------------------------------------------------------------
// Property 1: Fingerprinting is deterministic across runs
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn prop_fingerprinting_is_deterministic(peaks in arb_peaks(0, 50)) {
        let config = HashConfig::default();
        let hashes_a = generate_hashes(&peaks, &config);
        let hashes_b = generate_hashes(&peaks, &config);

        prop_assert_eq!(hashes_a.len(), hashes_b.len());
        for (a, b) in hashes_a.iter().zip(hashes_b.iter()) {
            prop_assert_eq!(a.hash, b.hash);
            // Use abs diff for f32 comparison to avoid precision issues.
            prop_assert!((a.anchor_time - b.anchor_time).abs() < f32::EPSILON);
        }
    }
}

// ---------------------------------------------------------------------------
// Property 2: Query always returns None for an empty index
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn prop_empty_index_always_returns_none(peaks in arb_peaks(1, 50)) {
        let config = HashConfig::default();
        let hashes = generate_hashes(&peaks, &config);

        let index = Index::default();
        let result = index.query(&hashes);

        prop_assert!(result.is_none(), "empty index should never return a match");
    }
}

// ---------------------------------------------------------------------------
// Property 3: Score is monotonically higher for longer matching clips
// ---------------------------------------------------------------------------
//
// Strategy: index a track with N fingerprints; query first with K < N hashes
// (short clip), then with all N hashes (full clip). The full-clip score must
// be >= the short-clip score.

proptest! {
    #[test]
    fn prop_longer_clip_score_geq_shorter_clip(
        peaks in arb_peaks(4, 60)
    ) {
        let config = HashConfig::default();
        let all_fps = generate_hashes(&peaks, &config);

        // Skip if we didn't produce enough fingerprints to split.
        prop_assume!(!all_fps.is_empty());

        let mid = all_fps.len() / 2;
        prop_assume!(mid > 0);

        let short_fps = &all_fps[..mid];
        let full_fps = &all_fps[..];

        let mut index = Index::default();
        index.insert("track_a", &all_fps);

        let short_result = index.query(short_fps);
        let full_result = index.query(full_fps);

        // Both should match the same track.
        if let (Some(short), Some(full)) = (&short_result, &full_result) {
            prop_assert_eq!(&short.track_id, &full.track_id);
            prop_assert!(
                full.score >= short.score,
                "full-clip score ({}) should be >= short-clip score ({})",
                full.score,
                short.score,
            );
        }
        // If the short query matched nothing, the full query must match.
        if short_result.is_none() {
            prop_assert!(full_result.is_some(), "full clip must match when short clip does not");
        }
    }
}
