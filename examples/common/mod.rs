//! Shared helpers for the runnable examples in this directory.
//!
//! Not part of the published crate -- just synthetic-audio generation so
//! each example is fully self-contained (no external WAV files, no network,
//! no `data/` fixtures) and reproducible for anyone running
//! `cargo run --example <name>`.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

/// Synthesizes a mono `22,050 Hz` signal built from several harmonically
/// related tones with slow amplitude/vibrato modulation, so it produces a
/// rich, non-trivial constellation of spectral peaks (a pure single sine
/// wave produces almost none -- see `dsp::peaks`' tied-peak-suppression
/// behavior).
///
/// `seed` shifts the harmonic phases so different calls produce genuinely
/// different-sounding (and differently-fingerprinted) "tracks".
#[allow(dead_code)]
pub fn synth_track(duration_secs: f32, sample_rate: u32, seed: f32) -> Vec<f32> {
    let n = (duration_secs * sample_rate as f32) as usize;
    let freqs = [220.0, 330.0, 440.0, 550.0, 660.0, 770.0, 880.0];

    (0..n)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            let mut sample = 0.0_f32;
            for (k, &f) in freqs.iter().enumerate() {
                let phase = (k as f32 + seed) * 0.3;
                let amp = 0.12 * (1.0 + 0.5 * (t * 0.5 + phase).sin());
                let vibrato = 1.0 + 0.02 * (t * 0.9 + phase).sin();
                sample += amp * (std::f32::consts::TAU * f * vibrato * t).sin();
            }
            sample.clamp(-1.0, 1.0)
        })
        .collect()
}

/// Generates a few seconds of low-amplitude white noise -- stands in for
/// "unrelated audio" (e.g. talk radio, ambient chatter) between known tracks
/// in the streaming-monitor example.
#[allow(dead_code)]
pub fn synth_noise(duration_secs: f32, sample_rate: u32, seed: u64) -> Vec<f32> {
    // A tiny xorshift PRNG -- no need to pull in `rand` for example code.
    let mut state = seed.max(1);
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    let n = (duration_secs * sample_rate as f32) as usize;
    (0..n)
        .map(|_| {
            let r = (next() % 2000) as f32 / 1000.0 - 1.0; // roughly [-1.0, 1.0]
            r * 0.05
        })
        .collect()
}
