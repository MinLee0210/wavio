# wavio-python

Python bindings for the `wavio` peak-based audio fingerprinting library, written in Rust.

## Installation

You can install `wavio` from source using `maturin`:
```bash
pip install maturin
maturin develop
```

## Basic Usage

```python
import wavio

# Fingerprint a song
fingerprinter = wavio.PyFingerprinter()
hashes = fingerprinter.fingerprint_file("song.wav")

# Create an index
index = wavio.PyIndex()
index.insert("My Song", hashes)

# Query a clip
clip_hashes = fingerprinter.fingerprint_file("clip.wav")
match = index.query(clip_hashes)

if match:
    print(f"Match found: {match['track_id']} with score {match['score']} (offset: {match['offset_secs']}s)")
else:
    print("No match found.")

# Save index to disk
index.save("music.db")

# Load index from disk
loaded_index = wavio.PyIndex.load("music.db")
print(f"Loaded {loaded_index.track_count} tracks.")
```
