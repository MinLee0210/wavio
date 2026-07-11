# Python Examples

## Music Library Indexer

Index an entire folder of WAV files and identify clips against it.

```python
import os
import wavio

def build_index(music_dir: str, db_path: str):
    """Index all WAV files in a directory."""
    fp = wavio.PyFingerprinter()
    index = wavio.PyIndex()

    wav_files = [f for f in os.listdir(music_dir) if f.endswith(".wav")]
    print(f"Indexing {len(wav_files)} files...")

    for filename in wav_files:
        path = os.path.join(music_dir, filename)
        track_id = os.path.splitext(filename)[0]
        try:
            hashes = fp.fingerprint_file(path)
            index.insert(track_id, hashes)
            print(f"  ✓ {track_id}: {len(hashes)} hashes")
        except IOError as e:
            print(f"  ✗ {track_id}: {e}")

    index.save(db_path)
    print(f"\nSaved {index.track_count} tracks to {db_path}")


def identify(clip_path: str, db_path: str, min_score: int = 10):
    """Identify a clip against a saved database."""
    fp = wavio.PyFingerprinter()
    index = wavio.PyIndex.load(db_path)

    hashes = fp.fingerprint_file(clip_path)
    result = index.query(hashes)

    if result and result["score"] >= min_score:
        print(f"Match: {result['track_id']}")
        print(f"Offset: {result['offset_secs']:.1f}s  (score: {result['score']})")
    else:
        print("No confident match found.")


# Usage
build_index("./music/", "./music.db")
identify("./clip.wav", "./music.db")
```

---

## Batch Fingerprinting with Threading

The GIL is released during fingerprinting, so you can safely use threads:

```python
import wavio
from concurrent.futures import ThreadPoolExecutor

fp = wavio.PyFingerprinter()
files = ["song_a.wav", "song_b.wav", "song_c.wav", "song_d.wav"]

def process(path):
    name = path.replace(".wav", "")
    hashes = fp.fingerprint_file(path)  # GIL released here
    return name, hashes

with ThreadPoolExecutor(max_workers=4) as pool:
    results = list(pool.map(process, files))

index = wavio.PyIndex()
for name, hashes in results:
    index.insert(name, hashes)

print(f"Indexed {index.track_count} tracks")
```

---

## Query with Confidence Threshold

In production, always filter on score to avoid false positives:

```python
import wavio

def identify(clip_path: str, db_path: str):
    fp = wavio.PyFingerprinter()
    index = wavio.PyIndex.load(db_path)
    hashes = fp.fingerprint_file(clip_path)
    result = index.query(hashes)

    if result is None:
        return {"status": "no_match"}

    if result["score"] < 10:
        return {"status": "low_confidence", "candidate": result["track_id"]}

    return {
        "status": "match",
        "track": result["track_id"],
        "score": result["score"],
        "position": f"{result['offset_secs']:.1f}s",
    }
```

---

## Django / Flask Integration

```python
# views.py (Django example)
import wavio
import tempfile

# Load index once at startup
INDEX = wavio.PyIndex.load("/data/music.db")
FP = wavio.PyFingerprinter()

def identify_upload(request):
    audio_file = request.FILES["clip"]

    with tempfile.NamedTemporaryFile(suffix=".wav", delete=True) as tmp:
        for chunk in audio_file.chunks():
            tmp.write(chunk)
        tmp.flush()

        hashes = FP.fingerprint_file(tmp.name)
        result = INDEX.query(hashes)

    if result and result["score"] > 10:
        return JsonResponse({"track": result["track_id"], "score": result["score"]})
    return JsonResponse({"track": None}, status=404)
```
