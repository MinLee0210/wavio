// ─── Lint Configuration ──────────────────────────────────────────────
//
// Level 1: deny all basic clippy warnings — catches common mistakes
#![deny(clippy::all)]
// Level 2: warn on pedantic lints — stricter, opinionated style checks
#![warn(clippy::pedantic)]
// Level 3: warn on missing docs — enforces documentation on public items
#![warn(missing_docs)]
// Level 4: deny unsafe code — no `unsafe` blocks allowed in this crate
#![forbid(unsafe_code)]

//! # wavio
//!
//! **Peak-based audio fingerprinting. Zero overhead. Written in Rust.**
//!
//! `wavio` is a high-throughput acoustic fingerprinting library built for
//! DSP engineers who need fast, deterministic audio identification without
//! the weight of an ML stack. No embeddings, no models, no runtime — just
//! spectral peaks, combinatorial hashing, and raw speed.
//!
//! ## Pipeline Overview
//!
//! ```text
//! WAV file
//!   → load & normalize (mono f32 @ 22,050 Hz)
//!     → sliding-window FFT (Hann, 2048-sample frames)
//!       → dB power spectrogram
//!         → 2D local-max peak detection
//!           → combinatorial hashing (peak pairs → u64)
//!             → index (HashMap / sled) → query → match
//! ```
//!
//! ## Quick Example
//!
//! ```rust,no_run
//! use wavio::dsp::audio::load_wav;
//! use wavio::dsp::spectrogram::{compute_spectrogram, SpectrogramConfig};
//! use wavio::dsp::peaks::{extract_peaks, PeakExtractorConfig};
//! use wavio::hash::{generate_hashes, HashConfig};
//! use wavio::index::Index;
//!
//! // 1. Load a WAV file
//! let audio = load_wav("track.wav").expect("failed to load WAV");
//!
//! // 2. Compute spectrogram
//! let spec = compute_spectrogram(&audio.samples, &SpectrogramConfig::default())
//!     .expect("spectrogram failed");
//!
//! // 3. Extract constellation peaks
//! let peaks = extract_peaks(&spec, &PeakExtractorConfig::default());
//!
//! // 4. Generate fingerprint hashes
//! let hashes = generate_hashes(&peaks, &HashConfig::default());
//!
//! // 5. Index and query
//! let mut index = Index::default();
//! index.insert("my_track", &hashes);
//!
//! let result = index.query(&hashes);
//! assert_eq!(result.unwrap().track_id, "my_track");
//! ```
//!
//! ## Feature Flags
//!
//! | Flag | Description | Dependencies |
//! |------|-------------|--------------|
//! | `parallel` | Rayon-based parallel fingerprinting and peak extraction | `rayon`, `dashmap` |
//! | `persist` | On-disk index persistence via `sled` | `sled`, `serde`, `bincode` |
//! | `python` | Python bindings via PyO3 | `pyo3` |
//!
//! Enable features in your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! wavio = { version = "0.1", features = ["persist", "parallel"] }
//! ```

pub mod dsp;
pub mod error;
pub mod hash;
pub mod index;
pub mod io;
pub mod utils;

#[cfg(feature = "persist")]
pub mod persist;

/// Python bindings via PyO3.
#[cfg(feature = "python")]
pub mod python;
