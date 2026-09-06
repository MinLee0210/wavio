//! Broadcast / stream monitoring using `StreamingFingerprinter`.
//!
//! Simulates a live audio feed (radio broadcast, video's audio track, a
//! Twitch/YouTube stream) arriving in small chunks. A small library of
//! "known tracks" is indexed up front; as the stream is fed in piece by
//! piece, each newly-finalized batch of fingerprints is queried against the
//! index to detect *when* and *where* a known track plays -- without ever
//! holding the whole broadcast in memory.
//!
//! Run with: `cargo run --example 03_stream_monitor`

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

#[path = "common/mod.rs"]
mod common;

use wavio::dsp::Fingerprinter;
use wavio::dsp::streaming::{StreamConfig, StreamingFingerprinter};
use wavio::index::Index;

const SAMPLE_RATE: u32 = 22_050;
const DETECTION_CONFIDENCE_THRESHOLD: f32 = 0.3;

fn main() {
    let fingerprinter = Fingerprinter::default();

    // 1. Index a small library of tracks we want to detect in the stream.
    let known_tracks = [
        (
            "station_jingle",
            common::synth_track(10.0, SAMPLE_RATE, 2.0),
        ),
        ("ad_track", common::synth_track(8.0, SAMPLE_RATE, 5.0)),
    ];

    let mut index = Index::default();
    for (name, samples) in &known_tracks {
        let fps = fingerprinter
            .fingerprint(samples)
            .expect("fingerprint failed");
        index.insert(name, &fps);
    }
    println!("Indexed {} known tracks.\n", index.track_count());

    // 2. Build a synthetic "broadcast": ambient noise, then the jingle,
    //    more noise, then the ad, then more noise -- like a real stream
    //    where known content is interspersed with unrelated audio.
    let mut broadcast = Vec::new();
    broadcast.extend(common::synth_noise(3.0, SAMPLE_RATE, 1));
    let jingle_starts_at = broadcast.len() as f32 / SAMPLE_RATE as f32;
    broadcast.extend(known_tracks[0].1.clone());
    broadcast.extend(common::synth_noise(3.0, SAMPLE_RATE, 2));
    let ad_starts_at = broadcast.len() as f32 / SAMPLE_RATE as f32;
    broadcast.extend(known_tracks[1].1.clone());
    broadcast.extend(common::synth_noise(3.0, SAMPLE_RATE, 3));

    println!(
        "Simulated broadcast: {:.1}s total (jingle at ~{:.1}s, ad at ~{:.1}s).\n",
        broadcast.len() as f32 / SAMPLE_RATE as f32,
        jingle_starts_at,
        ad_starts_at
    );

    // 3. Feed it through the stream in 1-second chunks, as if it were
    //    arriving live, and check each newly-finalized batch against the
    //    known-track index.
    let stream_config = StreamConfig::new(6.0, 1.0);
    let mut streamer = StreamingFingerprinter::new(Fingerprinter::default(), &stream_config)
        .expect("failed to build streaming fingerprinter");

    let chunk_size = SAMPLE_RATE as usize; // 1 second per "live" chunk
    for chunk in broadcast.chunks(chunk_size) {
        let batch = streamer.push(chunk).expect("push failed");
        report_detections(&index, &batch);
    }
    let tail = streamer.finish().expect("finish failed");
    report_detections(&index, &tail);
}

fn report_detections(index: &Index, batch: &[wavio::hash::Fingerprint]) {
    if batch.is_empty() {
        return;
    }

    let start = batch
        .iter()
        .map(|fp| fp.anchor_time)
        .fold(f32::MAX, f32::min);
    let end = batch
        .iter()
        .map(|fp| fp.anchor_time)
        .fold(f32::MIN, f32::max);

    match index.query_with_min_confidence(batch, DETECTION_CONFIDENCE_THRESHOLD) {
        Some(result) => println!(
            "[{start:5.1}s - {end:5.1}s] DETECTED '{}' (confidence {:.0}%)",
            result.track_id,
            result.confidence * 100.0
        ),
        None => println!("[{start:5.1}s - {end:5.1}s] (no known track)"),
    }
}
