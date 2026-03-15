//! Criterion benchmarks for the wavio DSP pipeline and fingerprint index.
//!
//! Run with:
//!   cargo bench                         # serial
//!   cargo bench --features parallel     # with rayon parallelism
//!
//! Three benchmark groups:
//! - `fingerprint_single`: full DSP pipeline on a synthetic ~3-min audio clip.
//! - `index_1k`:           insert 1,000 synthetic tracks into an in-memory index.
//! - `query_1k`:           run 1,000 queries against a pre-built 1k-track index.

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use wavio::dsp::peaks::{PeakExtractorConfig, extract_peaks};
use wavio::dsp::spectrogram::{SpectrogramConfig, compute_spectrogram};
use wavio::hash::{Fingerprint, HashConfig, generate_hashes};
use wavio::index::Index;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Generates a synthetic mono PCM signal (silence with a single sine tone)
/// at 22,050 Hz for the requested duration in seconds.
#[allow(clippy::cast_precision_loss)]
fn synthetic_samples(duration_secs: f32, sample_rate: u32) -> Vec<f32> {
    let n = (duration_secs * sample_rate as f32) as usize;
    // Simple 440 Hz sine to avoid all-silence edge cases in peak detection.
    (0..n)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            (std::f32::consts::TAU * 440.0 * t).sin() * 0.5
        })
        .collect()
}

/// Runs the full DSP pipeline on `samples` and returns fingerprints.
fn fingerprint(samples: &[f32]) -> Vec<Fingerprint> {
    let spec_cfg = SpectrogramConfig::default();
    let peak_cfg = PeakExtractorConfig::default();
    let hash_cfg = HashConfig::default();

    let spec = compute_spectrogram(samples, &spec_cfg).expect("spectrogram failed");
    let peaks = extract_peaks(&spec, &peak_cfg);
    generate_hashes(&peaks, &hash_cfg)
}

/// Builds a batch of 1,000 synthetic fingerprint sets (one per "track").
/// Each track uses a distinct hash range to avoid cross-track collisions.
#[allow(clippy::cast_precision_loss)]
fn build_synthetic_batch(n_tracks: usize) -> Vec<(String, Vec<Fingerprint>)> {
    (0..n_tracks)
        .map(|track_idx| {
            // 20 unique fingerprints per track, no hash overlap between tracks.
            let fps: Vec<Fingerprint> = (0..20_u64)
                .map(|i| Fingerprint {
                    hash: (track_idx as u64) * 1_000_000 + i,
                    anchor_time: i as f32 * 0.05,
                })
                .collect();
            (format!("track_{track_idx}"), fps)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// bench_fingerprint_single — full pipeline on a ~3-min WAV
// ---------------------------------------------------------------------------

fn bench_fingerprint_single(c: &mut Criterion) {
    let sample_rate = 22_050_u32;
    // 180 seconds ≈ 3 minutes at 22,050 Hz → ~3.97M samples.
    let samples = synthetic_samples(180.0, sample_rate);

    c.bench_function("fingerprint_single_3min", |b| {
        b.iter(|| {
            let fps = fingerprint(&samples);
            criterion::black_box(fps);
        });
    });
}

// ---------------------------------------------------------------------------
// bench_index_1k — index 1,000 synthetic tracks
// ---------------------------------------------------------------------------

fn bench_index_1k(c: &mut Criterion) {
    let batch = build_synthetic_batch(1_000);

    c.bench_function("index_insert_1k_tracks", |b| {
        b.iter_batched(
            || batch.clone(),
            |b| {
                let mut index = Index::default();
                for (name, fps) in &b {
                    index.insert(name, fps);
                }
                criterion::black_box(index);
            },
            BatchSize::LargeInput,
        );
    });
}

// ---------------------------------------------------------------------------
// bench_query_1k — 1,000 queries against a pre-built 1k-track index
// ---------------------------------------------------------------------------

fn bench_query_1k(c: &mut Criterion) {
    // Build the index once outside the timed loop.
    let batch = build_synthetic_batch(1_000);
    let mut index = Index::default();
    for (name, fps) in &batch {
        index.insert(name, fps);
    }

    // Build 1,000 query fingerprint sets — each targeting a different track.
    #[allow(clippy::cast_precision_loss)]
    let queries: Vec<Vec<Fingerprint>> = (0..1_000_usize)
        .map(|track_idx| {
            (0..20_u64)
                .map(|i| Fingerprint {
                    hash: (track_idx as u64) * 1_000_000 + i,
                    anchor_time: i as f32 * 0.05,
                })
                .collect()
        })
        .collect();

    c.bench_function("index_query_1k", |b| {
        b.iter(|| {
            for q in &queries {
                criterion::black_box(index.query(q));
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Criterion boilerplate
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_fingerprint_single,
    bench_index_1k,
    bench_query_1k,
);
criterion_main!(benches);
