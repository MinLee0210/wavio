//! On-disk persistence for fingerprint indices using `sled`.
//!
//! This module provides `PersistentIndex` — a wrapper around `sled::Db`
//! that stores fingerprints persistently while maintaining the same query
//! interface as the in-memory `Index`.
//!
//! # Disk Layout
//!
//! The persistent index uses three sled trees:
//! - `hashes`: Maps from `u64` hash (as big-endian bytes) to bincode-encoded
//!   `Vec<(u32, f32)>` (TrackId, anchor_time) pairs.
//! - `tracks_by_name`: Maps track name to track ID.
//! - `tracks_by_id`: Maps track ID (as bytes) to track name.
//! - `metadata`: Stores configuration and the next available TrackId.

use std::collections::HashMap;
use std::path::Path;

use crate::error::WavioError;
use crate::hash::Fingerprint;
use crate::index::{Index, IndexConfig, QueryResult, TrackId};

// Type alias for convenience
type WavioResult<T> = Result<T, WavioError>;

// ---------------------------------------------------------------------------
// Persistent Index
// ---------------------------------------------------------------------------

/// On-disk persistent fingerprint index backed by `sled`.
///
/// Provides the same query interface as `Index` but stores data persistently.
#[derive(Debug)]
pub struct PersistentIndex {
    db: sled::Db,
    hashes_tree: sled::Tree,
    tracks_by_name: sled::Tree,  // name -> id
    tracks_by_id: sled::Tree,    // id -> name
    metadata_tree: sled::Tree,
    config: IndexConfig,
}

impl PersistentIndex {
    /// Opens or creates a persistent index at the given path.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be opened.
    pub fn open<P: AsRef<Path>>(path: P) -> WavioResult<Self> {
        let db = sled::open(path).map_err(|e| WavioError::IoError(e.to_string()))?;

        let hashes_tree = db.open_tree("hashes").map_err(|e| WavioError::IoError(e.to_string()))?;

        let tracks_by_name = db.open_tree("tracks_by_name").map_err(|e| WavioError::IoError(e.to_string()))?;

        let tracks_by_id = db.open_tree("tracks_by_id").map_err(|e| WavioError::IoError(e.to_string()))?;

        let metadata_tree = db.open_tree("metadata").map_err(|e| WavioError::IoError(e.to_string()))?;

        // Load configuration from metadata, or use default.
        let config = if let Some(config_bytes) = metadata_tree.get("config").ok().flatten() {
            bincode::deserialize(&config_bytes).unwrap_or_default()
        } else {
            IndexConfig::default()
        };

        Ok(Self {
            db,
            hashes_tree,
            tracks_by_name,
            tracks_by_id,
            metadata_tree,
            config,
        })
    }

    /// Returns the number of indexed tracks.
    #[must_use]
    pub fn track_count(&self) -> usize {
        self.tracks_by_id.iter().count()
    }

    /// Returns the total number of hash entries across all tracks.
    #[must_use]
    pub fn hash_count(&self) -> usize {
        self.hashes_tree
            .iter()
            .filter_map(Result::ok)
            .map(|(_, val)| {
                bincode::deserialize::<Vec<(TrackId, f32)>>(&val)
                    .map(|v| v.len())
                    .unwrap_or(0)
            })
            .sum()
    }

    /// Inserts a track's fingerprints into the index.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence operations fail.
    pub fn insert(&mut self, track_name: &str, fingerprints: &[Fingerprint]) -> WavioResult<()> {
        // Allocate a new TrackId if this is a new track.
        let next_id_key = b"next_id";
        let current_next_id: TrackId = self
            .metadata_tree
            .get(next_id_key)
            .ok()
            .flatten()
            .and_then(|bytes| {
                if bytes.len() == 4 {
                    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                } else {
                    None
                }
            })
            .unwrap_or(0);

        let track_id = if let Some(stored_id_bytes) = self
            .tracks_by_name
            .get(track_name)
            .ok()
            .flatten()
        {
            u32::from_le_bytes([stored_id_bytes[0], stored_id_bytes[1], stored_id_bytes[2], stored_id_bytes[3]])
        } else {
            let new_id = current_next_id;
            // Store bidirectional mapping
            self.tracks_by_name
                .insert(track_name, new_id.to_le_bytes().to_vec())
                .map_err(|e| WavioError::IoError(e.to_string()))?;

            self.tracks_by_id
                .insert(new_id.to_le_bytes().to_vec(), track_name)
                .map_err(|e| WavioError::IoError(e.to_string()))?;

            self.metadata_tree
                .insert(next_id_key, (new_id + 1).to_le_bytes().to_vec())
                .map_err(|e| WavioError::IoError(e.to_string()))?;

            new_id
        };

        // Insert fingerprints into the hash table.
        for fp in fingerprints {
            let key = fp.hash.to_be_bytes();

            let mut entries = if let Some(val) = self
                .hashes_tree
                .get(&key)
                .map_err(|e| WavioError::IoError(e.to_string()))?
            {
                bincode::deserialize(&val).unwrap_or_default()
            } else {
                Vec::new()
            };

            entries.push((track_id, fp.anchor_time));
            let encoded = bincode::serialize(&entries).map_err(|e| WavioError::IndexError(e.to_string()))?;

            self.hashes_tree
                .insert(&key, encoded)
                .map_err(|e| WavioError::IoError(e.to_string()))?;
        }

        Ok(())
    }

    /// Queries the index with a set of fingerprints and returns the
    /// best-matching track, if any.
    ///
    /// Uses the same algorithm as the in-memory `Index`:
    /// 1. For each query fingerprint, look up matching entries.
    /// 2. Compute time offsets and quantize into histogram bins.
    /// 3. Return the track with the highest histogram peak.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence operations fail.
    #[must_use]
    pub fn query(&self, fingerprints: &[Fingerprint]) -> Option<QueryResult> {
        if fingerprints.is_empty() {
            return None;
        }

        // Per-track histogram: track_id -> (offset_bin -> count)
        let mut histograms: HashMap<TrackId, HashMap<i64, u32>> = HashMap::new();

        for fp in fingerprints {
            let key = fp.hash.to_be_bytes();
            if let Ok(Some(val)) = self.hashes_tree.get(&key) {
                if let Ok(entries) = bincode::deserialize::<Vec<(TrackId, f32)>>(&val) {
                    for (track_id, db_time) in entries {
                        let offset = db_time - fp.anchor_time;
                        let bin = self.offset_to_bin(offset);

                        *histograms
                            .entry(track_id)
                            .or_default()
                            .entry(bin)
                            .or_insert(0) += 1;
                    }
                }
            }
        }

        // Find the track and bin with the highest count.
        let mut best_track: Option<TrackId> = None;
        let mut best_score: u32 = 0;
        let mut best_bin: i64 = 0;

        for (track_id, bins) in &histograms {
            for (&bin, &count) in bins {
                if count > best_score {
                    best_score = count;
                    best_bin = bin;
                    best_track = Some(*track_id);
                }
            }
        }

        best_track.and_then(|tid| {
            self.track_name(tid)
                .map(|name| QueryResult {
                    track_id: name,
                    score: best_score,
                    offset_secs: self.bin_to_offset(best_bin),
                })
        })
    }

    /// Persists pending updates to disk. Normally happens automatically,
    /// but you can call this to ensure durability.
    ///
    /// # Errors
    ///
    /// Returns an error if the flush operation fails.
    pub fn flush(&mut self) -> WavioResult<()> {
        self.db.flush().map_err(|e| WavioError::IoError(e.to_string()))?;
        Ok(())
    }

    /// Retrieves the track name for a given TrackId.
    #[must_use]
    fn track_name(&self, track_id: TrackId) -> Option<String> {
        self.tracks_by_id
            .get(track_id.to_le_bytes().to_vec())
            .ok()
            .flatten()
            .and_then(|v| String::from_utf8(v.to_vec()).ok())
    }

    /// Quantizes a time offset (seconds) into a histogram bin index.
    #[allow(clippy::cast_possible_truncation)]
    fn offset_to_bin(&self, offset: f32) -> i64 {
        (offset / self.config.offset_bin_size).round() as i64
    }

    /// Converts a histogram bin index back to a time offset (seconds).
    #[allow(clippy::cast_precision_loss)]
    fn bin_to_offset(&self, bin: i64) -> f32 {
        bin as f32 * self.config.offset_bin_size
    }

    /// Loads the persistent index into memory as an in-memory `Index`.
    ///
    /// This enables the hybrid approach: load from disk once, query in memory.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence operations fail.
    pub fn load_into_memory(&self) -> WavioResult<Index> {
        let mut in_memory_index = Index::new(self.config.clone());

        // Iterate over all hash entries and load them into memory
        for result in self.hashes_tree.iter() {
            let (hash_bytes, val) = result.map_err(|e| WavioError::IoError(e.to_string()))?;

            if hash_bytes.len() == 8 {
                let hash = u64::from_be_bytes([
                    hash_bytes[0], hash_bytes[1], hash_bytes[2], hash_bytes[3],
                    hash_bytes[4], hash_bytes[5], hash_bytes[6], hash_bytes[7],
                ]);

                if let Ok(entries) = bincode::deserialize::<Vec<(TrackId, f32)>>(&val) {
                    for (track_id, anchor_time) in entries {
                        // Look up track name by ID
                        if let Some(track_name) = self.track_name(track_id) {
                            let fp = Fingerprint { hash, anchor_time };
                            in_memory_index.insert(&track_name, &[fp]);
                        }
                    }
                }
            }
        }

        Ok(in_memory_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fp(hash: u64, anchor_time: f32) -> Fingerprint {
        Fingerprint { hash, anchor_time }
    }

    #[test]
    fn test_persistent_index_insert_and_query() {
        let tmp_dir = "/tmp/wavio_test_persist_1";
        let _ = fs::remove_dir_all(tmp_dir);

        {
            let mut index = PersistentIndex::open(tmp_dir).expect("failed to open db");
            let track_fps = vec![
                fp(1000, 0.0),
                fp(2000, 0.1),
                fp(3000, 0.2),
                fp(4000, 0.3),
                fp(5000, 0.4),
            ];
            index.insert("song_a", &track_fps).expect("failed to insert");
            index.flush().expect("failed to flush");

            let result = index.query(&track_fps);
            assert!(result.is_some());
            let qr = result.unwrap();
            assert_eq!(qr.track_id, "song_a");
            assert_eq!(qr.score, 5);
        }

        // Reopen and verify data persisted.
        {
            let index = PersistentIndex::open(tmp_dir).expect("failed to reopen db");
            assert_eq!(index.track_count(), 1);
            assert_eq!(index.hash_count(), 5);

            let track_fps = vec![
                fp(1000, 0.0),
                fp(2000, 0.1),
                fp(3000, 0.2),
                fp(4000, 0.3),
                fp(5000, 0.4),
            ];
            let result = index.query(&track_fps);
            assert!(result.is_some());
            let qr = result.unwrap();
            assert_eq!(qr.track_id, "song_a");
            assert_eq!(qr.score, 5);
        }

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn test_persistent_index_multiple_tracks() {
        let tmp_dir = "/tmp/wavio_test_persist_multi";
        let _ = fs::remove_dir_all(tmp_dir);

        // Insert multiple tracks and verify persistence
        {
            let mut index = PersistentIndex::open(tmp_dir).expect("failed to open db");

            for track_num in 0..3_u64 {
                let fps: Vec<Fingerprint> = (0..10)
                    .map(|i| fp(track_num * 10_000 + i, i as f32 * 0.05))
                    .collect();
                let track_name = format!("track_{}", track_num);
                index.insert(&track_name, &fps).expect("failed to insert");
            }
            index.flush().expect("failed to flush");
        }

        // Reopen and verify all tracks are still there
        {
            let index = PersistentIndex::open(tmp_dir).expect("failed to reopen db");
            assert_eq!(index.track_count(), 3);
            assert_eq!(index.hash_count(), 30);

            // Query track 1
            let track1_fps: Vec<Fingerprint> = (0..10)
                .map(|i| fp(10_000 + i, i as f32 * 0.05))
                .collect();
            let result = index
                .query(&track1_fps)
                .expect("expected query result");
            assert_eq!(result.track_id, "track_1");
            assert_eq!(result.score, 10);
        }

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn test_persistent_and_memory_index_equivalence() {
        let tmp_dir = "/tmp/wavio_test_equivalence";
        let _ = fs::remove_dir_all(tmp_dir);

        let test_fps = vec![
            fp(1001, 0.0),
            fp(2002, 0.1),
            fp(3003, 0.2),
            fp(4004, 0.3),
        ];

        // Create in-memory index and save to disk
        {
            let mut mem_index = Index::default();
            mem_index.insert("test_song", &test_fps);

            mem_index
                .save_to_disk(&tmp_dir)
                .expect("failed to save to disk");
        }

        // Load from disk and verify equivalence
        {
            let loaded_index = Index::load_from_disk(&tmp_dir).expect("failed to load from disk");

            let result = loaded_index.query(&test_fps);
            assert!(result.is_some());
            let qr = result.unwrap();
            assert_eq!(qr.track_id, "test_song");
            assert_eq!(qr.score, 4);
        }

        let _ = fs::remove_dir_all(tmp_dir);
    }
}
