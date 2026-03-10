//! Fingerprint indexing and query matching.
//!
//! This module provides in-memory storage for fingerprint hashes,
//! along with the query engine for audio identification.
//!
//! The index maps each hash to a list of `(TrackId, anchor_time)` pairs.
//! Queries build per-track time-offset histograms and return the track
//! with the highest histogram peak.

use std::collections::HashMap;

use crate::hash::Fingerprint;

// ---------------------------------------------------------------------------
// TrackId mapping
// ---------------------------------------------------------------------------

/// Internal numeric identifier for an indexed track.
pub type TrackId = u32;

/// Bidirectional mapping between track names and numeric IDs.
#[derive(Debug, Clone, Default)]
struct TrackMap {
    name_to_id: HashMap<String, TrackId>,
    id_to_name: HashMap<TrackId, String>,
    next_id: TrackId,
}

impl TrackMap {
    /// Returns the ID for `name`, inserting a new mapping if needed.
    fn get_or_insert(&mut self, name: &str) -> TrackId {
        if let Some(&id) = self.name_to_id.get(name) {
            return id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.name_to_id.insert(name.to_string(), id);
        self.id_to_name.insert(id, name.to_string());
        id
    }

    /// Looks up the name for a given `TrackId`.
    fn name(&self, id: TrackId) -> Option<&str> {
        self.id_to_name.get(&id).map(String::as_str)
    }
}

// ---------------------------------------------------------------------------
// Query result
// ---------------------------------------------------------------------------

/// The result of a fingerprint query against the index.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    /// Name of the best-matching track.
    pub track_id: String,
    /// Number of time-aligned hash hits (histogram peak height).
    pub score: u32,
    /// Estimated time offset (seconds) between the query clip and the
    /// indexed track. Positive means the query starts later in the track.
    pub offset_secs: f32,
}

// ---------------------------------------------------------------------------
// Index configuration
// ---------------------------------------------------------------------------

/// Configuration for the in-memory index.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "persist", derive(serde::Serialize, serde::Deserialize))]
pub struct IndexConfig {
    /// Bin width (seconds) for the time-offset histogram.
    /// Smaller values give finer offset resolution but spread hits
    /// across more bins, potentially reducing the peak height.
    pub offset_bin_size: f32,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            offset_bin_size: 0.05, // 50 ms
        }
    }
}

// ---------------------------------------------------------------------------
// In-memory index
// ---------------------------------------------------------------------------

/// In-memory fingerprint index.
///
/// Stores a mapping from hash values to `(TrackId, anchor_time)` entries.
/// Supports insertion of new tracks and querying against stored fingerprints.
#[derive(Debug, Clone)]
pub struct Index {
    /// hash -> list of (`track_id`, `anchor_time`)
    table: HashMap<u64, Vec<(TrackId, f32)>>,
    tracks: TrackMap,
    config: IndexConfig,
}

impl Default for Index {
    fn default() -> Self {
        Self::new(IndexConfig::default())
    }
}

impl Index {
    /// Creates a new empty index with the given configuration.
    #[must_use]
    pub fn new(config: IndexConfig) -> Self {
        Self {
            table: HashMap::new(),
            tracks: TrackMap::default(),
            config,
        }
    }

    /// Returns the number of indexed tracks.
    #[must_use]
    pub fn track_count(&self) -> usize {
        self.tracks.name_to_id.len()
    }

    /// Returns the total number of hash entries across all tracks.
    #[must_use]
    pub fn hash_count(&self) -> usize {
        self.table.values().map(Vec::len).sum()
    }

    /// Inserts a track's fingerprints into the index.
    ///
    /// If a track with the same name already exists, additional
    /// fingerprints are appended (no deduplication).
    pub fn insert(&mut self, track_name: &str, fingerprints: &[Fingerprint]) {
        let track_id = self.tracks.get_or_insert(track_name);

        for fp in fingerprints {
            self.table
                .entry(fp.hash)
                .or_default()
                .push((track_id, fp.anchor_time));
        }
    }

    /// Queries the index with a set of fingerprints and returns the
    /// best-matching track, if any.
    ///
    /// The algorithm:
    /// 1. For each query fingerprint, look up all matching entries in the table.
    /// 2. For each match, compute `offset = db_time - query_time`.
    /// 3. Quantize the offset into histogram bins.
    /// 4. The track with the tallest histogram bin wins.
    ///
    /// Returns `None` if no matching hashes are found.
    #[must_use]
    pub fn query(&self, fingerprints: &[Fingerprint]) -> Option<QueryResult> {
        if fingerprints.is_empty() {
            return None;
        }

        // Per-track histogram: track_id -> (offset_bin -> count)
        let mut histograms: HashMap<TrackId, HashMap<i64, u32>> = HashMap::new();

        for fp in fingerprints {
            if let Some(entries) = self.table.get(&fp.hash) {
                for &(track_id, db_time) in entries {
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
            self.tracks.name(tid).map(|name| QueryResult {
                track_id: name.to_string(),
                score: best_score,
                offset_secs: self.bin_to_offset(best_bin),
            })
        })
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

    /// Saves the in-memory index to disk using persistent storage.
    ///
    /// This is a convenience method that creates a `PersistentIndex` and
    /// writes all tracks and fingerprints to disk.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence operations fail.
    #[cfg(feature = "persist")]
    pub fn save_to_disk<P: AsRef<std::path::Path>>(&self, path: P) -> Result<(), crate::error::WavioError> {
        use crate::persist::PersistentIndex;

        let mut persistent = PersistentIndex::open(&path)?;

        // Extract all tracks and their fingerprints from the in-memory index
        for (track_id, track_name) in &self.tracks.id_to_name {
            let mut track_fps = Vec::new();

            // Find all fingerprints for this track
            for (hash, entries) in &self.table {
                for (tid, anchor_time) in entries {
                    if tid == track_id {
                        track_fps.push(Fingerprint {
                            hash: *hash,
                            anchor_time: *anchor_time,
                        });
                    }
                }
            }

            persistent.insert(track_name, &track_fps)?;
        }

        persistent.flush()?;
        Ok(())
    }

    /// Loads an in-memory index from disk using persistent storage.
    ///
    /// This is a convenience method that opens a `PersistentIndex` and
    /// loads all data into memory.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence operations fail.
    #[cfg(feature = "persist")]
    pub fn load_from_disk<P: AsRef<std::path::Path>>(path: P) -> Result<Self, crate::error::WavioError> {
        use crate::persist::PersistentIndex;

        let persistent = PersistentIndex::open(path)?;
        persistent.load_into_memory()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Fingerprint;

    /// Helper: create a fingerprint with given hash and anchor time.
    fn fp(hash: u64, anchor_time: f32) -> Fingerprint {
        Fingerprint { hash, anchor_time }
    }

    // ---- Index 1 track, query exact clip -> correct match ----

    #[test]
    fn test_insert_and_exact_query() {
        let mut index = Index::default();
        let track_fps = vec![
            fp(1000, 0.0),
            fp(2000, 0.1),
            fp(3000, 0.2),
            fp(4000, 0.3),
            fp(5000, 0.4),
        ];
        index.insert("song_a", &track_fps);

        // Query with the exact same fingerprints (offset = 0).
        let result = index.query(&track_fps).unwrap();
        assert_eq!(result.track_id, "song_a");
        assert_eq!(result.score, 5);
        assert!(result.offset_secs.abs() < 0.1);
    }

    // ---- Index 10 tracks, query clip from track 5 -> correct match ----

    #[test]
    fn test_multi_track_query() {
        let mut index = Index::default();

        // Insert 10 tracks, each with distinct hash ranges.
        for track_idx in 0..10_u64 {
            let base_hash = track_idx * 10_000;
            let fps: Vec<Fingerprint> = (0..50)
                .map(|i| fp(base_hash + i, i as f32 * 0.02))
                .collect();
            index.insert(&format!("track_{track_idx}"), &fps);
        }

        assert_eq!(index.track_count(), 10);

        // Query with hashes belonging to track 5.
        let query_fps: Vec<Fingerprint> = (0..50)
            .map(|i| fp(50_000 + i, i as f32 * 0.02))
            .collect();

        let result = index.query(&query_fps).unwrap();
        assert_eq!(result.track_id, "track_5");
        assert_eq!(result.score, 50);
    }

    // ---- Query audio not in index -> None ----

    #[test]
    fn test_query_no_match() {
        let mut index = Index::default();
        let track_fps = vec![fp(100, 0.0), fp(200, 0.1)];
        index.insert("song_a", &track_fps);

        // Query with completely different hashes.
        let query_fps = vec![fp(999, 0.0), fp(888, 0.1)];
        let result = index.query(&query_fps);
        assert!(result.is_none());
    }

    // ---- Empty query -> None ----

    #[test]
    fn test_query_empty() {
        let index = Index::default();
        assert!(index.query(&[]).is_none());
    }

    // ---- Track count and hash count ----

    #[test]
    fn test_track_and_hash_counts() {
        let mut index = Index::default();
        assert_eq!(index.track_count(), 0);
        assert_eq!(index.hash_count(), 0);

        index.insert("song_a", &[fp(1, 0.0), fp(2, 0.1)]);
        assert_eq!(index.track_count(), 1);
        assert_eq!(index.hash_count(), 2);

        index.insert("song_b", &[fp(3, 0.0), fp(4, 0.1), fp(5, 0.2)]);
        assert_eq!(index.track_count(), 2);
        assert_eq!(index.hash_count(), 5);
    }

    // ---- Offset detection ----

    #[test]
    fn test_offset_detection() {
        let mut index = Index::default();

        // Index a track with anchor times starting at 0.
        let track_fps: Vec<Fingerprint> = (0..20)
            .map(|i| fp(i + 100, i as f32 * 0.05))
            .collect();
        index.insert("song_a", &track_fps);

        // Query with same hashes but shifted forward by 1.0 seconds.
        // This simulates querying a clip that starts 1s into the song.
        let shift = 1.0_f32;
        let query_fps: Vec<Fingerprint> = (0..20)
            .map(|i| fp(i + 100, i as f32 * 0.05 - shift))
            .collect();

        let result = index.query(&query_fps).unwrap();
        assert_eq!(result.track_id, "song_a");
        assert_eq!(result.score, 20);
        // offset = db_time - query_time = t - (t - 1.0) = 1.0
        assert!(
            (result.offset_secs - shift).abs() < 0.1,
            "Expected offset ~{shift}, got {}",
            result.offset_secs
        );
    }

    // ---- Partial overlap still matches ----

    #[test]
    fn test_partial_overlap_query() {
        let mut index = Index::default();

        let track_fps: Vec<Fingerprint> = (0..100)
            .map(|i| fp(i + 1000, i as f32 * 0.01))
            .collect();
        index.insert("song_a", &track_fps);

        // Query with only 30 of the 100 hashes (partial clip).
        let query_fps: Vec<Fingerprint> = (20..50)
            .map(|i| fp(i + 1000, i as f32 * 0.01))
            .collect();

        let result = index.query(&query_fps).unwrap();
        assert_eq!(result.track_id, "song_a");
        assert_eq!(result.score, 30);
    }

    // ---- Different offset bin sizes ----

    #[test]
    fn test_offset_bin_10ms() {
        let config = IndexConfig {
            offset_bin_size: 0.01, // 10 ms bins
        };
        let mut index = Index::new(config);
        let fps: Vec<Fingerprint> = (0..20)
            .map(|i| fp(i + 500, i as f32 * 0.03))
            .collect();
        index.insert("song_a", &fps);

        let result = index.query(&fps).unwrap();
        assert_eq!(result.track_id, "song_a");
        assert_eq!(result.score, 20);
    }

    // ---- Duplicate track name appends ----

    #[test]
    fn test_duplicate_insert_appends() {
        let mut index = Index::default();
        index.insert("song_a", &[fp(1, 0.0)]);
        index.insert("song_a", &[fp(2, 0.1)]);
        assert_eq!(index.track_count(), 1);
        assert_eq!(index.hash_count(), 2);
    }
}
