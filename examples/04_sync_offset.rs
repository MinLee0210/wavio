//! Sync / time-alignment detection.
//!
//! `QueryResult::offset_secs` tells you *where inside the original track* a
//! query clip came from. This example indexes a full track, cuts a short
//! clip from a known position, and shows that the recovered offset matches
//! reality -- useful for aligning a sample to its source, or finding where
//! a snippet sits inside a longer recording.
//!
//! Run with: `cargo run --example 04_sync_offset`

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

    let full_track = common::synth_track(30.0, SAMPLE_RATE, 1.5);
    let fps = fingerprinter
        .fingerprint(&full_track)
        .expect("fingerprint failed");

    let mut index = Index::default();
    index.insert("full_recording", &fps);

    // Try several clips cut from different, known positions in the track.
    let test_positions_secs = [2.3, 11.7, 19.0, 25.4];

    println!("Indexed a 30.0s recording. Testing clip alignment:\n");

    for &clip_start_secs in &test_positions_secs {
        let clip_len_secs = 3.0;
        let start_sample = (clip_start_secs * SAMPLE_RATE as f32) as usize;
        let len = (clip_len_secs * SAMPLE_RATE as f32) as usize;
        let clip = &full_track[start_sample..start_sample + len];

        let clip_fps = fingerprinter.fingerprint(clip).expect("fingerprint failed");

        match index.query(&clip_fps) {
            Some(result) => {
                let error = (result.offset_secs - clip_start_secs).abs();
                println!(
                    "  Clip actually started at {clip_start_secs:5.2}s -> recovered offset {:5.2}s (error {error:.2}s)",
                    result.offset_secs
                );
            }
            None => println!("  Clip at {clip_start_secs:5.2}s: no match found"),
        }
    }
}
