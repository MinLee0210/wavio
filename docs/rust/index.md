# Rust Guide

`wavio` is a native Rust library with zero unsafe code. It gives you full control over every stage of the DSP pipeline.

---

## Overview

The library is organized into modules that mirror the pipeline:

```
wavio::dsp::audio       →  Load WAV, normalize to mono f32
wavio::dsp::spectrogram →  Sliding-window FFT → dB spectrogram
wavio::dsp::peaks       →  2D local-max peak extraction
wavio::hash             →  Combinatorial hashing → u64 fingerprints
wavio::index            →  In-memory index + query engine
wavio::persist          →  On-disk sled backend (feature = "persist")
wavio::io               →  Pluggable audio source trait
wavio::error            →  WavioError enum
```

## Minimal Example

```rust
use wavio::dsp::audio::load_wav;
use wavio::dsp::spectrogram::{compute_spectrogram, SpectrogramConfig};
use wavio::dsp::peaks::{extract_peaks, PeakExtractorConfig};
use wavio::hash::{generate_hashes, HashConfig};
use wavio::index::Index;

let audio = load_wav("track.wav").unwrap();
let spec = compute_spectrogram(&audio.samples, &SpectrogramConfig::default()).unwrap();
let peaks = extract_peaks(&spec, &PeakExtractorConfig::default());
let hashes = generate_hashes(&peaks, &HashConfig::default());

let mut index = Index::default();
index.insert("track", &hashes);
```

## Error Handling

All fallible operations return `Result<T, WavioError>`:

```rust
use wavio::error::WavioError;

match load_wav("missing.wav") {
    Ok(audio) => { /* ... */ }
    Err(WavioError::FileNotFound(path)) => eprintln!("Not found: {path}"),
    Err(e) => eprintln!("Error: {e}"),
}
```

---

[:octicons-arrow-right-24: API Reference](api.md) — all public types and functions

[:octicons-arrow-right-24: Examples](examples.md) — real-world patterns
