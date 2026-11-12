# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0] - 2026-11-12

### Added

- **Hybrid hashing** (`Fingerprinter::with_hybrid_hashing`, CLI `--hybrid`, Python `mode="hybrid"`) — emits pairwise *and* triplet hashes into one index (triplet hashes carry `triplet::TRIPLET_TAG` so the families never collide). Pairwise hashes are the most noise-robust but collapse under speed/pitch change; triplet hashes survive speed/pitch change but are weaker in heavy noise. Their votes add in the same offset bin, so whichever survives carries the match. On the synthetic accuracy harness, 3 s clips at 0 dB noise: 98% (hybrid) vs 42–52% (triplet) vs 100% (pairwise); at +-6% speed: 92% vs 78–90% vs 5%. Cost: ~1.5x hashes per track.
- **Sub-bin peak interpolation** (`PeakExtractorConfig::interpolate`, `Fingerprinter::with_peak_interpolation`) — refines each peak's *frequency* with a parabolic fit. Off by default so existing hashes are unchanged. It removes most of the FFT-bin quantization error that ratio-based triplet hashes are sensitive to: 3 s clips at +2% speed go from 85% to 98% top-1 accuracy, noise+speed from 55% to 80%. Time is deliberately *not* interpolated — measured, it made triplet matching much worse. `wavio-cli --robust` now enables it.
- **Time-scale-aware voting** (`Index::query_scaled`, `PersistentIndex::query_scaled`, `ScaleSearch`, CLI `query --detect-speed`, Python `query_scaled`) — re-scores the top candidates under a grid of speed hypotheses and reports the estimated speed in the new `QueryResult::speed` (e.g. a 1.06x clip is reported as 1.06). Opt-in. It does **not** measurably improve identification accuracy in the benchmark; its value is the speed estimate.
- `QueryResult::margin` — lead over the runner-up as a fraction of the winning score. Unlike `confidence` it does not shrink just because the query contains audio that is not in the index. CLI: `query --min-margin <F>`; Python: `"margin"` in the result dict.
- `Fingerprinter::signature()` — a stable description of every setting that affects the hashes.
- `PersistentIndex`: `insert_many` (one transaction), `remove_track`, `contains_track`, `signature` / `check_signature` / `ensure_signature`, `config` / `set_config`. `Index::signature` / `set_signature`, `Index::insert_batch`.
- `WavioError::IncompatibleIndex`.
- `examples/06_accuracy_eval.rs` — an accuracy harness (top-1 accuracy by hashing mode x clip length x distortion). The library is synthetic, so use it for *relative* comparisons and regressions, not as an absolute leaderboard.
- Criterion benchmarks sized like a real library (~10M-entry index, 10 s noisy query clips, 3-minute multi-tone track) instead of 20-hash toy tracks.

- **Windowed-sinc resampler** (`dsp::audio::resample`). The old linear interpolation had no low-pass filter, so downsampling 44.1/48 kHz audio folded everything above 11.025 kHz back into the band. The new Kaiser-windowed-sinc filter removes it (15 kHz tone at 44.1 kHz: alias RMS < 0.01 vs > 0.2), has a flat passband with no added delay, and keeps levels correct at the edges. The old behavior remains as `resample_linear`. Cost: ~80-90 ms per 3-minute track serial (linear: ~25 ms); with the `parallel` feature the output samples are computed across threads, bit-identical to serial, at ~17-21 ms. Measured effect on identification accuracy over 20 real recordings is within noise (see BENCHMARKS.md); it is correctness, not a benchmark win.
- **Rarity (IDF) weighting** (`QueryOptions::idf_weighting`, `Index::query_with`, `PersistentIndex::query_with`, CLI `query --idf`, Python `idf=True`) — each vote is weighted by `1 / (1 + ln(postings))`, so hashes that occur in many places (coarse triplet ratios, looping music) count for less. `QueryResult::weighted_score` reports the ranking score; `score` stays the raw count of aligned hashes. On real recordings it helps the weaker modes under noise (triplet at 10 dB: 27% -> 37%, hybrid: 83% -> 87%) and is neutral otherwise. Off by default.
- **Real-recording evaluation** (`examples/06_accuracy_eval.rs --dir DIR`, `scripts/prepare_real_eval.sh`) — builds the library from real audio and sends query clips through the realistic path: cut from the original, converted to 48 kHz and optionally MP3-encoded by `ffmpeg`, then loaded and resampled by wavio. It showed that the synthetic harness was optimistic (see below).
- **Python bindings parity**: `PyFingerprinter(mode="pairwise"|"triplet"|"hybrid")`, `fingerprint_samples` (NumPy / `array('f')` / list, GIL released), `PyStreamingFingerprinter`, `PyPersistentIndex`, `signature` / `ensure_fingerprinter` / `check_fingerprinter`, `contains_track`, `idf=` on every query, and `margin` / `speed` / `weighted_score` in result dicts. Settings mismatches raise `ValueError`. 17 pytest tests, now run in CI.

### Changed

- **On-disk format (schema version 2).** The `hashes` table (one bincode `Vec` per hash, rewritten whole on every insert) is replaced by a `postings` multimap. Appending a fingerprint is now O(log n) regardless of how many tracks share the hash: inserting a 50,000-hash track takes ~36 ms instead of ~290 ms. **Databases from 0.3.0 and earlier are migrated automatically on first open**; databases written by this version cannot be opened by older releases. Identical `(hash, track, time)` entries are stored once, so saving an index twice no longer duplicates anything.
- **The database now records the settings it was built with** (hash mode, FFT/peak/hash parameters) and `wavio-cli` refuses to query or extend it with different ones (`Incompatible index: ...`). Previously, mixing `--robust` with a pairwise database silently returned "no match". Databases from before this release have no recorded settings; the CLI warns and assumes they match.
- `PersistentIndex::query*` now reads only the postings for the query's hashes straight from disk. `wavio-cli` no longer loads the whole database into RAM and rewrites it on every `index` / `query` run; `index` fingerprints in parallel and writes all new tracks in a single transaction.
- A corrupt stored `IndexConfig` is now an error instead of silently falling back to the default.
- **Scoring**: a track's score is its tallest *pair of adjacent* offset bins (votes straddling a bin boundary are no longer split), the offset is the vote-weighted mean of those bins, and ties are broken deterministically. `Index::query`, `query_topn`, `PersistentIndex::query` and `query_topn` now share a single implementation (they had been four copies).
- `Index::save_to_disk` is O(total entries) (it was O(tracks x entries)) and stores the index config and settings signature.
- `extract_peaks` / `extract_peaks_parallel` share one per-frame implementation; the parallel version is ~40% faster (35 -> 20 ms on a 3-minute track). The serial version is unchanged in speed. (A separable max-filter was tried and benchmarked ~2x *slower*, because the early-exit scan only pays the full window for actual peaks.)
- `cargo fmt` and `cargo clippy --all-targets --all-features -- -D warnings` are clean (CI enforces both).

### Deprecated

- `Index::insert_batch_parallel` — it never ran in parallel. Use `Index::insert_batch`.

### Fixed

- `.github/workflows/pypi.yml` built with default features, which produced a bindings-free `cffi` wheel; it now builds the PyO3 extension (`--features python,persist,parallel`). Note: the name `wavio` on PyPI is held by an unrelated project, so the upload step cannot succeed until the package is renamed.
- `wavio-cli index` re-run on the same folder with `--force` duplicated every hash of the re-indexed tracks (load, append, save); `--force` now replaces them.
- A multi-channel stream with more than 65,535 channels could silently truncate the channel count in the symphonia decoder.

### Findings from real recordings

20 real recordings (Ubuntu's bundled ringtones, 247 s total), 60 queries per cell, so differences under ~8 points are not significant:

- The synthetic harness **overstated noise robustness**: pairwise at 0 dB noise is 60% on real audio, not 100%. Triplet-only is far worse in noise on real audio (10 dB: ~27%) than on synthetic (90-100%).
- **Hybrid is not strictly better than pairwise.** Under noise alone it is statistically indistinguishable from pairwise (83-87% vs 93% at 10 dB), at 1.5x the index size; its advantage is speed/pitch robustness (100% vs 18% at +3% speed). The README now recommends hybrid when speed or pitch changes are possible, pairwise otherwise.
- MP3 compression is harmless: even 32 kbps scores 98-100% in every mode.
- The new resampler and IDF weighting are each worth at most a few points; neither hurts any mode.

### Tried and rejected

Recorded so nobody repeats them; all were measured with `examples/06_accuracy_eval.rs`:

- **Interpolating peak *time*** as well as frequency: turns frame-aligned peaks into near-coincident ones whose time ratios are pure jitter; accuracy fell sharply (e.g. +2% speed, 3 s: 90% -> 50%).
- **All-pairs triplets** (every pair among the nearest candidates rather than adjacent ones): no gain on any distortion and worse on noise, at ~2x the hashes.
- **Coarser triplet ratio quantization** (0.02-0.05 log2 steps) and a **minimum anchor distance** (`min_dt = 0.1`): both worse on speed and noise.

## [0.3.0] - 2026-09-06

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
