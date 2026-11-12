//! Criterion benchmarks for the wavio DSP pipeline and fingerprint index.
//!
//! Run with:
//!   cargo bench                                   # serial
//!   cargo bench --features parallel               # with rayon parallelism
//!   cargo bench --features parallel,persist       # also the on-disk index
//!
//! Workloads are sized like real libraries (see `BENCHMARKS.md`):
//! - a ~3-minute track yields on the order of 50,000 hashes;
//! - a query is a ~10-second clip (~2,800 hashes) of which ~30% are noise
//!   hashes that match nothing;
//! - hash values are drawn from a 2^24 space, so hash collisions between
//!   tracks (the real cost of voting) actually occur.
//!
//! Groups:
//! - `dsp/*`:    spectrogram, peak extraction, and the full pipeline on 3 min
//!   of synthetic multi-tone "music" with noise.
//! - `index/*`:  in-memory insert and query on a 200-track / 10M-entry index.
//! - `persist/*`: on-disk insert and query on a 50-track / 2.5M-entry index.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::float_cmp,
    clippy::similar_names,
    clippy::too_many_lines
)]

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use wavio::dsp::Fingerprinter;
use wavio::dsp::peaks::{PeakExtractorConfig, extract_peaks};
use wavio::dsp::spectrogram::{SpectrogramConfig, compute_spectrogram};
use wavio::hash::Fingerprint;
use wavio::index::Index;

const SAMPLE_RATE: u32 = 22_050;
const HASHES_PER_TRACK: usize = 50_000;
const HASH_SPACE: u64 = 1 << 24;
const TRACK_SECS: f32 = 180.0;
const CLIP_SECS: f32 = 10.0;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Tiny deterministic xorshift PRNG (no `rand` dependency).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    #[allow(clippy::cast_precision_loss)]
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1_u64 << 24) as f32
    }
}

/// Synthetic "music": a drifting set of harmonic tones plus broadband noise,
/// so the spectrogram has a realistic number of local maxima.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn synthetic_music(duration_secs: f32) -> Vec<f32> {
    let n = (duration_secs * SAMPLE_RATE as f32) as usize;
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    (0..n)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let note = (t * 2.0).floor();
            let base = 180.0 + (note * 37.0) % 400.0;
            let tone: f32 = (1..=5)
                .map(|h| (std::f32::consts::TAU * base * h as f32 * t).sin() / h as f32)
                .sum();
            (0.25 * tone + 0.05 * (rng.unit() - 0.5)).clamp(-1.0, 1.0)
        })
        .collect()
}

/// A synthetic track's fingerprints: random hashes over `TRACK_SECS`.
#[allow(clippy::cast_precision_loss)]
fn synthetic_track(seed: u64) -> Vec<Fingerprint> {
    let mut rng = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1);
    let mut fps: Vec<Fingerprint> = (0..HASHES_PER_TRACK)
        .map(|_| Fingerprint::new(rng.next() % HASH_SPACE, rng.unit() * TRACK_SECS))
        .collect();
    fps.sort_by(|a, b| a.anchor_time.total_cmp(&b.anchor_time));
    fps
}

/// A ~10 s query clip cut from `track` at `start_secs`, rebased to t=0, with
/// 30% of its hashes replaced by noise (as if the clip had been degraded).
fn query_from(track: &[Fingerprint], start_secs: f32, seed: u64) -> Vec<Fingerprint> {
    let mut rng = Rng(seed | 1);
    track
        .iter()
        .filter(|f| f.anchor_time >= start_secs && f.anchor_time < start_secs + CLIP_SECS)
        .map(|f| {
            let hash = if rng.unit() < 0.3 {
                rng.next() % HASH_SPACE
            } else {
                f.hash
            };
            Fingerprint::new(hash, f.anchor_time - start_secs)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// dsp/*
// ---------------------------------------------------------------------------

fn bench_dsp(c: &mut Criterion) {
    let samples = synthetic_music(TRACK_SECS);
    let fingerprinter = Fingerprinter::default();
    let spec = compute_spectrogram(&samples, &SpectrogramConfig::default()).unwrap();
    let peak_config = PeakExtractorConfig::default();

    let mut group = c.benchmark_group("dsp");
    group.sample_size(10);

    // Resampling a 3-minute 44.1 kHz / 48 kHz mono signal to 22.05 kHz.
    let src_44k = vec![0.1_f32; 44_100 * 180];
    let src_48k = vec![0.1_f32; 48_000 * 180];
    group.bench_function("resample_sinc_44k_3min", |b| {
        b.iter(|| criterion::black_box(wavio::dsp::audio::resample(&src_44k, 44_100, 22_050)));
    });
    group.bench_function("resample_sinc_48k_3min", |b| {
        b.iter(|| criterion::black_box(wavio::dsp::audio::resample(&src_48k, 48_000, 22_050)));
    });
    group.bench_function("resample_linear_44k_3min", |b| {
        b.iter(|| {
            criterion::black_box(wavio::dsp::audio::resample_linear(&src_44k, 44_100, 22_050))
        });
    });

    group.bench_function("spectrogram_3min", |b| {
        b.iter(|| {
            criterion::black_box(
                compute_spectrogram(&samples, &SpectrogramConfig::default()).unwrap(),
            )
        });
    });

    group.bench_function("extract_peaks_3min", |b| {
        b.iter(|| criterion::black_box(extract_peaks(&spec, &peak_config)));
    });

    #[cfg(feature = "parallel")]
    group.bench_function("extract_peaks_parallel_3min", |b| {
        b.iter(|| {
            criterion::black_box(wavio::dsp::peaks::extract_peaks_parallel(
                &spec,
                &peak_config,
            ))
        });
    });

    group.bench_function("fingerprint_full_3min", |b| {
        b.iter(|| criterion::black_box(fingerprinter.fingerprint(&samples).unwrap()));
    });

    group.finish();

    let n_peaks = extract_peaks(&spec, &peak_config).len();
    let n_hashes = fingerprinter.fingerprint(&samples).unwrap().len();
    eprintln!("[dsp] 3-min synthetic track: {n_peaks} peaks, {n_hashes} hashes");
}

// ---------------------------------------------------------------------------
// index/*
// ---------------------------------------------------------------------------

fn bench_index(c: &mut Criterion) {
    const N_TRACKS: u64 = 200;
    let tracks: Vec<Vec<Fingerprint>> = (0..N_TRACKS).map(synthetic_track).collect();

    let mut group = c.benchmark_group("index");
    group.sample_size(10);

    group.bench_function("insert_10_tracks_x_50k", |b| {
        b.iter_batched(
            Index::default,
            |mut index| {
                for (i, fps) in tracks.iter().take(10).enumerate() {
                    index.insert(&format!("track_{i}"), fps);
                }
                criterion::black_box(index)
            },
            BatchSize::LargeInput,
        );
    });

    let mut index = Index::default();
    for (i, fps) in tracks.iter().enumerate() {
        index.insert(&format!("track_{i}"), fps);
    }
    eprintln!(
        "[index] {} tracks, {} entries",
        index.track_count(),
        index.hash_count()
    );

    let queries: Vec<Vec<Fingerprint>> = (0..20_u64)
        .map(|q| {
            #[allow(clippy::cast_precision_loss)]
            let start = 5.0 + q as f32 * 7.0;
            query_from(&tracks[(q * 9 % N_TRACKS) as usize], start, q + 1)
        })
        .collect();
    eprintln!("[index] query size: ~{} hashes", queries[0].len());

    // Sanity: the benchmark must be measuring correct matches.
    for (q, query) in queries.iter().enumerate() {
        let r = index.query(query).expect("query should match");
        assert_eq!(r.track_id, format!("track_{}", q as u64 * 9 % N_TRACKS));
    }

    group.bench_function("query_10s_clip_in_200_tracks", |b| {
        b.iter(|| {
            for q in &queries {
                criterion::black_box(index.query(q));
            }
        });
    });
    group.finish();
}

// ---------------------------------------------------------------------------
// persist/*
// ---------------------------------------------------------------------------

#[cfg(feature = "persist")]
fn bench_persist(c: &mut Criterion) {
    use wavio::persist::PersistentIndex;

    const N_TRACKS: u64 = 50;
    let tracks: Vec<Vec<Fingerprint>> = (0..N_TRACKS).map(synthetic_track).collect();
    let dir = tempfile::tempdir().unwrap();

    let mut group = c.benchmark_group("persist");
    group.sample_size(10);

    group.bench_function("insert_1_track_x_50k", |b| {
        let mut db = PersistentIndex::open(dir.path().join("insert.redb")).unwrap();
        let mut n = 0_u64;
        b.iter(|| {
            db.insert(&format!("t{n}"), &tracks[(n % N_TRACKS) as usize])
                .unwrap();
            n += 1;
        });
    });

    let mut db = PersistentIndex::open(dir.path().join("query.redb")).unwrap();
    let batch: Vec<(String, Vec<Fingerprint>)> = tracks
        .iter()
        .enumerate()
        .map(|(i, f)| (format!("track_{i}"), f.clone()))
        .collect();
    db.insert_many(&batch).unwrap();
    eprintln!(
        "[persist] {} tracks, {} entries",
        db.track_count(),
        db.hash_count()
    );

    let queries: Vec<Vec<Fingerprint>> = (0..10_u64)
        .map(|q| {
            #[allow(clippy::cast_precision_loss)]
            let start = 5.0 + q as f32 * 11.0;
            query_from(&tracks[(q * 4 % N_TRACKS) as usize], start, q + 1)
        })
        .collect();
    for (q, query) in queries.iter().enumerate() {
        assert_eq!(
            db.query(query).expect("query should match").track_id,
            format!("track_{}", q as u64 * 4 % N_TRACKS)
        );
    }

    group.bench_function("query_10s_clip_in_50_tracks", |b| {
        b.iter(|| {
            for q in &queries {
                criterion::black_box(db.query(q));
            }
        });
    });
    group.finish();
}

#[cfg(not(feature = "persist"))]
fn bench_persist(_: &mut Criterion) {}

criterion_group!(benches, bench_dsp, bench_index, bench_persist);
criterion_main!(benches);
