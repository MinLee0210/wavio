//! Song identification -- the core "Shazam" use case.
//!
//! Builds a tiny in-memory library of tracks, fingerprints a short clip cut
//! from the middle of one of them, and identifies which track it came from
//! (plus *where* in the track it came from).
//!
//! Run with: `cargo run --example 01_identify_clip`

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

#[path = "common/mod.rs"]
mod common;

use wavio::dsp::Fingerprinter;
use wavio::index::Index;

const SAMPLE_RATE: u32 = 22_050;

fn main() {
    let fingerprinter = Fingerprinter::default();

    // 1. Build a library of three distinct tracks and index them.
    let library = [
        (
            "bohemian_rhapsody",
            common::synth_track(20.0, SAMPLE_RATE, 0.0),
        ),
        (
            "stairway_to_heaven",
            common::synth_track(20.0, SAMPLE_RATE, 3.0),
        ),
        (
            "hotel_california",
            common::synth_track(20.0, SAMPLE_RATE, 6.0),
        ),
    ];

    let mut index = Index::default();
    for (name, samples) in &library {
        let fingerprints = fingerprinter
            .fingerprint(samples)
            .expect("fingerprint failed");
        println!("Indexed '{name}': {} fingerprints", fingerprints.len());
        index.insert(name, &fingerprints);
    }
    println!(
        "\nLibrary ready: {} tracks, {} total fingerprints.\n",
        index.track_count(),
        index.hash_count()
    );

    // 2. Take a short, unlabeled clip starting 8.5s into "stairway_to_heaven"
    //    -- simulating a snippet someone recorded and wants identified.
    let (_, full_track) = &library[1];
    let clip_start_secs = 8.5;
    let clip_start_sample = (clip_start_secs * SAMPLE_RATE as f32) as usize;
    let clip_duration_secs = 4.0;
    let clip_len = (clip_duration_secs * SAMPLE_RATE as f32) as usize;
    let clip = &full_track[clip_start_sample..clip_start_sample + clip_len];

    // 3. Identify it.
    let clip_fingerprints = fingerprinter.fingerprint(clip).expect("fingerprint failed");
    println!(
        "Query clip: {:.1}s long, {} fingerprints extracted.\n",
        clip_duration_secs,
        clip_fingerprints.len()
    );

    match index.query(&clip_fingerprints) {
        Some(result) => {
            println!("Match found!");
            println!("  Track:      {}", result.track_id);
            println!("  Score:      {}", result.score);
            println!("  Confidence: {:.1}%", result.confidence * 100.0);
            println!(
                "  Offset:     {:.2}s (clip actually started at {:.2}s)",
                result.offset_secs, clip_start_secs
            );
        }
        None => println!("No match found."),
    }

    // 4. Ranked alternatives, in case you want to show runner-up candidates
    //    or the match was ambiguous.
    println!("\nTop-3 ranked candidates:");
    for (rank, result) in index.query_topn(&clip_fingerprints, 3).iter().enumerate() {
        println!(
            "  #{}: {} (score={}, confidence={:.1}%)",
            rank + 1,
            result.track_id,
            result.score,
            result.confidence * 100.0
        );
    }
}
