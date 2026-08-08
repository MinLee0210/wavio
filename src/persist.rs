//! On-disk persistence for fingerprint indices using `redb`.
//!
//! This module provides `PersistentIndex` — a wrapper around `redb::Database`
//! that stores fingerprints persistently while maintaining the same query
//! interface as the in-memory `Index`.
//!
//! # Backend Status
//!
//! **Note:** This module has been migrated from `sled` to `redb`. `redb` is
//! an embedded key-value store written in pure, safe Rust. It offers
//! ACID transactions and B-tree storage layout, ensuring durability and
//! memory safety.
//!
//! # Disk Layout
//!
//! The persistent index uses four redb tables:
//! - `hashes`: Maps from `u64` hash to bincode-encoded `Vec<(u32, f32)>` (TrackId, anchor_time) pairs.
//! - `tracks_by_name`: Maps track name (&str) to track ID (u32).
//! - `tracks_by_id`: Maps track ID (u32) to track name (&str).
//! - `metadata`: Stores configuration and the next available TrackId.

use std::collections::HashMap;
use std::path::Path;

use redb::{Database, TableDefinition, ReadableTable, ReadableTableMetadata};

use crate::error::WavioError;
use crate::hash::Fingerprint;
use crate::index::{Index, IndexConfig, QueryResult, TrackId};

// Type alias for convenience
type WavioResult<T> = Result<T, WavioError>;

// Table Definitions
const HASHES: TableDefinition<u64, &[u8]> = TableDefinition::new("hashes");
const TRACKS_BY_NAME: TableDefinition<&str, u32> = TableDefinition::new("tracks_by_name");
const TRACKS_BY_ID: TableDefinition<u32, &str> = TableDefinition::new("tracks_by_id");
const METADATA: TableDefinition<&str, &[u8]> = TableDefinition::new("metadata");

// ---------------------------------------------------------------------------
// Persistent Index
// ---------------------------------------------------------------------------

/// On-disk persistent fingerprint index backed by `redb`.
///
/// Provides the same query interface as `Index` but stores data persistently.
#[derive(Debug)]
pub struct PersistentIndex {
    db: Database,
    config: IndexConfig,
}

impl PersistentIndex {
    /// Opens or creates a persistent index at the given path.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be opened or initialized.
    pub fn open<P: AsRef<Path>>(path: P) -> WavioResult<Self> {
        let path_ref = path.as_ref();
        let db = if path_ref.exists() {
            Database::open(path_ref).map_err(|e| WavioError::IoError(e.to_string()))?
        } else {
            if let Some(parent) = path_ref.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            Database::create(path_ref).map_err(|e| WavioError::IoError(e.to_string()))?
        };

        // Initialize tables by starting a write transaction
        let write_txn = db.begin_write().map_err(|e| WavioError::IoError(e.to_string()))?;
        let config = {
            // Open tables to ensure they are created
            let _hashes = write_txn.open_table(HASHES).map_err(|e| WavioError::IoError(e.to_string()))?;
            let _tracks_by_name = write_txn.open_table(TRACKS_BY_NAME).map_err(|e| WavioError::IoError(e.to_string()))?;
            let _tracks_by_id = write_txn.open_table(TRACKS_BY_ID).map_err(|e| WavioError::IoError(e.to_string()))?;

            let metadata = write_txn.open_table(METADATA).map_err(|e| WavioError::IoError(e.to_string()))?;
            if let Some(config_bytes) = metadata.get("config").map_err(|e| WavioError::IoError(e.to_string()))? {
                bincode::deserialize(config_bytes.value()).unwrap_or_default()
            } else {
                IndexConfig::default()
            }
        };
        write_txn.commit().map_err(|e| WavioError::IoError(e.to_string()))?;

        Ok(Self { db, config })
    }

    /// Returns the number of indexed tracks.
    #[must_use]
    pub fn track_count(&self) -> usize {
        let Ok(read_txn) = self.db.begin_read() else { return 0; };
        let Ok(t_id) = read_txn.open_table(TRACKS_BY_ID) else { return 0; };
        let Ok(len) = t_id.len() else { return 0; };
        len as usize
    }

    /// Returns the total number of hash entries across all tracks.
    #[must_use]
    pub fn hash_count(&self) -> usize {
        let Ok(read_txn) = self.db.begin_read() else { return 0; };
        let Ok(hashes) = read_txn.open_table(HASHES) else { return 0; };
        let Ok(iter) = hashes.iter() else { return 0; };
        let mut count = 0;
        for (_, val) in iter.flatten() {
            if let Ok(entries) = bincode::deserialize::<Vec<(TrackId, f32)>>(val.value()) {
                count += entries.len();
            }
        }
        count
    }

    /// Inserts a track's fingerprints into the index.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence operations fail.
    pub fn insert(&mut self, track_name: &str, fingerprints: &[Fingerprint]) -> WavioResult<()> {
        let write_txn = self.db.begin_write().map_err(|e| WavioError::IoError(e.to_string()))?;

        let track_id = {
            let mut metadata = write_txn.open_table(METADATA).map_err(|e| WavioError::IoError(e.to_string()))?;
            let mut tracks_by_name = write_txn.open_table(TRACKS_BY_NAME).map_err(|e| WavioError::IoError(e.to_string()))?;
            let mut tracks_by_id = write_txn.open_table(TRACKS_BY_ID).map_err(|e| WavioError::IoError(e.to_string()))?;

            let current_next_id: TrackId = metadata
                .get("next_id")
                .map_err(|e| WavioError::IoError(e.to_string()))?
                .and_then(|val| {
                    let bytes = val.value();
                    if bytes.len() == 4 {
                        Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                    } else {
                        None
                    }
                })
                .unwrap_or(0);

            if let Some(stored_id) = tracks_by_name.get(track_name).map_err(|e| WavioError::IoError(e.to_string()))? {
                stored_id.value()
            } else {
                let new_id = current_next_id;
                tracks_by_name.insert(track_name, new_id).map_err(|e| WavioError::IoError(e.to_string()))?;
                tracks_by_id.insert(new_id, track_name).map_err(|e| WavioError::IoError(e.to_string()))?;
                
                let next_id_bytes = (new_id + 1).to_le_bytes();
                metadata.insert("next_id", next_id_bytes.as_slice()).map_err(|e| WavioError::IoError(e.to_string()))?;
                new_id
            }
        };

        // Insert fingerprints into the hashes table
        {
            let mut hashes = write_txn.open_table(HASHES).map_err(|e| WavioError::IoError(e.to_string()))?;
            for fp in fingerprints {
                let key = fp.hash;
                let mut entries = if let Some(val) = hashes.get(key).map_err(|e| WavioError::IoError(e.to_string()))? {
                    bincode::deserialize(val.value()).unwrap_or_default()
                } else {
                    Vec::new()
                };

                entries.push((track_id, fp.anchor_time));
                let encoded = bincode::serialize(&entries).map_err(|e| WavioError::IndexError(e.to_string()))?;
                hashes.insert(key, encoded.as_slice()).map_err(|e| WavioError::IoError(e.to_string()))?;
            }
        }

        write_txn.commit().map_err(|e| WavioError::IoError(e.to_string()))?;
        Ok(())
    }

    /// Queries the index with a set of fingerprints and returns the
    /// best-matching track, if any.
    ///
    /// # Errors
    ///
    /// Returns `None` if no matching hashes are found or query fails.
    #[must_use]
    pub fn query(&self, fingerprints: &[Fingerprint]) -> Option<QueryResult> {
        if fingerprints.is_empty() {
            return None;
        }

        let read_txn = self.db.begin_read().ok()?;
        let hashes = read_txn.open_table(HASHES).ok()?;

        // Per-track histogram: track_id -> (offset_bin -> count)
        let mut histograms: HashMap<TrackId, HashMap<i64, u32>> = HashMap::new();

        for fp in fingerprints {
            let key = fp.hash;
            if let Ok(Some(val)) = hashes.get(key) {
                if let Ok(entries) = bincode::deserialize::<Vec<(TrackId, f32)>>(val.value()) {
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

        // Find the track and bin with the highest count
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

        #[allow(clippy::cast_precision_loss)]
        let confidence = best_score as f32 / fingerprints.len() as f32;

        best_track.and_then(|tid| {
            self.track_name_with_txn(&read_txn, tid)
                .map(|name| QueryResult {
                    track_id: name,
                    score: best_score,
                    offset_secs: self.bin_to_offset(best_bin),
                    confidence,
                })
        })
    }

    /// Persists pending updates to disk. Since `redb` commits are durable on write,
    /// this is a no-op that returns `Ok(())`.
    ///
    /// # Errors
    ///
    /// Always returns `Ok(())`.
    pub fn flush(&mut self) -> WavioResult<()> {
        Ok(())
    }

    /// Retrieves the track name for a given TrackId using an active transaction.
    fn track_name_with_txn(&self, read_txn: &redb::ReadTransaction, track_id: TrackId) -> Option<String> {
        let tracks_by_id = read_txn.open_table(TRACKS_BY_ID).ok()?;
        let val = tracks_by_id.get(track_id).ok()??;
        Some(val.value().to_string())
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
    /// # Errors
    ///
    /// Returns an error if database read fails.
    pub fn load_into_memory(&self) -> WavioResult<Index> {
        let mut in_memory_index = Index::new(self.config.clone());
        let read_txn = self.db.begin_read().map_err(|e| WavioError::IoError(e.to_string()))?;
        let hashes = read_txn.open_table(HASHES).map_err(|e| WavioError::IoError(e.to_string()))?;

        let iter = hashes.iter().map_err(|e| WavioError::IoError(e.to_string()))?;
        for result in iter {
            let (hash_guard, val_guard) = result.map_err(|e| WavioError::IoError(e.to_string()))?;
            let hash = hash_guard.value();
            let val = val_guard.value();

            if let Ok(entries) = bincode::deserialize::<Vec<(TrackId, f32)>>(val) {
                for (track_id, anchor_time) in entries {
                    if let Some(track_name) = self.track_name_with_txn(&read_txn, track_id) {
                        let fp = Fingerprint::new(hash, anchor_time);
                        in_memory_index.insert(&track_name, &[fp]);
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
        Fingerprint::new(hash, anchor_time)
    }

    fn cleanup(path: &str) {
        let p = Path::new(path);
        if p.is_file() {
            let _ = fs::remove_file(p);
        } else if p.is_dir() {
            let _ = fs::remove_dir_all(p);
        }
    }

    #[test]
    fn test_persistent_index_insert_and_query() {
        let tmp_dir = "target/wavio_test_persist_1";
        cleanup(tmp_dir);

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

        cleanup(tmp_dir);
    }

    #[test]
    fn test_persistent_index_multiple_tracks() {
        let tmp_dir = "target/wavio_test_persist_multi";
        cleanup(tmp_dir);

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

        cleanup(tmp_dir);
    }

    #[test]
    fn test_persistent_and_memory_index_equivalence() {
        let tmp_dir = "target/wavio_test_equivalence";
        cleanup(tmp_dir);

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
                .save_to_disk(tmp_dir)
                .expect("failed to save to disk");
        }

        // Load from disk and verify equivalence
        {
            let loaded_index = Index::load_from_disk(tmp_dir).expect("failed to load from disk");

            let result = loaded_index.query(&test_fps);
            assert!(result.is_some());
            let qr = result.unwrap();
            assert_eq!(qr.track_id, "test_song");
            assert_eq!(qr.score, 4);
        }

        cleanup(tmp_dir);
    }
}
