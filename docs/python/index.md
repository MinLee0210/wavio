# Python Guide

`wavio` provides Python bindings via [PyO3](https://pyo3.rs) — the fingerprinting runs in Rust at native speed while exposing a Pythonic API.

---

## Overview

The Python API has two classes:

| Class | Purpose |
|-------|---------|
| `PyFingerprinter` | Extracts fingerprints from WAV files |
| `PyIndex` | Stores and queries fingerprints |

```python
import wavio

# The two classes you need
fp = wavio.PyFingerprinter()
index = wavio.PyIndex()
```

## Key Features

- **GIL release** — fingerprinting runs without holding the Python GIL, so it's safe to use with threads
- **Native speed** — the DSP pipeline runs in compiled Rust, not Python
- **Simple types** — fingerprints are plain `list[tuple[int, float]]`, query results are `dict`

## Basic Workflow

```python
import wavio

fp = wavio.PyFingerprinter()
index = wavio.PyIndex()

# Fingerprint and index tracks
for song in ["song_a.wav", "song_b.wav", "song_c.wav"]:
    name = song.replace(".wav", "")
    hashes = fp.fingerprint_file(song)
    index.insert(name, hashes)
    print(f"  ✓ {name}: {len(hashes)} hashes")

# Query
result = index.query(fp.fingerprint_file("mystery_clip.wav"))
if result:
    print(f"It's {result['track_id']}! (score: {result['score']})")
```

---

[:octicons-arrow-right-24: API Reference](api.md) — full method signatures and parameter details

[:octicons-arrow-right-24: Examples](examples.md) — real-world usage patterns
