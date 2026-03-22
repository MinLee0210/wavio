# Quick Start

This guide walks through the core workflow: **fingerprint → index → query**.

---

## Python

### 1. Fingerprint an audio file

```python
import wavio

fp = wavio.PyFingerprinter()
hashes = fp.fingerprint_file("song.wav")
print(f"Generated {len(hashes)} fingerprints")
# Generated 4521 fingerprints
```

Each fingerprint is a `(hash: int, anchor_time: float)` tuple.

### 2. Build an index

```python
index = wavio.PyIndex()
index.insert("Song A", fp.fingerprint_file("song_a.wav"))
index.insert("Song B", fp.fingerprint_file("song_b.wav"))
index.insert("Song C", fp.fingerprint_file("song_c.wav"))

print(f"{index.track_count} tracks, {index.hash_count} hashes")
```

### 3. Query a clip

```python
clip_hashes = fp.fingerprint_file("clip.wav")
result = index.query(clip_hashes)

if result:
    print(f"Match: {result['track_id']}")
    print(f"Score: {result['score']}")
    print(f"Offset: {result['offset_secs']:.1f}s into the track")
else:
    print("No match found.")
```

### 4. Save and load (optional)

```python
# Save for later
index.save("music.db")

# In a new session
index = wavio.PyIndex.load("music.db")
```

---

## Rust

### 1. Fingerprint an audio file

```rust
use wavio::dsp::audio::load_wav;
use wavio::dsp::spectrogram::{compute_spectrogram, SpectrogramConfig};
use wavio::dsp::peaks::{extract_peaks, PeakExtractorConfig};
use wavio::hash::{generate_hashes, HashConfig};

fn fingerprint(path: &str) -> Vec<wavio::hash::Fingerprint> {
    let audio = load_wav(path).expect("failed to load");
    let spec = compute_spectrogram(&audio.samples, &SpectrogramConfig::default()).unwrap();
    let peaks = extract_peaks(&spec, &PeakExtractorConfig::default());
    generate_hashes(&peaks, &HashConfig::default())
}

let hashes = fingerprint("song.wav");
println!("{} fingerprints", hashes.len());
```

### 2. Build an index

```rust
use wavio::index::Index;

let mut index = Index::default();
index.insert("Song A", &fingerprint("song_a.wav"));
index.insert("Song B", &fingerprint("song_b.wav"));

println!("{} tracks, {} hashes", index.track_count(), index.hash_count());
```

### 3. Query a clip

```rust
let clip = fingerprint("clip.wav");

match index.query(&clip) {
    Some(result) => {
        println!("Match: {}", result.track_id);
        println!("Score: {}", result.score);
        println!("Offset: {:.1}s", result.offset_secs);
    }
    None => println!("No match found."),
}
```

### 4. Save and load (optional, requires `persist` feature)

```rust
// Save
index.save_to_disk("./music.db")?;

// Load
let index = Index::load_from_disk("./music.db")?;
```

---

## CLI

```bash
# Index a folder
wavio-cli index --db music.db ./songs/

# Query a clip
wavio-cli query --db music.db clip.wav
# Match found: Song A
# Score: 312
# Offset: 47.35s

# Database info
wavio-cli info --db music.db
# Tracks indexed: 142
# Total hashes: 581,493
```

---

## Next Steps

- [:octicons-arrow-right-24: Python API Reference](../python/api.md) — full method signatures and details
- [:octicons-arrow-right-24: Rust API Reference](../rust/api.md) — all public types and functions
- [:octicons-arrow-right-24: Configuration](../reference/configuration.md) — tune parameters for your use case
