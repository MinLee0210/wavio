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

## Hashing modes

```python
wavio.PyFingerprinter()                 # "pairwise": best in noise (default)
wavio.PyFingerprinter(mode="triplet")   # survives speed/pitch changes, weak in noise
wavio.PyFingerprinter(mode="hybrid")    # both; use when speed/pitch may change (~1.5x index size)
```

Hashes from different modes (or different settings) never match each other. `fp.signature` describes
every setting that affects the hashes.

## Fingerprinting audio you already have in memory

```python
import numpy as np

samples = np.asarray(audio, dtype=np.float32)       # mono, 22,050 Hz; an array('f') or list also works
hashes = fp.fingerprint_samples(samples)            # no per-sample Python objects, GIL released
```

## Streaming

```python
streamer = wavio.PyStreamingFingerprinter(fp, block_duration_secs=10.0, overlap_secs=1.0)
hashes = []
for chunk in microphone_chunks():                   # any float32 buffers, any sizes
    hashes += streamer.push(chunk)                  # anchor times are absolute stream times
hashes += streamer.finish()
```

## An on-disk index that checks itself

```python
db = wavio.PyPersistentIndex("library.redb")
db.ensure_fingerprinter(fp)           # records the settings; ValueError if they differ later
db.insert_many([("song_a", hashes_a), ("song_b", hashes_b)])    # one transaction

match = db.query(clip_hashes)         # answered from disk; the GIL is released
db.remove_track("song_a")
```

Queries return dicts with `track_id`, `score`, `offset_secs`, `confidence`, `margin` (lead over the
runner-up), `speed` and `weighted_score`. Options on every query method:

* `idf=True` — weight hashes by rarity (helps triplet/hybrid in noise).
* `query_scaled(hashes, max_deviation=0.08, step=0.01)` — also searches playback-speed hypotheses and
  reports the estimated `speed` (e.g. `1.06`).

## Tests

```bash
pip install maturin pytest
maturin develop --manifest-path ../Cargo.toml --features python,persist,parallel
python -m pytest tests
```
