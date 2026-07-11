# Rust Examples

## Helper: Fingerprint function

Most examples reuse this helper:

```rust
use wavio::dsp::audio::load_wav;
use wavio::dsp::spectrogram::{compute_spectrogram, SpectrogramConfig};
use wavio::dsp::peaks::{extract_peaks, PeakExtractorConfig};
use wavio::hash::{generate_hashes, Fingerprint, HashConfig};

fn fingerprint(path: &str) -> Result<Vec<Fingerprint>, wavio::error::WavioError> {
    let audio = load_wav(path)?;
    let spec = compute_spectrogram(&audio.samples, &SpectrogramConfig::default())?;
    let peaks = extract_peaks(&spec, &PeakExtractorConfig::default());
    Ok(generate_hashes(&peaks, &HashConfig::default()))
}
```

---

## Index and Query

```rust
use wavio::index::Index;

fn main() -> anyhow::Result<()> {
    let mut index = Index::default();

    // Index a library
    for path in &["song_a.wav", "song_b.wav", "song_c.wav"] {
        let name = path.replace(".wav", "");
        let hashes = fingerprint(path)?;
        println!("  ✓ {name}: {} hashes", hashes.len());
        index.insert(&name, &hashes);
    }

    // Query
    let clip = fingerprint("clip.wav")?;
    match index.query(&clip) {
        Some(r) => println!("Match: {} (score {}, offset {:.1}s)", r.track_id, r.score, r.offset_secs),
        None => println!("No match."),
    }

    Ok(())
}
```

---

## Parallel Batch Indexing

Requires the `parallel` feature.

```rust
use rayon::prelude::*;
use wavio::index::Index;

fn main() -> anyhow::Result<()> {
    let files = vec!["a.wav", "b.wav", "c.wav", "d.wav"];

    // Fingerprint in parallel
    let batch: Vec<(String, Vec<_>)> = files
        .par_iter()
        .filter_map(|path| {
            let name = path.replace(".wav", "");
            fingerprint(path).ok().map(|h| (name, h))
        })
        .collect();

    // Insert (serial, cheap)
    let mut index = Index::default();
    for (name, hashes) in &batch {
        index.insert(name, hashes);
    }

    println!("{} tracks indexed", index.track_count());
    Ok(())
}
```

---

## Persist to Disk

Requires the `persist` feature.

```rust
fn main() -> anyhow::Result<()> {
    // Build and save
    let mut index = wavio::index::Index::default();
    index.insert("track_a", &fingerprint("a.wav")?);
    index.save_to_disk("./music.db")?;

    // Load in another process
    let loaded = wavio::index::Index::load_from_disk("./music.db")?;
    println!("{} tracks ready", loaded.track_count());
    Ok(())
}
```

---

## Custom Pipeline Parameters

```rust
use wavio::dsp::audio::load_wav;
use wavio::dsp::spectrogram::{compute_spectrogram, SpectrogramConfig};
use wavio::dsp::peaks::{extract_peaks, PeakExtractorConfig};
use wavio::hash::{generate_hashes, HashConfig};

let audio = load_wav("speech.wav")?;

// Larger FFT window for better frequency resolution
let spec_config = SpectrogramConfig {
    window_size: 4096,
    hop_size: 1024,
};
let spec = compute_spectrogram(&audio.samples, &spec_config)?;

// Higher threshold for cleaner peaks
let peak_config = PeakExtractorConfig {
    threshold_db: -30.0,
    time_neighborhood: 15,
    freq_neighborhood: 15,
    window_size: 4096,  // must match spectrogram
    hop_size: 1024,     // must match spectrogram
    ..PeakExtractorConfig::default()
};
let peaks = extract_peaks(&spec, &peak_config);

// Wider time window for speech
let hash_config = HashConfig {
    max_dt: 2.0,
    fan_value: 10,
    ..HashConfig::default()
};
let hashes = generate_hashes(&peaks, &hash_config);
```

---

## Error Handling

```rust
use wavio::error::WavioError;
use wavio::dsp::audio::load_wav;

fn safe_fingerprint(path: &str) {
    match load_wav(path) {
        Ok(audio) => println!("Loaded {:.1}s of audio", audio.duration_secs()),
        Err(WavioError::FileNotFound(p)) => eprintln!("File not found: {p}"),
        Err(WavioError::InvalidWavFormat(msg)) => eprintln!("Bad WAV: {msg}"),
        Err(WavioError::AudioTooShort) => eprintln!("Audio too short for fingerprinting"),
        Err(e) => eprintln!("Unexpected error: {e}"),
    }
}
```
