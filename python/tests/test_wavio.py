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
