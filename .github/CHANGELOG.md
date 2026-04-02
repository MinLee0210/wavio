# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-03-22

### Added

- **DSP Pipeline**
  - WAV file loading with stereo→mono downmix and f32 normalization (`dsp::audio`)
  - Sliding-window FFT with Hann windowing and dB power spectrogram (`dsp::spectrogram`)
  - 2D local-max peak detection with configurable neighborhood and threshold (`dsp::peaks`)
  - `AudioSource` trait for pluggable audio backends (`io::IOReader`)

- **Fingerprinting**
  - Combinatorial hashing of peak pairs into `u64` fingerprints (`hash`)
  - Configurable fan value, time window, and frequency bins via `HashConfig`
  - Deterministic output regardless of input order

- **Indexing & Querying**
  - In-memory `Index` with `HashMap<u64, Vec<(TrackId, f32)>>` storage
  - Time-offset histogram-based query matching with configurable bin size
  - `QueryResult` with track name, score, and estimated time offset

- **Persistence** (`persist` feature)
  - On-disk `PersistentIndex` backed by `sled` embedded database
  - Convenience methods `Index::save_to_disk()` and `Index::load_from_disk()`
  - Four-tree sled layout: hashes, tracks_by_name, tracks_by_id, metadata

- **Parallelism** (`parallel` feature)
  - `extract_peaks_parallel()` — frame-level parallel peak detection via rayon
  - `generate_hashes_parallel()` — anchor-level parallel hash generation
  - `Index::insert_batch_parallel()` — parallel batch indexing

- **CLI** (`wavio-cli` binary)
  - `wavio index` — batch-index a directory of WAV files
  - `wavio query` — identify a clip against the database
  - `wavio info` — print track/hash counts
  - Progress bar via `indicatif`, verbose mode with `--verbose`

- **Python Bindings** (`python` feature)
  - `PyFingerprinter` class with GIL-releasing `fingerprint_file()` method
  - `PyIndex` class with `insert()`, `query()`, `load()`, `save()` methods
  - `maturin`-based build system

- **Documentation & Tooling**
  - `ARCHITECTURE.md` with pipeline diagram and design rationale
  - `CONTRIBUTING.md` with code style and PR checklist
  - `BENCHMARKS.md` with criterion baseline numbers
  - GitHub Actions CI (fmt, clippy, test, audit, MSRV check)
  - Comprehensive doc comments on all public items with `# Examples`
  - `#[non_exhaustive]` on all extensible public structs/enums

### Security

- `#![forbid(unsafe_code)]` — no unsafe blocks in the crate
- `cargo audit` integrated in CI

[0.1.0]: https://github.com/MinLee0210/wavio/releases/tag/v0.1.0
