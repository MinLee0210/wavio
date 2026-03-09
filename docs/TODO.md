# wavio — 3-Month Development Roadmap
> Peak-based audio fingerprinting in Rust · ~10 hrs/week · ~120 hrs total

---

## Quick Reference

| Symbol | Meaning |
|--------|---------|
| `[ ]`  | Not started |
| `[~]`  | In progress |
| `[x]`  | Done |
| ⚠️     | Potential blocker |
| 🦀     | Rust-specific learning curve |

---

## Month 1 — Core DSP Pipeline
> **Goal:** Raw audio in → fingerprints out. Everything else builds on this.  
> **Budget:** ~40 hrs · Weeks 1–4

---

### Week 1 — Project Scaffolding (~8 hrs)

- [x] Initialize Cargo workspace with `cargo new wavio --lib`
- [x] Set up `Cargo.toml` with initial dependencies (`rustfft`, `ndarray`, `hound`)
- [ ] Configure `clippy` + `rustfmt` with strict lints (`#![deny(clippy::all)]`)
- [x] Set up GitHub repo, `.gitignore`, branch protection on `main`
- [ ] Set up GitHub Actions CI — `cargo test`, `cargo clippy`, `cargo fmt --check`
- [ ] Write the top-level `lib.rs` with public module skeleton (`dsp`, `index`, `hash`)
- [ ] Define core error type using `thiserror` — `WavioError` enum
- [ ] Write a `CONTRIBUTING.md` — even for solo projects, forces you to think about API design
- [ ] **Milestone:** `cargo build` passes, CI is green

---

### Week 2 — Audio Ingestion (~10 hrs)

- [ ] Implement WAV loader using `hound` — stereo → mono downmix, normalize to `f32`
- [ ] Write unit test: load a known WAV, assert sample count and sample rate
- [ ] Add `symphonia` feature flag (`features = ["symphonia"]`) for MP3/AAC/FLAC
- [ ] Implement `AudioSource` trait — abstraction over WAV and symphonia decoders
- [ ] 🦀 Handle `symphonia`'s `Decoder` trait objects carefully — boxing required
- [ ] Write integration test: load MP3 and WAV of same file, assert same sample length
- [ ] Add resampling stub — note: full resampling deferred to Month 2
- [ ] ⚠️ Decide on internal sample rate standard (recommend: 22,050 Hz) — document this decision in `ARCHITECTURE.md`
- [ ] **Milestone:** Can load WAV and MP3 files into a normalized `Vec<f32>`

---

### Week 3 — FFT & Spectrogram (~12 hrs)

- [ ] Implement sliding window iterator over PCM samples — configurable `window_size` and `hop_size`
- [ ] Apply Hann window function to each frame before FFT
- [ ] Integrate `rustfft` — compute real-to-complex FFT per window frame
- [ ] Convert complex FFT output to power spectrum (magnitude squared)
- [ ] Convert power to dB scale: `10 * log10(power + 1e-10)`
- [ ] 🦀 Learn `ndarray` Array2 layout — store spectrogram as `Array2<f32>` (shape: `[n_frames, n_bins]`)
- [ ] Write unit test: sine wave at known frequency should show peak at that bin
- [ ] Write unit test: all-zeros input should produce all-`-inf` dB spectrogram
- [ ] Benchmark FFT pipeline with `criterion` — establish baseline ms/track
- [ ] ⚠️ FFT size choice (2048 vs 4096) affects frequency resolution vs time resolution — test both, document tradeoff
- [ ] **Milestone:** Can generate a correct spectrogram from a WAV file

---

### Week 4 — Peak Detection (~10 hrs)

- [ ] Implement 2D local maximum filter over spectrogram (neighborhood size: configurable)
- [ ] Apply amplitude threshold filter — discard peaks below `threshold_db` (default: `-40.0`)
- [ ] Return peaks as `Vec<Peak>` where `Peak { time: f32, freq: f32, amplitude: f32 }`
- [ ] 🦀 Use `ndarray`'s `.windows()` for neighborhood scanning — avoid manual index arithmetic
- [ ] Write unit test: synthetic spectrogram with known peaks, assert exact recovery
- [ ] Write unit test: noisy spectrogram, assert peaks are above threshold only
- [ ] Tune default parameters (`neighborhood = 20`, `threshold_db = -40.0`) on real music files
- [ ] ⚠️ Too many peaks = slow hashing. Too few = poor recall. Target: 200–500 peaks per 10s clip
- [ ] Add `PeakExtractorConfig` struct with `Default` impl — all tunable params in one place
- [ ] **Milestone:** Can extract constellation points from a real song spectrogram

---

## Month 2 — Hashing, Indexing & Persistence
> **Goal:** Fingerprint database you can write to and query against.  
> **Budget:** ~40 hrs · Weeks 5–8

---

### Week 5 — Combinatorial Hashing (~10 hrs)

- [ ] Design hash encoding: `(freq1_bin, freq2_bin, delta_t_quantized)` → `u64`
- [ ] Implement `generate_hashes(peaks: &[Peak], fan_value: usize) -> Vec<(u64, f32)>`
  - Each hash paired with its anchor time `t1`
- [ ] Sort peaks by time before pairing — order matters for determinism
- [ ] Apply `max_dt` and `min_dt` constraints on peak pairs (default: `0.0`–`1.0` sec)
- [ ] 🦀 Use bit-packing for the hash: `freq1 << 40 | freq2 << 20 | delta_t` — fits in `u64` cleanly
- [ ] Write unit test: same audio segment always produces same hashes (determinism)
- [ ] Write unit test: two different songs produce non-overlapping hash sets (collision rate < 5%)
- [ ] Add `HashConfig` struct — `fan_value`, `min_dt`, `max_dt`, `freq_bins`
- [ ] **Milestone:** Can generate a stable, deterministic set of hashes from peaks

---

### Week 6 — In-Memory Index (~10 hrs)

- [ ] Design `Index` struct — wraps `HashMap<u64, Vec<(TrackId, f32)>>`
  - `TrackId` = `u32` internally, mapped to `String` name
- [ ] Implement `Index::insert(track_id: &str, hashes: Vec<(u64, f32)>)`
- [ ] Implement `Index::query(hashes: &[(u64, f32)]) -> Option<QueryResult>`
  - Build per-track time-offset histograms
  - Return track with highest histogram peak
- [ ] Define `QueryResult { track_id: String, score: u32, offset_secs: f32 }`
- [ ] Write unit test: index 1 track, query exact clip → correct match
- [ ] Write unit test: index 10 tracks, query clip from track 5 → correct match only
- [ ] Write unit test: query audio not in index → `None` result
- [ ] ⚠️ Offset histogram bin size affects accuracy — test `10ms` vs `50ms` bins
- [ ] Benchmark: query latency on 1k track index with `criterion`
- [ ] **Milestone:** Full round-trip — WAV file → index → query → correct track name

---

### Week 7 — On-Disk Persistence (`sled` feature) (~10 hrs)

- [ ] Add `persist` feature flag gating `sled` dependency
- [ ] Design on-disk schema: key = `hash (u64 as bytes)`, value = `Vec<(u32, f32)>` (bincode-encoded)
- [ ] Implement `PersistentIndex` wrapping `sled::Db` with same interface as `Index`
- [ ] Implement `PersistentIndex::open(path: &Path)` and `::flush()`
- [ ] 🦀 `sled` keys must be byte slices — use `u64::to_be_bytes()` for consistent ordering
- [ ] Implement merge: load `PersistentIndex` into memory for querying (hybrid approach)
- [ ] Write integration test: index tracks, drop process, reopen DB, query → still correct
- [ ] ⚠️ `sled` is in maintenance mode — document this, note `redb` as future alternative
- [ ] Add `Index::save_to_disk(path)` and `Index::load_from_disk(path)` convenience methods
- [ ] **Milestone:** Index survives process restart

---

### Week 8 — Parallelism + Benchmarking (~10 hrs)

- [ ] Add `rayon` dependency, gate behind `parallel` feature flag (on by default)
- [ ] Parallelize fingerprint generation: `tracks.par_iter().map(|t| fingerprint(t))`
- [ ] Parallelize peak extraction across spectrogram frames with `rayon`
- [ ] 🦀 `ndarray` + `rayon`: use `par_axis_iter` — tricky but worth it
- [ ] Set up `benches/` directory with `criterion` benchmarks:
  - `bench_fingerprint_single` — one 3-min WAV
  - `bench_index_1k` — index 1,000 synthetic tracks
  - `bench_query_1k` — 1,000 queries against 1k-track index
- [ ] Run benchmarks, record baseline numbers in `BENCHMARKS.md`
- [ ] ⚠️ `DashMap` for concurrent index writes — consider if needed for multi-threaded indexing
- [ ] **Milestone:** Parallel indexing working, benchmarks documented

---

## Month 3 — CLI, Python Bindings & Release
> **Goal:** Usable by others. Documented. Published.  
> **Budget:** ~40 hrs · Weeks 9–12

---

### Week 9 — CLI Tool (~10 hrs)

- [ ] Add `[[bin]]` target `wavio-cli` in `Cargo.toml`
- [ ] Add `clap` dependency with derive feature
- [ ] Implement `index` subcommand: `wavio index --db ./wavio.db ./music/*.mp3`
  - Walks directory, fingerprints all audio files, writes to persistent index
- [ ] Implement `query` subcommand: `wavio query --db ./wavio.db ./clip.wav`
  - Returns best match, score, and estimated time offset
- [ ] Implement `info` subcommand: `wavio info --db ./wavio.db` — track count, hash count
- [ ] Add `--verbose` flag — print peak count, hash count, query time
- [ ] Add progress bar with `indicatif` crate for batch indexing
- [ ] Write CLI integration tests using `assert_cmd` crate
- [ ] ⚠️ Error messages must be human-readable — DSP engineers will debug from CLI output
- [ ] **Milestone:** Can index a folder of music and identify a clip from the command line

---

### Week 10 — Python Bindings (`PyO3`) (~10 hrs)

- [ ] Add `pyo3` dependency with `extension-module` feature
- [ ] Create `python/` directory with `pyproject.toml` using `maturin`
- [ ] 🦀 `maturin develop` workflow — understand editable installs before writing bindings
- [ ] Expose `PyFingerprinter` class — `.fingerprint_file(path: str) -> list[tuple[int, float]]`
- [ ] Expose `PyIndex` class — `.insert(track_id, fingerprints)`, `.query(fingerprints) -> dict`
- [ ] Write Python test suite: `pytest tests/test_wavio.py`
- [ ] ⚠️ GIL handling — release GIL during fingerprinting with `py.allow_threads(|| ...)`
- [ ] Add `wavio` to PyPI via `maturin publish` (optional — can defer to v0.2)
- [ ] Write `python/README.md` with pip install + usage example
- [ ] **Milestone:** `import wavio` works in Python, full round-trip test passes

---

### Week 11 — Documentation & API Polish (~10 hrs)

- [ ] Write `//!` crate-level doc comment in `lib.rs` — overview, quick example, feature flags
- [ ] Write `///` doc comments on every public struct, trait, and function
- [ ] Add `# Examples` sections to all public functions — `cargo test --doc` must pass
- [ ] Run `cargo doc --open` — fix any broken links or missing docs
- [ ] Write `ARCHITECTURE.md`:
  - ASCII pipeline diagram
  - Design decisions and rationale (sample rate, FFT size, hash bit-packing)
  - Known limitations section
- [ ] Update `README.md` — add real benchmark numbers, installation, CLI usage
- [ ] Add `CHANGELOG.md` following Keep a Changelog format
- [ ] Review public API — rename anything ambiguous, seal internal traits with `pub(crate)`
- [ ] Run `cargo clippy -- -W clippy::pedantic`, fix all warnings
- [ ] ⚠️ Add `#[non_exhaustive]` on enums/structs you may extend — prevents breaking changes in v0.2
- [ ] **Milestone:** `cargo doc` is complete, zero warnings, all doc tests pass

---

### Week 12 — Testing, Hardening & Publish (~10 hrs)

- [ ] Write property-based tests with `proptest`:
  - Fingerprinting is deterministic across runs
  - Query always returns `None` for empty index
  - Score is monotonically higher for longer matching clips
- [ ] Set up code coverage with `cargo-tarpaulin` — target > 70%
- [ ] Test on Linux + macOS via GitHub Actions matrix build
- [ ] Run `cargo audit` — fix any known vulnerability advisories
- [ ] Pin MSRV in `Cargo.toml`: `rust-version = "1.75.0"`
- [ ] Do a dry run: `cargo publish --dry-run` — fix any packaging issues
- [ ] Tag `v0.1.0`, write GitHub release notes
- [ ] Publish to `crates.io`: `cargo publish`
- [ ] Announce on r/rust, This Week in Rust submissions, Hacker News (Show HN)
- [ ] ⚠️ `crates.io` publishes are permanent and immutable — double-check before publishing
- [ ] **Milestone:** `wavio = "0.1"` works in any Rust project worldwide 🎉

---

## Dependency Map

| Crate | Purpose | Feature Flag |
|-------|---------|-------------|
| `rustfft` | FFT computation | always |
| `ndarray` | Spectrogram array ops | always |
| `hound` | WAV decoding | always |
| `thiserror` | Error types | always |
| `symphonia` | MP3/AAC/FLAC decoding | `symphonia` |
| `rayon` | Parallel processing | `parallel` |
| `dashmap` | Concurrent hash map | `parallel` |
| `sled` | On-disk persistence | `persist` |
| `serde` + `bincode` | Serialization | `persist` |
| `clap` | CLI argument parsing | bin only |
| `indicatif` | Progress bars | bin only |
| `pyo3` + `maturin` | Python bindings | `python` |
| `criterion` | Benchmarks | dev |
| `proptest` | Property-based testing | dev |
| `assert_cmd` | CLI integration tests | dev |

---

## Hours Budget

| Month | Focus | Budget | Risk |
|-------|-------|--------|------|
| Month 1 | DSP pipeline — FFT, spectrogram, peaks | 40 hrs | Medium — `ndarray` learning curve |
| Month 2 | Hashing, in-memory index, persistence | 40 hrs | Low — straightforward Rust |
| Month 3 | CLI, PyO3 bindings, docs, publish | 40 hrs | High — PyO3/maturin setup is fiddly |

> **If Month 1 runs over:** cut `symphonia` support, WAV-only for v0.1, add symphonia in v0.2.  
> **If Month 3 runs over:** defer PyO3 bindings entirely — CLI + crates.io publish is a solid v0.1.  
> **Never cut:** tests, benchmarks, `ARCHITECTURE.md` — these pay off immediately.