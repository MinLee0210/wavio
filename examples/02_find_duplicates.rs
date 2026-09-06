//! Duplicate / near-duplicate detection across a media library.
//!
//! Fingerprints every file in a "library" (here: synthetic tracks, one of
//! which is a re-encoded/lower-volume copy of another), then cross-queries
//! each track against everyone else's fingerprints to flag likely
//! duplicates by confidence.
//!
//! Run with: `cargo run --example 02_find_duplicates`

#[path = "common/mod.rs"]
mod common;

use wavio::dsp::Fingerprinter;
use wavio::index::Index;

const SAMPLE_RATE: u32 = 22_050;
const DUPLICATE_CONFIDENCE_THRESHOLD: f32 = 0.5;

fn main() {
    let fingerprinter = Fingerprinter::default();

    let original = common::synth_track(15.0, SAMPLE_RATE, 1.0);
    // A "duplicate": same content, just quieter -- e.g. a re-encoded or
    // volume-normalized copy someone re-uploaded under a different name.
    let quieter_copy: Vec<f32> = original.iter().map(|s| s * 0.6).collect();

    let library = [
        ("track_alpha", original),
        ("track_alpha_reupload", quieter_copy),
        ("track_beta", common::synth_track(15.0, SAMPLE_RATE, 4.0)),
        ("track_gamma", common::synth_track(15.0, SAMPLE_RATE, 7.0)),
    ];

    // Fingerprint everything up front.
    let fingerprints: Vec<(&str, Vec<wavio::hash::Fingerprint>)> = library
        .iter()
        .map(|(name, samples)| {
            (
                *name,
                fingerprinter
                    .fingerprint(samples)
                    .expect("fingerprint failed"),
            )
        })
        .collect();

    println!("Scanning {} tracks for duplicates...\n", library.len());

    // For each track, build an index of *everyone else* and query against
    // it -- a real match above the confidence threshold is a likely
    // duplicate (rather than just sharing a few incidental hash collisions).
    for (i, (name, fps)) in fingerprints.iter().enumerate() {
        let mut others = Index::default();
        for (j, (other_name, other_fps)) in fingerprints.iter().enumerate() {
            if i != j {
                others.insert(other_name, other_fps);
            }
        }

        match others.query_with_min_confidence(fps, DUPLICATE_CONFIDENCE_THRESHOLD) {
            Some(result) => println!(
                "'{name}' looks like a duplicate of '{}' (confidence {:.1}%, offset {:.2}s)",
                result.track_id,
                result.confidence * 100.0,
                result.offset_secs
            ),
            None => println!("'{name}': no duplicate found"),
        }
    }
}
