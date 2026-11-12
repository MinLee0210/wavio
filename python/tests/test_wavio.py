import wavio
import tempfile
import os
import pytest

# Note: creating a real wav file requires extra imports or fixed bites.
# We will just write a very simple valid wav logic or just test index operations
# For testing fingerprinter, we need an actual wav. We will skip fingerprinter test
# without a valid wav, or test an error exception.

def test_fingerprint_file_missing():
    fingerprinter = wavio.PyFingerprinter()
    with pytest.raises(Exception):
        fingerprinter.fingerprint_file("../../data/sample.wav")

def test_index_operations():
    index = wavio.PyIndex()
    assert index.track_count == 0
    assert index.hash_count == 0

    hashes1 = [(12345, 0.1), (23456, 0.2)]
    index.insert("Track A", hashes1)
    
    assert index.track_count == 1
    assert index.hash_count == 2

    hashes2 = [(34567, 0.1)]
    index.insert("Track B", hashes2)
    assert index.track_count == 2
    assert index.hash_count == 3

    # Query
    res = index.query([(12345, 0.5), (23456, 0.6)])
    assert res is not None
    assert res['track_id'] == "Track A"
    assert res['score'] == 2

    # Query no match
    res_none = index.query([(99999, 1.0)])
    assert res_none is None

def test_persist_operations():
    with tempfile.TemporaryDirectory() as temp_dir:
        db_path = os.path.join(temp_dir, "test.db")
        
        index = wavio.PyIndex()
        index.insert("Song 1", [(111, 0.1)])
        index.save(db_path)

        # Load back
        index_loaded = wavio.PyIndex.load(db_path)
        assert index_loaded.track_count == 1
        assert index_loaded.hash_count == 1


# ---------------------------------------------------------------------------
# In-memory audio, hashing modes, streaming, persistent index
# ---------------------------------------------------------------------------

import math
import random
from array import array

SR = 22050


def synth(seed, secs):
    """A random melody: notes of 0.2-0.6 s from a per-track scale, each with its
    own harmonic balance, plus occasional percussive noise bursts.

    (Steady tones are a bad fixture: they produce very few distinct peaks, and
    ratio-based triplet hashes need peaks that vary over time, as music does.)
    """
    rng = random.Random(seed)
    root = rng.uniform(110.0, 330.0)
    scale = [root * 2 ** (round(rng.uniform(0, 36)) / 12) for _ in range(7)]
    n = int(secs * SR)
    out = array("f", bytes(4 * n))
    t0 = 0
    while t0 < n:
        length = min(int(rng.uniform(0.2, 0.6) * SR), n - t0)
        f = rng.choice(scale)
        harm = [rng.uniform(0.1, 1.0) for _ in range(4)]
        drum = rng.random() < 0.25
        for i in range(length):
            t = i / SR
            env = (1 - i / length) ** 0.7 * min(i / 200, 1.0)
            s = sum(a * math.sin(math.tau * f * (h + 1) * t) for h, a in enumerate(harm))
            if drum and i < length // 4:
                s += 3.0 * (rng.random() - 0.5)
            out[t0 + i] = 0.12 * env * s
        t0 += length
    return out


@pytest.fixture(scope="module")
def track():
    return synth(1, 14.0)


@pytest.fixture(scope="module")
def other_track():
    return synth(5, 14.0)


def test_fingerprint_samples_accepts_buffers_and_lists(track):
    fp = wavio.PyFingerprinter()
    from_buffer = fp.fingerprint_samples(track)
    from_list = fp.fingerprint_samples(list(track))
    assert len(from_buffer) > 100
    assert from_buffer == from_list
    assert fp.fingerprint_samples(track) == from_buffer  # deterministic
    assert all(isinstance(h, int) and isinstance(t, float) for h, t in from_buffer[:5])


def test_fingerprint_samples_rejects_garbage():
    fp = wavio.PyFingerprinter()
    with pytest.raises(TypeError):
        fp.fingerprint_samples("not audio")
    with pytest.raises(Exception):
        fp.fingerprint_samples([0.0] * 10)  # shorter than one FFT window


def test_modes_and_signatures(track):
    pairwise = wavio.PyFingerprinter()
    triplet = wavio.PyFingerprinter(mode="triplet")
    hybrid = wavio.PyFingerprinter(mode="hybrid")
    assert (pairwise.mode, triplet.mode, hybrid.mode) == ("pairwise", "triplet", "hybrid")
    assert len({pairwise.signature, triplet.signature, hybrid.signature}) == 3

    n_pair = len(pairwise.fingerprint_samples(track))
    n_trip = len(triplet.fingerprint_samples(track))
    n_hyb = len(hybrid.fingerprint_samples(track))
    assert n_trip > 0
    assert n_hyb > n_pair and n_hyb > n_trip

    with pytest.raises(ValueError):
        wavio.PyFingerprinter(mode="nonsense")


@pytest.mark.parametrize("mode", ["pairwise", "triplet", "hybrid"])
def test_identify_a_clip_in_every_mode(mode, track, other_track):
    fp = wavio.PyFingerprinter(mode=mode)
    index = wavio.PyIndex()
    index.insert("song", fp.fingerprint_samples(track))
    index.insert("other", fp.fingerprint_samples(other_track))

    clip = track[3 * SR : 8 * SR]
    match = index.query(fp.fingerprint_samples(clip))
    assert match is not None
    assert match["track_id"] == "song"
    assert abs(match["offset_secs"] - 3.0) < 0.25
    assert 0.0 < match["confidence"] <= 1.0
    assert 0.0 <= match["margin"] <= 1.0
    assert match["speed"] == 1.0
    assert match["weighted_score"] == match["score"]  # no idf


def test_query_options_idf_and_scaled(track):
    fp = wavio.PyFingerprinter(mode="hybrid")
    index = wavio.PyIndex()
    hashes = fp.fingerprint_samples(track)
    index.insert("song", hashes)
    assert index.contains_track("song") and not index.contains_track("nope")

    # A duplicate makes every hash occur twice, so rarity weights drop below 1.
    index.insert("duplicate", hashes)

    clip = fp.fingerprint_samples(track[2 * SR : 9 * SR])
    plain = index.query(clip)
    weighted = index.query(clip, idf=True)
    assert weighted["track_id"] in ("song", "duplicate")
    assert weighted["score"] == plain["score"]  # raw count is unchanged
    assert plain["weighted_score"] == plain["score"]
    assert 0 < weighted["weighted_score"] < weighted["score"]

    scaled = index.query_scaled(clip, max_deviation=0.05, step=0.01)
    assert scaled["track_id"] in ("song", "duplicate")
    assert abs(scaled["speed"] - 1.0) < 0.011

    top = index.query_topn(clip, 3, idf=True)
    assert {r["track_id"] for r in top} == {"song", "duplicate"}


def test_index_signature_round_trip():
    fp = wavio.PyFingerprinter(mode="triplet")
    index = wavio.PyIndex()
    assert index.signature is None
    index.set_signature(fp.signature)
    assert index.signature == fp.signature


def test_streaming_matches_batch(track):
    fp = wavio.PyFingerprinter()
    batch = {h for h, _ in fp.fingerprint_samples(track)}

    streamer = wavio.PyStreamingFingerprinter(fp, block_duration_secs=5.0, overlap_secs=1.0)
    streamed = []
    chunk = 2048
    for i in range(0, len(track), chunk):
        streamed += streamer.push(track[i : i + chunk])
    streamed += streamer.finish()

    got = {h for h, _ in streamed}
    jaccard = len(batch & got) / len(batch | got)
    assert jaccard > 0.95, jaccard
    times = [t for _, t in streamed]
    assert max(times) > 10.0  # anchor times are absolute stream times


def test_streaming_rejects_bad_overlap():
    fp = wavio.PyFingerprinter()
    with pytest.raises(ValueError):
        wavio.PyStreamingFingerprinter(fp, block_duration_secs=10.0, overlap_secs=0.1)


def test_persistent_index_round_trip(tmp_path, track, other_track):
    path = str(tmp_path / "lib.redb")
    fp = wavio.PyFingerprinter(mode="hybrid")

    db = wavio.PyPersistentIndex(path)
    assert db.signature is None
    db.ensure_fingerprinter(fp)
    assert db.signature == fp.signature
    db.insert_many(
        [
            ("song", fp.fingerprint_samples(track)),
            ("other", fp.fingerprint_samples(other_track)),
        ]
    )
    assert db.track_count == 2 and db.hash_count > 0
    del db

    db = wavio.PyPersistentIndex(path)  # reopened: everything persisted
    assert db.track_count == 2 and db.contains_track("song")
    db.check_fingerprinter(fp)

    clip = fp.fingerprint_samples(track[4 * SR : 10 * SR])
    match = db.query(clip)
    assert match["track_id"] == "song"
    assert abs(match["offset_secs"] - 4.0) < 0.25
    assert db.query_topn(clip, 2)[0]["track_id"] == "song"
    assert db.query_scaled(clip)["track_id"] == "song"
    assert db.query(clip, idf=True)["track_id"] == "song"

    assert db.remove_track("song") is True
    assert db.remove_track("song") is False
    assert db.track_count == 1
    assert db.query(clip) is None or db.query(clip)["track_id"] != "song"


def test_persistent_index_refuses_mismatched_settings(tmp_path):
    path = str(tmp_path / "lib.redb")
    db = wavio.PyPersistentIndex(path)
    db.ensure_fingerprinter(wavio.PyFingerprinter())

    other = wavio.PyFingerprinter(mode="triplet")
    with pytest.raises(ValueError, match="Incompatible"):
        db.ensure_fingerprinter(other)
    with pytest.raises(ValueError, match="Incompatible"):
        db.check_fingerprinter(other)
    db.check_fingerprinter(wavio.PyFingerprinter())  # same settings: fine


def test_check_fingerprinter_never_writes(tmp_path):
    db = wavio.PyPersistentIndex(str(tmp_path / "lib.redb"))
    db.check_fingerprinter(wavio.PyFingerprinter(mode="hybrid"))
    assert db.signature is None


def test_in_memory_index_save_and_load_keeps_signature(tmp_path, track):
    fp = wavio.PyFingerprinter()
    index = wavio.PyIndex()
    index.set_signature(fp.signature)
    index.insert("song", fp.fingerprint_samples(track))
    path = str(tmp_path / "saved.redb")
    index.save(path)

    loaded = wavio.PyIndex.load(path)
    assert loaded.signature == fp.signature
    assert loaded.track_count == 1
    assert loaded.query(fp.fingerprint_samples(track[: 6 * SR]))["track_id"] == "song"

    # ...and the same file opens as a PyPersistentIndex.
    db = wavio.PyPersistentIndex(path)
    db.check_fingerprinter(fp)
    with pytest.raises(ValueError):
        db.check_fingerprinter(wavio.PyFingerprinter(mode="hybrid"))
