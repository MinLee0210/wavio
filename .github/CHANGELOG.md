# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Pitch-shift / time-stretch-robust hashing** (`wavio::triplet`) — a new opt-in hashing mode that encodes ratios within a triplet of peaks instead of absolute frequency/time values, making it largely invariant to uniform pitch shift and time stretch (see `ARCHITECTURE.md` §3b for the invariance rationale and its limitations). Enable via `Fingerprinter::with_triplet_hashing(TripletHashConfig::default())` or the CLI's new `--robust` flag. Produces plain `Fingerprint`s, so `Index`/`PersistentIndex` require no changes.
- **Streaming fingerprinting** (`wavio::dsp::streaming::StreamingFingerprinter`) — fingerprints audio in overlapping fixed-size blocks so callers don't need the whole file in memory (e.g. live/incremental sources). Block boundaries are aligned to the spectrogram's hop-size grid and trimmed on both edges for full peak-neighborhood and fan-out context, so streamed output matches a single batch `Fingerprinter::fingerprint` call almost exactly.
- `Index::query_topn` / `PersistentIndex::query_topn` — returns up to `n` ranked matches (best first) instead of only the single best track.
- `Index::query_with_min_confidence` / `PersistentIndex::query_with_min_confidence` — rejects a match whose `confidence` is below a threshold.
- `Index::contains_track` — checks whether a track name is already indexed.
- CLI: `index --force` re-indexes a track already present in the database (previously, re-running `index` on the same folder silently duplicated every hash for re-indexed tracks); `query --min-confidence <F>` rejects weak matches; `--robust` (global) switches both subcommands to triplet hashing.
- Python bindings: `PyIndex.query_topn` and `PyIndex.query_with_min_confidence`, mirroring the new `Index` methods.

## [0.2.0] - 2026-08-08

### Added

- `QueryResult.confidence` — `score` normalized by the query's fingerprint count, giving a `[0.0, 1.0]`-ish indicator of match decisiveness independent of clip length. Exposed through `Index::query`, `PersistentIndex::query`, the CLI (`Confidence: NN.N%`), and the Python bindings' query dict.

### Changed

- `wavio-cli index` now fingerprints files in parallel via `rayon` when built with the `parallel` feature, instead of processing files strictly sequentially. Insertion into the index remains serial.
- `Index::insert_batch_parallel` documentation corrected: it performs plain serial insertion (too cheap to benefit from a thread pool); callers should parallelize fingerprint *computation* upstream, as `wavio-cli` now does.

### Fixed

- `cargo bench` failed to compile: `benches/fingerprint.rs` constructed the `#[non_exhaustive]` `Fingerprint` struct via literal syntax from outside the crate. Switched to `Fingerprint::new(..)`.
- A pre-existing `clippy::all`-denied lint in `PersistentIndex::hash_count`.
- Stale documentation describing the persistence backend as `sled`; it was migrated to `redb` before the 0.1.0 release but README, crate docs, and the reference site still referenced `sled`. Also corrected two outdated "known limitations" entries (symphonia decoding and resampling are both implemented).

### Removed

- Unused `dashmap` optional dependency (declared under the `parallel` feature but never referenced in code).

## [0.1.0] - 2026-07-11

### Added

- **DSP Pipeline**
  - WAV file loading with stereo→mono downmix and f32 normalization (`dsp::audio`)
  - Sliding-window FFT with Hann windowing and dB power spectrogram (`dsp::spectrogram`)
  - 2D local-max peak detection with configurable neighborhood and threshold (`dsp::peaks`)
  - `AudioSource` trait for pluggable audio backends (`io::IOReader`)
  - Linear interpolation audio resampling to normalize any incoming sample rate to the internal `22,050 Hz` target standard.
  - Multi-format audio loading using `symphonia` decoding engine (supporting MP3, FLAC, AAC, M4A, OGG).

- **Fingerprinting**
  - Combinatorial hashing of peak pairs into `u64` fingerprints (`hash`)
  - Configurable fan value, time window, and frequency bins via `HashConfig`
  - Deterministic output regardless of input order
  - Unified pipeline orchestration engine (`Fingerprinter` struct) to run the full DSP pipeline in a single step.

- **Indexing & Querying**
  - In-memory `Index` with `HashMap<u64, Vec<(TrackId, f32)>>` storage
  - Time-offset histogram-based query matching with configurable bin size
  - `QueryResult` with track name, score, and estimated time offset

- **Persistence** (`persist` feature)
  - On-disk `PersistentIndex` backed by modern, transactional, B-tree database `redb` (replacing the deprecated `sled` engine).
  - Convenience methods `Index::save_to_disk()` and `Index::load_from_disk()`
  - Four-table redb layout: hashes, tracks_by_name, tracks_by_id, metadata

- **Parallelism** (`parallel` feature)
  - `extract_peaks_parallel()` — frame-level parallel peak detection via rayon
  - `generate_hashes_parallel()` — anchor-level parallel hash generation
  - `Index::insert_batch_parallel()` — parallel batch indexing

- **CLI** (`wavio-cli` binary)
  - `wavio index` — batch-index a directory of WAV/MP3/FLAC files
  - `wavio query` — identify a clip against the database
  - `wavio info` — print track/hash counts
  - Progress bar via `indicatif`, verbose mode with `--verbose`

- **Python Bindings** (`python` feature)
  - `PyFingerprinter` class with GIL-releasing `fingerprint_file()` method
  - `PyIndex` class with `insert()`, `query()`, `load()`, `save()` methods
  - Fully customizable configuration parameters passed to `PyFingerprinter` and `PyIndex` constructor signages, with read-only properties getters.
  - `maturin`-based build system

- **Documentation & Tooling**
  - `ARCHITECTURE.md` with pipeline diagram and design rationale
  - `CONTRIBUTING.md` with code style and PR checklist
  - `BENCHMARKS.md` with criterion baseline numbers
  - `tarpaulin.toml` configuration targeting >70% coverage.
  - GitHub Actions CI (fmt, clippy, multi-platform Linux + macOS matrix testing, MSRV check)
  - Comprehensive doc comments on all public items with `# Examples`
  - `#[non_exhaustive]` on all extensible public structs/enums

### Security

- `#![forbid(unsafe_code)]` — no unsafe blocks in the crate
- `cargo audit` integrated in CI

[0.1.0]: https://github.com/MinLee0210/wavio/releases/tag/v0.1.0
