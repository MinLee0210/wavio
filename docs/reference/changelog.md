# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.1.0] - 2026-03-22

### Added

- **DSP Pipeline**
    - WAV file loading with stereo→mono downmix and f32 normalization
    - Sliding-window FFT with Hann windowing and dB power spectrogram
    - 2D local-max peak detection with configurable neighborhood and threshold
    - `AudioSource` trait for pluggable audio backends

- **Fingerprinting**
    - Combinatorial hashing of peak pairs into `u64` fingerprints
    - Configurable fan value, time window, and frequency bins
    - Deterministic output regardless of input order

- **Indexing & Querying**
    - In-memory `Index` with `HashMap<u64, Vec<(TrackId, f32)>>` storage
    - Time-offset histogram-based query matching
    - `QueryResult` with track name, score, and estimated offset

- **Persistence** (`persist` feature)
    - On-disk `PersistentIndex` backed by sled
    - `Index::save_to_disk()` and `Index::load_from_disk()`

- **Parallelism** (`parallel` feature)
    - `extract_peaks_parallel()` via rayon
    - `generate_hashes_parallel()` via rayon
    - `Index::insert_batch_parallel()`

- **CLI** (`wavio-cli` binary)
    - `index`, `query`, `info` subcommands
    - Progress bar and verbose mode

- **Python Bindings** (`python` feature)
    - `PyFingerprinter` and `PyIndex` classes via PyO3
    - GIL-releasing fingerprinting

- **Documentation & Tooling**
    - MkDocs Material documentation site
    - `ARCHITECTURE.md`, `CONTRIBUTING.md`, `BENCHMARKS.md`
    - GitHub Actions CI
    - `#[non_exhaustive]` on all extensible structs/enums

### Security

- `#![forbid(unsafe_code)]`
- `cargo audit` integrated in CI

[0.1.0]: https://github.com/MinLee0210/wavio/releases/tag/v0.1.0
