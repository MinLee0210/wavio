# Architecture

> How wavio turns raw audio into identifiable fingerprints.

## Pipeline Diagram

```
┌─────────────┐
│  WAV File   │
└──────┬──────┘
       │  load_wav()                  dsp/audio.rs
       ▼
┌─────────────┐
│  AudioData  │  mono f32 @ 22,050 Hz
└──────┬──────┘
       │  compute_spectrogram()       dsp/spectrogram.rs
       ▼
┌─────────────────┐
│  Spectrogram    │  Array2<f32>  [n_frames × n_bins]
│  (dB power)     │  Hann window, 2048-pt FFT, hop=512
└──────┬──────────┘
       │  extract_peaks()             dsp/peaks.rs
       ▼
┌─────────────┐
│  Vec<Peak>  │  Constellation points (time, freq, amp)
└──────┬──────┘
       │  generate_hashes()           hash.rs
       ▼
┌──────────────────┐
│ Vec<Fingerprint> │  (u64 hash, f32 anchor_time) pairs
└──────┬───────────┘
       │  index.insert()              index.rs
       ▼
┌─────────────┐     index.query()
│    Index    │ ─────────────────► QueryResult
│  (HashMap)  │                    { track_id, score, offset }
└─────────────┘
       │  save_to_disk()              persist.rs  (feature = "persist")
       ▼
┌─────────────┐
│  sled DB    │  On-disk persistence
└─────────────┘
```

---

## Design Decisions

### Internal Sample Rate: 22,050 Hz

- Nyquist frequency of 11,025 Hz — sufficient for music fingerprinting
- Half the samples of 44,100 Hz — faster processing
- Standard in audio fingerprinting research

Defined as `INTERNAL_SAMPLE_RATE` in `dsp/audio.rs`.

### FFT Size: 2048 samples

- **Frequency resolution:** ~10.77 Hz per bin (22,050 / 2,048)
- **Time resolution per frame:** ~23.2 ms (512 / 22,050) with hop=512

4096-sample FFT was considered but doubles compute cost with diminishing returns for music identification.

### Hash Bit-Packing Layout

```
 Bits 40–59   Bits 20–39   Bits 0–19
┌────────────┬────────────┬────────────┐
│ freq1_bin  │ freq2_bin  │  delta_t   │
│  (20 bits) │  (20 bits) │  (20 bits) │
└────────────┴────────────┴────────────┘
```

20 bits per field supports up to 1,048,575 values. Packing into `u64` makes hashing and HashMap lookups trivially fast.

### Offset Histogram Query

1. For each query hash found in the index, compute `offset = db_time - query_time`
2. Quantize offset into bins (default 50 ms)
3. Track with tallest histogram bin wins

### Persistence: sled

Embedded key-value store with zero-config setup. Four sled trees:

| Tree | Key | Value |
|------|-----|-------|
| `hashes` | `u64` (big-endian bytes) | bincode `Vec<(TrackId, f32)>` |
| `tracks_by_name` | track name | `u32` ID |
| `tracks_by_id` | `u32` ID | track name |
| `metadata` | `"config"`, `"next_id"` | config / counter |

!!! note "sled is in maintenance mode"
    The API is designed to be backend-agnostic. Migration to [`redb`](https://crates.io/crates/redb) is planned for v0.2.

---

## Module Map

```
src/
  lib.rs             Crate root — lint config, module declarations
  error.rs           WavioError enum (thiserror)
  utils.rs           File validation helpers
  dsp/
    mod.rs           DSP pipeline re-exports
    audio.rs         WAV loading, mono downmix, f32 normalization
    spectrogram.rs   Sliding-window FFT → dB power spectrogram
    peaks.rs         2D local-max constellation point extraction
  hash.rs            Combinatorial hashing → u64 fingerprints
  index.rs           In-memory index + query engine
  persist.rs         On-disk sled persistence (feature = "persist")
  python.rs          PyO3 bindings (feature = "python")
  io/
    mod.rs           I/O trait re-exports
    base.rs          IOReader trait
    file.rs          File-based IOReader
  bin/
    wavio-cli.rs     CLI (index, query, info)
```

---

## Known Limitations

1. **WAV-only input.** MP3/AAC/FLAC via `symphonia` is stubbed but not implemented in v0.1.
2. **No resampling.** Audio must be at 22,050 Hz for correct results.
3. **sled backend.** Maintenance-mode dependency. Migration to `redb` planned.
4. **No concurrent writes.** The `Index` uses `HashMap`; use `insert_batch_parallel` for parallel indexing.
5. **Music-tuned defaults.** Speech or environmental audio may need parameter tuning.
