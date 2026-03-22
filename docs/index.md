---
hide:
  - navigation
---

# wavio

**Peak-based audio fingerprinting. Zero overhead. Written in Rust.**

<p style="font-size: 1.2em; color: #888;">
Identify audio tracks in microseconds — no ML, no embeddings, just spectral peaks and combinatorial hashing.
</p>

---

<div class="grid cards" markdown>

-   :material-language-python:{ .lg .middle } **Python Bindings**

    ---

    Use wavio from Python with familiar APIs. Fingerprint files, build indices, and query tracks.

    [:octicons-arrow-right-24: Python Guide](python/index.md)

-   :material-language-rust:{ .lg .middle } **Rust Library**

    ---

    Full control over the DSP pipeline. Zero-cost abstractions, no unsafe code.

    [:octicons-arrow-right-24: Rust Guide](rust/index.md)

-   :material-console:{ .lg .middle } **CLI Tool**

    ---

    Index and query audio from the command line. Batch process entire folders.

    [:octicons-arrow-right-24: CLI Reference](cli.md)

-   :material-speedometer:{ .lg .middle } **Performance**

    ---

    88ms to fingerprint a 3-min track. 0.57µs per query. No GPU required.

    [:octicons-arrow-right-24: Benchmarks](reference/benchmarks.md)

</div>

---

## How It Works

```
Audio File (.wav)
  ↓  load & normalize (mono f32 @ 22,050 Hz)
    ↓  sliding-window FFT (Hann, 2048-sample frames)
      ↓  dB power spectrogram
        ↓  2D local-max peak detection
          ↓  combinatorial hashing (peak pairs → u64)
            ↓  index  →  query  →  match!
```

Two recordings of the same song — even with background noise — share a large subset of their spectral peaks. The query engine finds the track whose hash times align best via a time-offset histogram, and returns the match with its score and position.

---

## Quick Install

=== "Rust"

    ```toml
    [dependencies]
    wavio = { version = "0.1", features = ["persist", "parallel"] }
    ```

=== "Python"

    ```bash
    pip install maturin
    git clone https://github.com/MinLee0210/wavio.git
    cd wavio/python && maturin develop --features python
    ```

=== "CLI"

    ```bash
    cargo install wavio --features persist
    ```

[:octicons-arrow-right-24: Full installation guide](getting-started/installation.md)

---

## Quick Example

=== "Python"

    ```python
    import wavio

    fp = wavio.PyFingerprinter()
    index = wavio.PyIndex()

    # Index a track
    hashes = fp.fingerprint_file("song.wav")
    index.insert("My Song", hashes)

    # Identify a clip
    clip = fp.fingerprint_file("clip.wav")
    result = index.query(clip)
    print(result)  # {'track_id': 'My Song', 'score': 312, 'offset_secs': 47.3}
    ```

=== "Rust"

    ```rust
    use wavio::dsp::audio::load_wav;
    use wavio::dsp::spectrogram::{compute_spectrogram, SpectrogramConfig};
    use wavio::dsp::peaks::{extract_peaks, PeakExtractorConfig};
    use wavio::hash::{generate_hashes, HashConfig};
    use wavio::index::Index;

    let audio = load_wav("song.wav").unwrap();
    let spec = compute_spectrogram(&audio.samples, &SpectrogramConfig::default()).unwrap();
    let peaks = extract_peaks(&spec, &PeakExtractorConfig::default());
    let hashes = generate_hashes(&peaks, &HashConfig::default());

    let mut index = Index::default();
    index.insert("My Song", &hashes);
    let result = index.query(&hashes);
    ```

=== "CLI"

    ```bash
    wavio-cli index --db music.db ./songs/
    wavio-cli query --db music.db clip.wav
    ```

[:octicons-arrow-right-24: Full quick start guide](getting-started/quickstart.md)
