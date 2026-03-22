# wavio

**Peak-based audio fingerprinting. Zero overhead. Written in Rust.**

`wavio` is a high-throughput acoustic fingerprinting library built for DSP engineers who need fast, deterministic audio identification without the weight of an ML stack. No embeddings, no models, no runtime — just spectral peaks, combinatorial hashing, and raw speed.

## Features

- **Full DSP pipeline** — WAV loading → FFT spectrogram → constellation peak detection → combinatorial hashing
- **In-memory & on-disk indexing** — query against thousands of tracks in microseconds
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
wavio = { version = "0.1", features = ["persist", "parallel"] }
```

### Feature Flags

| Flag | Description |
|------|-------------|
| `parallel` | Rayon-based parallel fingerprinting and peak extraction |
| `persist` | On-disk index persistence via `sled` |
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

fp = wavio.PyFingerprinter()
hashes = fp.fingerprint_file("track.wav")

index = wavio.PyIndex()
index.insert("my_track", hashes)

result = index.query(hashes)
print(result)  # {'track_id': 'my_track', 'score': ..., 'offset_secs': ...}
```

See [python/README.md](python/README.md) for more details.

## Benchmarks

> Platform: macOS, release profile · Criterion · 22,050 Hz synthetic audio

| Benchmark | Median |
|-----------|--------|
| Fingerprint single 3-min track | 88.6 ms |
| Index 1,000 tracks (20 hashes each) | 1.04 ms |
| Query 1,000 lookups | 568 µs (~0.57 µs/query) |

Reproduce with:

```bash
cargo bench --features parallel
```

See [docs/BENCHMARKS.md](docs/BENCHMARKS.md) for full results.

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for the pipeline diagram, design decisions (sample rate, FFT size, hash bit-packing), and known limitations.

## Contributing

See [.github/CONTRIBUTING.md](.github/CONTRIBUTING.md) for development setup, code style, and PR checklist.

## License

[MIT](./LICENSE)