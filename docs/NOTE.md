# NOTE


## 15/03/2026

### On-Disk Persistence (`sled`)
- Verified `PersistentIndex` which wraps `sled::Db`.
- It uses four trees: `hashes` (hash to `Vec<(TrackId, f32)>`), `tracks_by_name`, `tracks_by_id`, and `metadata`.
- Added documentation noting that `sled` is currently in maintenance mode and `redb` is recommended as a future migration target.

### Parallelism (`rayon`) & Benchmarking (`criterion`)
- Gated parallel processing behind the `parallel` feature flag.
- Added `extract_peaks_parallel` to `peaks.rs`. It distributes frame-level local-max computation across the thread pool (embarrassingly parallel since it's read-only on the spectrogram).
- Added `generate_hashes_parallel` to `hash.rs` to compute each anchor's target pairs independently.
- Added `Index::insert_batch_parallel` to `index.rs` which performs the CPU-heavy DSP fingerprinting concurrently, then inserts into the `HashMap` serially. Decided against `DashMap` as it's unnecessary since only the cheap insertion phase is serial.
- Created `benches/fingerprint.rs` with 3 benchmarks: `fingerprint_single_3min`, `index_insert_1k_tracks`, and `index_query_1k`.
- Baseline numbers recorded in `docs/BENCHMARKS.md`:
  - ~88.6 ms to fingerprint a 3-minute clip
  - ~1.04 ms to insert 1,000 tracks
  - ~568 µs to perform 1,000 queries (~0.57 µs per query)

## 10/03/2026

- data samples: https://www.mmsp.ece.mcgill.ca/Documents/AudioFormats/WAVE/Samples.html