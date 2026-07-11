# Rust API Reference

## `dsp::audio`

WAV file loading and normalization.

### `load_wav(path: &str) -> Result<AudioData, WavioError>`

Loads a WAV file (any bit depth, stereo or mono) and returns normalized mono `f32` samples.

```rust
use wavio::dsp::audio::load_wav;

let audio = load_wav("track.wav")?;
println!("Duration: {:.1}s", audio.duration_secs());
println!("Samples: {}", audio.num_samples());
```

### `AudioData`

| Field | Type | Description |
|-------|------|-------------|
| `samples` | `Vec<f32>` | Mono PCM samples in `[-1.0, 1.0]` |
| `sample_rate` | `u32` | Native sample rate |
| `original_channels` | `u16` | Channels before downmixing |

| Method | Returns | Description |
|--------|---------|-------------|
| `duration_secs()` | `f32` | Duration in seconds |
| `num_samples()` | `usize` | Total mono sample count |

### `INTERNAL_SAMPLE_RATE: u32 = 22_050`

Standard sample rate used throughout the DSP pipeline.

---

## `dsp::spectrogram`

Sliding-window FFT spectrograms.

### `compute_spectrogram(samples, config) -> Result<Array2<f32>, WavioError>`

Computes a dB power spectrogram. Output shape: `[n_frames, window_size/2 + 1]`.

```rust
use wavio::dsp::spectrogram::{compute_spectrogram, SpectrogramConfig};

let config = SpectrogramConfig { window_size: 2048, hop_size: 512 };
let spec = compute_spectrogram(&audio.samples, &config)?;
println!("Shape: {:?}", spec.shape()); // [n_frames, 1025]
```

### `SpectrogramConfig`

| Field | Default | Description |
|-------|---------|-------------|
| `window_size` | `2048` | FFT window size in samples (power of 2) |
| `hop_size` | `512` | Step between windows |

### `hann_window(size: usize) -> Vec<f32>`

Generates a Hann window of the given length.

---

## `dsp::peaks`

Constellation peak extraction.

### `extract_peaks(spectrogram, config) -> Vec<Peak>`

Extracts local spectral maxima above a dB threshold. Returns peaks sorted by time, then frequency.

```rust
use wavio::dsp::peaks::{extract_peaks, PeakExtractorConfig};

let peaks = extract_peaks(&spec, &PeakExtractorConfig::default());
println!("{} peaks found", peaks.len());
```

### `extract_peaks_parallel(spectrogram, config) -> Vec<Peak>`

Parallel variant using rayon. Requires the `parallel` feature.

### `PeakExtractorConfig`

| Field | Default | Description |
|-------|---------|-------------|
| `time_neighborhood` | `10` | Half-width of neighborhood window (frames) |
| `freq_neighborhood` | `10` | Half-width of neighborhood window (bins) |
| `threshold_db` | `-40.0` | Minimum amplitude (dB) for a peak |
| `sample_rate` | `22_050` | Audio sample rate |
| `hop_size` | `512` | Hop size matching spectrogram |
| `window_size` | `2048` | FFT window size matching spectrogram |

### `Peak`

| Field | Type | Description |
|-------|------|-------------|
| `time` | `f32` | Position in seconds |
| `freq` | `f32` | Frequency in Hz |
| `amplitude` | `f32` | Amplitude in dB |

---

## `hash`

Combinatorial fingerprint hash generation.

### `generate_hashes(peaks, config) -> Vec<Fingerprint>`

Pairs anchor peaks with subsequent targets within the time window, producing `u64` hashes.

```rust
use wavio::hash::{generate_hashes, HashConfig};

let hashes = generate_hashes(&peaks, &HashConfig::default());
println!("{} fingerprints", hashes.len());
```

### `generate_hashes_parallel(peaks, config) -> Vec<Fingerprint>`

Parallel variant using rayon. Requires the `parallel` feature.

### `HashConfig`

| Field | Default | Description |
|-------|---------|-------------|
| `fan_value` | `15` | Max target peaks per anchor |
| `min_dt` | `0.0` | Min time gap (seconds) |
| `max_dt` | `1.0` | Max time gap (seconds) |
| `freq_bins` | `1024` | Frequency bins for quantization |
| `freq_resolution` | `~10.77` | Hz per bin |
| `dt_resolution` | `0.01` | Time quantization step (seconds) |

### `Fingerprint`

| Field | Type | Description |
|-------|------|-------------|
| `hash` | `u64` | Packed `(freq1, freq2, delta_t)` |
| `anchor_time` | `f32` | Time of anchor peak (seconds) |

Hash bit layout:

```
 Bits 40–59    Bits 20–39    Bits 0–19
┌─────────────┬─────────────┬─────────────┐
│  freq1_bin  │  freq2_bin  │   delta_t   │
│   (20 bits) │   (20 bits) │   (20 bits) │
└─────────────┴─────────────┴─────────────┘
```

---

## `index`

In-memory fingerprint index and query engine.

### `Index`

```rust
use wavio::index::{Index, IndexConfig};

let mut index = Index::default();
// or with custom config:
let mut index = Index::new(IndexConfig { offset_bin_size: 0.01 });
```

| Method | Description |
|--------|-------------|
| `new(config)` | Create with custom config |
| `insert(track_name, fingerprints)` | Add a track |
| `query(fingerprints) -> Option<QueryResult>` | Find best match |
| `track_count() -> usize` | Number of indexed tracks |
| `hash_count() -> usize` | Total hash entries |
| `save_to_disk(path)` | Save to sled DB *(persist feature)* |
| `load_from_disk(path) -> Result<Index>` | Load from sled DB *(persist feature)* |
| `insert_batch_parallel(batch)` | Parallel batch insert *(parallel feature)* |

### `QueryResult`

| Field | Type | Description |
|-------|------|-------------|
| `track_id` | `String` | Name of matching track |
| `score` | `u32` | Aligned hash hit count |
| `offset_secs` | `f32` | Estimated position in track (seconds) |

### `IndexConfig`

| Field | Default | Description |
|-------|---------|-------------|
| `offset_bin_size` | `0.05` | Histogram bin width (seconds) |

---

## `persist`

On-disk fingerprint index backed by sled. Requires the `persist` feature.

### `PersistentIndex`

```rust
use wavio::persist::PersistentIndex;

let mut db = PersistentIndex::open("./music.db")?;
db.insert("Song A", &hashes)?;
db.flush()?;

let result = db.query(&clip_hashes);
let mem_index = db.load_into_memory()?;
```

| Method | Description |
|--------|-------------|
| `open(path)` | Open or create database |
| `insert(name, fingerprints)` | Persist a track |
| `query(fingerprints)` | Query from disk |
| `flush()` | Force durability flush |
| `track_count()` | Number of tracks |
| `hash_count()` | Total hash entries |
| `load_into_memory()` | Convert to `Index` |

---

## `io`

Pluggable audio source trait.

### `IOReader` trait

```rust
pub trait IOReader {
    fn read(&self) -> Result<Vec<f32>, WavioError>;
}
```

### `FileIOReader`

```rust
use wavio::io::{FileIOReader, IOReader};

let reader = FileIOReader::new("song.wav");
let samples = reader.read()?;
```

---

## `error`

### `WavioError`

All fallible operations return `Result<T, WavioError>`. The enum is `#[non_exhaustive]`.

| Variant | Description |
|---------|-------------|
| `FileNotFound(String)` | Path does not exist |
| `FileNotReadable(String)` | File cannot be opened |
| `InvalidWavFormat(String)` | Not a valid WAV file |
| `UnsupportedAudioFormat(String)` | Format not supported |
| `AudioTooShort` | Too few samples |
| `FftError(String)` | FFT computation failed |
| `SpectrogramError(String)` | Spectrogram generation failed |
| `NoPeaksFound` | No peaks detected |
| `HashingError(String)` | Hash generation failed |
| `IndexError(String)` | Index read/write error |
| `IoError(String)` | General I/O error |
