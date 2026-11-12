# wavio

**Peak-based audio fingerprinting. Zero overhead. Written in Rust.**

`wavio` is a high-throughput acoustic fingerprinting library built for DSP engineers who need fast, deterministic audio identification without the weight of an ML stack. No embeddings, no models, no runtime — just spectral peaks, combinatorial hashing, and raw speed.

## Features

- **Full DSP pipeline** — WAV loading → FFT spectrogram → constellation peak detection → combinatorial hashing
- **In-memory & on-disk indexing** — query against thousands of tracks in microseconds
- **Ranked & threshold-gated queries** — `Index::query_topn` for runner-up candidates, `Index::query_with_min_confidence` to reject weak matches
- **Streaming fingerprinting** — `StreamingFingerprinter` processes audio in overlapping blocks, no need to hold the whole file in memory
- **Three hashing modes** — `pairwise` (default; best in noise), `triplet` (ratio-based; survives speed/pitch changes but is weak in noise) and `hybrid` (both in one index; survives speed/pitch changes *and* keeps pairwise's noise behavior, ~1.5x index size). See [accuracy](#accuracy)
- **Safe on-disk index** — answers queries straight from disk, appends in O(log n), records the settings it was built with and refuses to be queried with different ones
- **Match quality signals** — `confidence`, `margin` over the runner-up, and an optional playback-speed estimate
- **Deterministic** — same input always produces the same fingerprints
- **Zero unsafe code** — `#![forbid(unsafe_code)]`
- **Parallel processing** — optional rayon-based parallelism
- **Python bindings** — use from Python via PyO3/maturin
- **CLI tool** — index and query from the command line

## Installation

### As a Rust library

```toml
[dependencies]
wavio = "0.1"

# With optional features:
wavio = { version = "0.4", features = ["persist", "parallel"] }
```

### Feature Flags

| Flag | Description |
|------|-------------|
| `parallel` | Rayon-based parallel fingerprinting and peak extraction |
| `persist` | On-disk index persistence via `redb` |
| `python` | Python bindings via PyO3 |

### From source

```bash
git clone https://github.com/MinLee0210/wavio.git
cd wavio
cargo build --release --all-features
```

## Quick Start

```rust,no_run
use wavio::dsp::audio::load_wav;
use wavio::dsp::spectrogram::{compute_spectrogram, SpectrogramConfig};
use wavio::dsp::peaks::{extract_peaks, PeakExtractorConfig};
use wavio::hash::{generate_hashes, HashConfig};
use wavio::index::Index;

// Load → Spectrogram → Peaks → Hashes → Index → Query
let audio = load_wav("track.wav").unwrap();
let spec = compute_spectrogram(&audio.samples, &SpectrogramConfig::default()).unwrap();
let peaks = extract_peaks(&spec, &PeakExtractorConfig::default());
let hashes = generate_hashes(&peaks, &HashConfig::default());

let mut index = Index::default();
index.insert("my_track", &hashes);

let result = index.query(&hashes);
assert_eq!(result.unwrap().track_id, "my_track");
```

## CLI Usage

The `wavio-cli` binary provides three subcommands:

```bash
# Index a folder of WAV files into a database
wavio-cli index --db ./wavio.db ./music/

# Identify a clip against the database
wavio-cli query --db ./wavio.db ./clip.wav

# Print database statistics
wavio-cli info --db ./wavio.db
```

Add `--verbose` for detailed output (peak count, hash count, timing).

Other flags:

- `index --force` — re-index a track even if it's already in the database (by default, already-indexed tracks are skipped).
- `query --min-confidence <F>` — reject a match whose confidence is below `<F>` (0.0-1.0), reporting "no match" instead.
- `query --min-margin <F>` — reject a match that leads the runner-up by less than `<F>` of its own score.
- `query --detect-speed` — also search playback-speed hypotheses (+-8%) and print the estimated speed of the clip.
- `query --idf` — weight each matching hash by its rarity (hashes that occur in many places count for less). Never hurt in testing; helps the triplet/hybrid modes in noise by a few points.
- `--hybrid` (global) — index/query with both pairwise and triplet hashes. **Use it when speed or pitch changes are possible** (radio, DJ sets, tape/vinyl, time-stretched copies): +3% speed is 100% vs 18% for pairwise. For plain noise and compression, pairwise was as accurate on real recordings at two-thirds of the index size. Costs ~1.5x index size.
- `--robust` (global) — triplet hashes only (smaller index; but much weaker than pairwise or hybrid in noise on real audio).

A database records the hashing settings it was built with. Querying or extending it with different settings (e.g. `--hybrid` against a default database) fails with `Incompatible index` instead of silently returning "no match".

```bash
# Build the CLI (requires the persist feature)
cargo build --release --bin wavio-cli --features persist
```

## Python Bindings

Install via `maturin`:

```bash
cd python/
pip install maturin
maturin develop --features python
```

Usage:

```python
import wavio

fp = wavio.PyFingerprinter()          # or mode="triplet" / mode="hybrid"
hashes = fp.fingerprint_file("track.wav")

index = wavio.PyIndex()
index.insert("my_track", hashes)

result = index.query(hashes)
print(result)  # {'track_id': 'my_track', 'score': ..., 'offset_secs': ...}
```

See [python/README.md](python/README.md) for more details.

## Accuracy

Top-1 identification accuracy, 3-second clips, 40 synthetic tracks (see [`examples/06_accuracy_eval.rs`](examples/06_accuracy_eval.rs)):

| Distortion | pairwise | triplet | triplet + interpolation | **hybrid** |
|---|---:|---:|---:|---:|
| clean | 100% | 100% | 100% | 100% |
| noise 20 dB SNR | 100% | 90% | 90% | 100% |
| noise 0 dB SNR | 100% | 52% | 42% | 98% |
| speed +2% | 5% | 85% | 98% | 95% |
| speed -3% | 2% | 88% | 95% | 95% |
| speed +6% | 5% | 78% | 90% | 92% |
| noise 10 dB + speed +2% | 0% | 55% | 80% | 80% |
| index size (hashes/track) | 14.6k | 7.4k | 7.4k | 22.0k |

**Caveat:** that library is synthetic (random melodies), which is why pairwise looks perfect on noise. It supports *relative* comparison, not absolute claims.

Real recordings tell a more sober story. 20 real tracks, queries cut from the originals, converted to 48 kHz, optionally MP3-encoded, then resampled by wavio (60 queries per cell, so differences under ~8 points are not significant):

| Distortion | pairwise | triplet + interp | hybrid | hybrid + `--idf` |
|---|---:|---:|---:|---:|
| 48 kHz clean | 100% | 100% | 100% | 100% |
| MP3 32 kbps | 100% | 98% | 100% | 100% |
| noise 10 dB SNR | 93% | 27% | 83% | 87% |
| noise 0 dB SNR | 60% | 12% | 52% | 55% |
| speed +3% | 18% | 100% | 100% | 100% |

Takeaways: MP3 compression is a non-issue; real noise is much harder than synthetic noise; triplet-only is weak in noise; hybrid's edge over pairwise is speed/pitch robustness, not noise. Full tables for both harnesses are in [BENCHMARKS.md](.github/BENCHMARKS.md). The real-audio set is small (20 short tracks) and should be broadened.

## Benchmarks

> Linux x86-64 (12 threads), release profile, Criterion. Timings on this machine vary by about +-20% run to run.

| Benchmark | Result |
|---|---|
| Fingerprint a 3-min track (serial / `parallel` peaks) | ~0.2 s |
| Query a 10 s clip against 200 tracks / 10M entries (in memory) | ~0.7-1.0 ms |
| Query the same kind of clip against 50 tracks on disk | ~2 ms |
| Insert a 50,000-hash track into the on-disk index | ~36 ms (was ~290 ms) |

Reproduce with:

```bash
cargo bench --features parallel,persist
cargo run --release --features parallel --example 06_accuracy_eval
```

See [BENCHMARKS.md](.github/BENCHMARKS.md) for methodology and full results.

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for the pipeline diagram, design decisions (sample rate, FFT size, hash bit-packing), and known limitations.

## Contributing

See [.github/CONTRIBUTING.md](.github/CONTRIBUTING.md) for development setup, code style, and PR checklist.

## License

[MIT](./LICENSE)