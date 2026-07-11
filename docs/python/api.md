# Python API Reference

## `PyFingerprinter`

Extracts audio fingerprints from WAV files.

```python
fingerprinter = wavio.PyFingerprinter()
```

### Methods

#### `fingerprint_file(path: str) -> list[tuple[int, float]]`

Loads a WAV file and returns a list of `(hash, anchor_time)` pairs.

- The GIL is released during computation — safe for multi-threaded use.
- Returns an empty list if the audio is too short or contains no detectable peaks.

```python
fp = wavio.PyFingerprinter()
hashes = fp.fingerprint_file("song.wav")
# [(14829304838, 0.023), (9923847123, 0.046), ...]

print(f"Got {len(hashes)} fingerprints")
```

**Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `path` | `str` | Path to a WAV file |

**Returns:** `list[tuple[int, float]]` — list of `(hash, anchor_time_seconds)` pairs.

**Raises:**

| Exception | When |
|-----------|------|
| `IOError` | File not found, or not a valid WAV file |
| `RuntimeError` | Spectrogram or peak extraction failed |

---

## `PyIndex`

In-memory fingerprint index for storing and querying tracks.

```python
index = wavio.PyIndex()
```

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `track_count` | `int` | Number of indexed tracks |
| `hash_count` | `int` | Total number of hash entries |

```python
index = wavio.PyIndex()
index.insert("song", hashes)
print(index.track_count)  # 1
print(index.hash_count)   # 4521
```

### Methods

#### `insert(track_id: str, fingerprints: list[tuple[int, float]]) -> None`

Adds a track's fingerprints to the index.

If the same `track_id` is inserted multiple times, hashes are appended (no deduplication).

```python
fp = wavio.PyFingerprinter()
hashes = fp.fingerprint_file("song.wav")

index = wavio.PyIndex()
index.insert("My Song", hashes)
```

**Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `track_id` | `str` | Name/identifier for this track |
| `fingerprints` | `list[tuple[int, float]]` | Fingerprints from `PyFingerprinter.fingerprint_file()` |

---

#### `query(fingerprints: list[tuple[int, float]]) -> dict | None`

Searches the index for the best-matching track.

Returns a dictionary on match, or `None` if no matching hashes are found.

```python
result = index.query(clip_hashes)

if result:
    print(result["track_id"])    # "My Song"
    print(result["score"])       # 312
    print(result["offset_secs"]) # 47.35
```

**Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `fingerprints` | `list[tuple[int, float]]` | Query fingerprints |

**Returns:** `dict | None`

| Key | Type | Description |
|-----|------|-------------|
| `track_id` | `str` | Name of the matched track |
| `score` | `int` | Number of time-aligned hash hits (higher = better) |
| `offset_secs` | `float` | Estimated position of the clip within the track (seconds) |

!!! tip "Score filtering"
    A score of 0 means no overlapping hashes. For production use, filter on a minimum threshold:
    ```python
    if result and result["score"] > 10:
        print("Confident match!")
    ```

---

#### `save(path: str) -> None`

Saves the index to a sled database directory on disk.

!!! note "Requires `persist` feature"
    The Python module must be built with `maturin develop --features python,persist`.

```python
index.save("./music.db")
```

**Raises:** `IOError` if the directory cannot be written.

---

#### `PyIndex.load(path: str) -> PyIndex`  *(static method)*

Loads a previously saved index from disk.

```python
index = wavio.PyIndex.load("./music.db")
print(f"Loaded {index.track_count} tracks")
```

**Raises:** `IOError` if the path does not exist or cannot be read.
