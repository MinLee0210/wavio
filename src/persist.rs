//! On-disk persistence for fingerprint indices using `redb`.
//!
//! [`PersistentIndex`] stores fingerprints in an embedded, ACID, pure-Rust
//! `redb` database and answers queries **directly from disk** -- nothing is
//! loaded into memory except the entries a query actually touches.
//!
//! # Disk layout (schema version 2)
//!
//! - `postings`: a multimap from `u64` hash to packed `u64` values, each
//!   `(track_id << 32) | anchor_time.to_bits()`. Appending one fingerprint
//!   is a single B-tree insert (O(log n)), independent of how many other
//!   tracks share the hash. Identical `(hash, track, time)` entries
//!   collapse into one, which makes re-saving an index idempotent.
//! - `tracks_by_name` / `tracks_by_id`: bidirectional track-name mapping.
//! - `metadata`: `schema_version`, `next_id`, the [`IndexConfig`], and the
//!   fingerprinting `signature`.
//!
//! # Compatibility checks
//!
//! The `signature` records the settings that produced the stored hashes
//! (hash mode, FFT and peak parameters -- see
//! [`Fingerprinter::signature`](crate::dsp::Fingerprinter::signature)).
//! [`PersistentIndex::ensure_signature`] refuses to proceed if the current
//! settings differ, instead of silently returning "no match". Opening a
//! database written by a *newer* schema fails with
//! [`WavioError::IncompatibleIndex`]; databases from the original
//! (pre-versioning, `hashes`-table) layout are migrated automatically.

use std::path::Path;

use redb::{
    Database, MultimapTableDefinition, ReadableMultimapTable, ReadableTable, ReadableTableMetadata,
    TableDefinition,
};

use crate::error::WavioError;
use crate::hash::Fingerprint;
use crate::index::{
    Index, IndexConfig, QueryOptions, QueryResult, ScaleSearch, TrackId, VoteTally,
};

type WavioResult<T> = Result<T, WavioError>;

/// Current on-disk schema version.
pub const SCHEMA_VERSION: u32 = 2;

const POSTINGS: MultimapTableDefinition<u64, u64> = MultimapTableDefinition::new("postings");
const TRACKS_BY_NAME: TableDefinition<&str, u32> = TableDefinition::new("tracks_by_name");
const TRACKS_BY_ID: TableDefinition<u32, &str> = TableDefinition::new("tracks_by_id");
const METADATA: TableDefinition<&str, &[u8]> = TableDefinition::new("metadata");
/// Schema-1 layout: one bincode `Vec<(u32, f32)>` per hash. Migrated on open.
const LEGACY_HASHES: TableDefinition<u64, &[u8]> = TableDefinition::new("hashes");

const KEY_SCHEMA: &str = "schema_version";
const KEY_NEXT_ID: &str = "next_id";
const KEY_CONFIG: &str = "config";
const KEY_SIGNATURE: &str = "signature";

fn io<E: std::fmt::Display>(e: E) -> WavioError {
    WavioError::IoError(e.to_string())
}

fn pack(track: TrackId, anchor_time: f32) -> u64 {
    (u64::from(track) << 32) | u64::from(anchor_time.to_bits())
}

#[allow(clippy::cast_possible_truncation)]
fn unpack(v: u64) -> (TrackId, f32) {
    ((v >> 32) as TrackId, f32::from_bits(v as u32))
}

fn read_u32(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// On-disk persistent fingerprint index backed by `redb`.
///
/// Offers the same query interface as [`Index`], answered from disk.
#[derive(Debug)]
pub struct PersistentIndex {
    db: Database,
    config: IndexConfig,
}

impl PersistentIndex {
    /// Opens or creates a persistent index at the given path.
    ///
    /// Databases in the original layout are migrated in place.
    ///
    /// # Errors
    ///
    /// Returns [`WavioError::IncompatibleIndex`] if the database was written
    /// by a newer schema, [`WavioError::IndexError`] if its stored
    /// configuration is corrupt, or [`WavioError::IoError`] for storage
    /// failures.
    pub fn open<P: AsRef<Path>>(path: P) -> WavioResult<Self> {
        let path = path.as_ref();
        let db = if path.exists() {
            Database::open(path).map_err(io)?
        } else {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).map_err(io)?;
                }
            }
            Database::create(path).map_err(io)?
        };

        let txn = db.begin_write().map_err(io)?;
        let config = {
            let mut postings = txn.open_multimap_table(POSTINGS).map_err(io)?;
            txn.open_table(TRACKS_BY_NAME).map_err(io)?;
            txn.open_table(TRACKS_BY_ID).map_err(io)?;
            let mut meta = txn.open_table(METADATA).map_err(io)?;

            let version = meta
                .get(KEY_SCHEMA)
                .map_err(io)?
                .map(|v| read_u32(v.value()));
            match version {
                Some(Some(v)) if v > SCHEMA_VERSION => {
                    return Err(WavioError::IncompatibleIndex(format!(
                        "database uses schema version {v}, but this build of wavio only \
                         understands up to version {SCHEMA_VERSION}"
                    )));
                }
                Some(None) => {
                    return Err(WavioError::IndexError(
                        "corrupt schema_version in database metadata".into(),
                    ));
                }
                Some(Some(_)) => {}
                None => {
                    Self::migrate_legacy(&txn, &mut postings)?;
                    meta.insert(KEY_SCHEMA, SCHEMA_VERSION.to_le_bytes().as_slice())
                        .map_err(io)?;
                }
            }

            let stored = meta
                .get(KEY_CONFIG)
                .map_err(io)?
                .map(|v| bincode::deserialize::<IndexConfig>(v.value()));
            match stored {
                Some(Ok(cfg)) => cfg,
                Some(Err(e)) => {
                    return Err(WavioError::IndexError(format!(
                        "corrupt index config in database metadata: {e}"
                    )));
                }
                None => {
                    let cfg = IndexConfig::default();
                    let bytes = bincode::serialize(&cfg)
                        .map_err(|e| WavioError::IndexError(e.to_string()))?;
                    meta.insert(KEY_CONFIG, bytes.as_slice()).map_err(io)?;
                    cfg
                }
            }
        };
        txn.commit().map_err(io)?;

        Ok(Self { db, config })
    }

    /// Converts a schema-1 `hashes` table (if present) into `postings`.
    fn migrate_legacy(
        txn: &redb::WriteTransaction,
        postings: &mut redb::MultimapTable<u64, u64>,
    ) -> WavioResult<()> {
        {
            let legacy = txn.open_table(LEGACY_HASHES).map_err(io)?;
            for row in legacy.iter().map_err(io)? {
                let (hash, val) = row.map_err(io)?;
                let entries: Vec<(TrackId, f32)> = bincode::deserialize(val.value())
                    .map_err(|e| WavioError::IndexError(format!("corrupt legacy entry: {e}")))?;
                for (track, time) in entries {
                    postings
                        .insert(hash.value(), pack(track, time))
                        .map_err(io)?;
                }
            }
        }
        txn.delete_table(LEGACY_HASHES).map_err(io)?;
        Ok(())
    }

    /// Returns the index configuration stored in the database.
    #[must_use]
    pub fn config(&self) -> &IndexConfig {
        &self.config
    }

    /// Replaces the stored index configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the database write fails.
    pub fn set_config(&mut self, config: IndexConfig) -> WavioResult<()> {
        let bytes =
            bincode::serialize(&config).map_err(|e| WavioError::IndexError(e.to_string()))?;
        let txn = self.db.begin_write().map_err(io)?;
        {
            let mut meta = txn.open_table(METADATA).map_err(io)?;
            meta.insert(KEY_CONFIG, bytes.as_slice()).map_err(io)?;
        }
        txn.commit().map_err(io)?;
        self.config = config;
        Ok(())
    }

    /// Returns the fingerprinting-settings signature stored in the database,
    /// or `None` for a new database or one created before signatures existed.
    #[must_use]
    pub fn signature(&self) -> Option<String> {
        let txn = self.db.begin_read().ok()?;
        let meta = txn.open_table(METADATA).ok()?;
        let v = meta.get(KEY_SIGNATURE).ok()??;
        String::from_utf8(v.value().to_vec()).ok()
    }

    /// Verifies `signature` against the stored one without modifying the
    /// database. A database with no stored signature (new, or created before
    /// signatures existed) passes.
    ///
    /// # Errors
    ///
    /// Returns [`WavioError::IncompatibleIndex`] if a different signature is
    /// stored.
    pub fn check_signature(&self, signature: &str) -> WavioResult<()> {
        match self.signature() {
            Some(stored) if stored != signature => Err(WavioError::IncompatibleIndex(format!(
                "this index was built with `{stored}`, but the current settings are \
                 `{signature}`; fingerprints from different settings never match"
            ))),
            _ => Ok(()),
        }
    }

    /// Records `signature` as the settings this database was built with, or
    /// verifies it against the stored one (see
    /// [`PersistentIndex::check_signature`]).
    ///
    /// # Errors
    ///
    /// Returns [`WavioError::IncompatibleIndex`] if a different signature is
    /// already stored.
    pub fn ensure_signature(&mut self, signature: &str) -> WavioResult<()> {
        self.check_signature(signature)?;
        if self.signature().is_some() {
            return Ok(());
        }
        let txn = self.db.begin_write().map_err(io)?;
        {
            let mut meta = txn.open_table(METADATA).map_err(io)?;
            meta.insert(KEY_SIGNATURE, signature.as_bytes())
                .map_err(io)?;
        }
        txn.commit().map_err(io)
    }

    /// Returns the number of indexed tracks.
    #[must_use]
    pub fn track_count(&self) -> usize {
        let Ok(txn) = self.db.begin_read() else {
            return 0;
        };
        let Ok(t) = txn.open_table(TRACKS_BY_ID) else {
            return 0;
        };
        t.len()
            .map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX))
    }

    /// Returns the total number of stored `(hash, track, time)` entries.
    #[must_use]
    pub fn hash_count(&self) -> usize {
        let Ok(txn) = self.db.begin_read() else {
            return 0;
        };
        let Ok(t) = txn.open_multimap_table(POSTINGS) else {
            return 0;
        };
        t.len()
            .map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX))
    }

    /// Returns whether a track with the given name is indexed.
    #[must_use]
    pub fn contains_track(&self, name: &str) -> bool {
        let Ok(txn) = self.db.begin_read() else {
            return false;
        };
        let Ok(t) = txn.open_table(TRACKS_BY_NAME) else {
            return false;
        };
        matches!(t.get(name), Ok(Some(_)))
    }

    /// Inserts one track's fingerprints. See [`PersistentIndex::insert_many`].
    ///
    /// # Errors
    ///
    /// Returns an error if persistence operations fail.
    pub fn insert(&mut self, track_name: &str, fingerprints: &[Fingerprint]) -> WavioResult<()> {
        self.insert_many(&[(track_name, fingerprints.to_vec())])
    }

    /// Inserts many tracks in a **single transaction** (one fsync).
    ///
    /// If a track name already exists, its new fingerprints are added to the
    /// existing ones; use [`PersistentIndex::remove_track`] first to replace.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence operations fail; nothing is written
    /// in that case.
    pub fn insert_many<S: AsRef<str>>(
        &mut self,
        batch: &[(S, Vec<Fingerprint>)],
    ) -> WavioResult<()> {
        let txn = self.db.begin_write().map_err(io)?;
        {
            let mut meta = txn.open_table(METADATA).map_err(io)?;
            let mut by_name = txn.open_table(TRACKS_BY_NAME).map_err(io)?;
            let mut by_id = txn.open_table(TRACKS_BY_ID).map_err(io)?;
            let mut postings = txn.open_multimap_table(POSTINGS).map_err(io)?;

            let mut next_id: TrackId = meta
                .get(KEY_NEXT_ID)
                .map_err(io)?
                .and_then(|v| read_u32(v.value()))
                .unwrap_or(0);

            for (name, fps) in batch {
                let name = name.as_ref();
                let existing = by_name.get(name).map_err(io)?.map(|v| v.value());
                let track_id = if let Some(id) = existing {
                    id
                } else {
                    let id = next_id;
                    next_id += 1;
                    by_name.insert(name, id).map_err(io)?;
                    by_id.insert(id, name).map_err(io)?;
                    id
                };
                for fp in fps {
                    postings
                        .insert(fp.hash, pack(track_id, fp.anchor_time))
                        .map_err(io)?;
                }
            }
            meta.insert(KEY_NEXT_ID, next_id.to_le_bytes().as_slice())
                .map_err(io)?;
        }
        txn.commit().map_err(io)
    }

    /// Removes a track and all of its fingerprints. Returns `false` if no
    /// such track exists.
    ///
    /// This scans every posting once (O(total entries)).
    ///
    /// # Errors
    ///
    /// Returns an error if persistence operations fail.
    pub fn remove_track(&mut self, name: &str) -> WavioResult<bool> {
        let txn = self.db.begin_write().map_err(io)?;
        let removed = {
            let mut by_name = txn.open_table(TRACKS_BY_NAME).map_err(io)?;
            let mut by_id = txn.open_table(TRACKS_BY_ID).map_err(io)?;
            let mut postings = txn.open_multimap_table(POSTINGS).map_err(io)?;

            let Some(id) = by_name.remove(name).map_err(io)?.map(|v| v.value()) else {
                return Ok(false);
            };
            by_id.remove(id).map_err(io)?;

            let mut doomed: Vec<(u64, u64)> = Vec::new();
            for row in postings.iter().map_err(io)? {
                let (hash, values) = row.map_err(io)?;
                for v in values {
                    let v = v.map_err(io)?.value();
                    if unpack(v).0 == id {
                        doomed.push((hash.value(), v));
                    }
                }
            }
            for (hash, v) in doomed {
                postings.remove(hash, v).map_err(io)?;
            }
            true
        };
        txn.commit().map_err(io)?;
        Ok(removed)
    }

    /// Queries the index and returns the best-matching track, if any.
    ///
    /// Returns `None` if nothing matches (or the database cannot be read).
    #[must_use]
    pub fn query(&self, fingerprints: &[Fingerprint]) -> Option<QueryResult> {
        self.query_topn(fingerprints, 1).into_iter().next()
    }

    /// Like [`PersistentIndex::query`], but returns `None` unless the best
    /// match's `confidence` is at least `min_confidence`.
    #[must_use]
    pub fn query_with_min_confidence(
        &self,
        fingerprints: &[Fingerprint],
        min_confidence: f32,
    ) -> Option<QueryResult> {
        self.query(fingerprints)
            .filter(|r| r.confidence >= min_confidence)
    }

    /// Queries the index and returns up to `n` ranked matches, best first.
    ///
    /// Reads only the postings for the query's hashes. Returns an empty
    /// list if nothing matches (or the database cannot be read).
    #[must_use]
    pub fn query_topn(&self, fingerprints: &[Fingerprint], n: usize) -> Vec<QueryResult> {
        self.query_with(fingerprints, n, &QueryOptions::default())
    }

    /// Like [`PersistentIndex::query`], but with time-scale-aware voting
    /// (see [`ScaleSearch`]).
    #[must_use]
    pub fn query_scaled(
        &self,
        fingerprints: &[Fingerprint],
        search: &ScaleSearch,
    ) -> Option<QueryResult> {
        self.query_with(
            fingerprints,
            1,
            &QueryOptions::default().with_scale_search(*search),
        )
        .into_iter()
        .next()
    }

    /// Queries with explicit [`QueryOptions`] (rarity weighting, time-scale
    /// search) and returns up to `n` ranked matches, best first.
    #[must_use]
    pub fn query_with(
        &self,
        fingerprints: &[Fingerprint],
        n: usize,
        options: &QueryOptions,
    ) -> Vec<QueryResult> {
        let Ok(txn) = self.db.begin_read() else {
            return Vec::new();
        };
        let (Ok(postings), Ok(names)) = (
            txn.open_multimap_table(POSTINGS),
            txn.open_table(TRACKS_BY_ID),
        ) else {
            return Vec::new();
        };

        let mut tally = VoteTally::new(self.config.offset_bin_size, options);
        for fp in fingerprints {
            if let Ok(values) = postings.get(fp.hash) {
                let count = usize::try_from(values.len()).unwrap_or(usize::MAX);
                for v in values.flatten() {
                    let (track, db_time) = unpack(v.value());
                    tally.add(track, db_time, fp.anchor_time, count);
                }
            }
        }
        tally.rank(n, fingerprints.len(), |tid| {
            names.get(tid).ok().flatten().map(|v| v.value().to_string())
        })
    }

    /// Persists pending updates. `redb` commits are durable as soon as each
    /// write transaction finishes, so this is a no-op kept for API symmetry.
    ///
    /// # Errors
    ///
    /// Never fails today.
    #[allow(clippy::unused_self)]
    pub fn flush(&mut self) -> WavioResult<()> {
        Ok(())
    }

    /// Loads the whole database into memory as an [`Index`].
    ///
    /// # Errors
    ///
    /// Returns an error if a database read fails.
    pub fn load_into_memory(&self) -> WavioResult<Index> {
        let mut index = Index::new(self.config.clone());
        if let Some(sig) = self.signature() {
            index.set_signature(sig);
        }

        let txn = self.db.begin_read().map_err(io)?;
        let names = txn.open_table(TRACKS_BY_ID).map_err(io)?;
        let postings = txn.open_multimap_table(POSTINGS).map_err(io)?;

        // Register every track first so ids/names (including empty tracks) survive.
        let mut name_of: std::collections::HashMap<TrackId, String> =
            std::collections::HashMap::new();
        for row in names.iter().map_err(io)? {
            let (id, name) = row.map_err(io)?;
            name_of.insert(id.value(), name.value().to_string());
        }
        let mut sorted: Vec<_> = name_of.iter().collect();
        sorted.sort_by_key(|(id, _)| **id);
        for (_, name) in sorted {
            index.insert(name, &[]);
        }

        let mut per_track: std::collections::HashMap<TrackId, Vec<Fingerprint>> =
            std::collections::HashMap::new();
        for row in postings.iter().map_err(io)? {
            let (hash, values) = row.map_err(io)?;
            for v in values {
                let (track, time) = unpack(v.map_err(io)?.value());
                per_track
                    .entry(track)
                    .or_default()
                    .push(Fingerprint::new(hash.value(), time));
            }
        }
        for (track, fps) in per_track {
            if let Some(name) = name_of.get(&track) {
                index.insert(name, &fps);
            }
        }
        Ok(index)
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_cmp,
    clippy::similar_names,
    clippy::needless_pass_by_value
)]
mod tests {
    use super::*;

    fn fp(hash: u64, anchor_time: f32) -> Fingerprint {
        Fingerprint::new(hash, anchor_time)
    }

    fn db_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().join("test.redb")
    }

    #[test]
    fn insert_query_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = db_path(&dir);
        let fps: Vec<_> = (0..5).map(|i| fp(1000 * (i + 1), i as f32 * 0.1)).collect();

        {
            let mut index = PersistentIndex::open(&path).unwrap();
            index.insert("song_a", &fps).unwrap();
            let qr = index.query(&fps).unwrap();
            assert_eq!(qr.track_id, "song_a");
            assert_eq!(qr.score, 5);
        }

        let index = PersistentIndex::open(&path).unwrap();
        assert_eq!(index.track_count(), 1);
        assert_eq!(index.hash_count(), 5);
        assert!(index.contains_track("song_a"));
        assert!(!index.contains_track("nope"));
        assert_eq!(index.query(&fps).unwrap().score, 5);
    }

    #[test]
    fn multiple_tracks_in_one_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = PersistentIndex::open(db_path(&dir)).unwrap();
        let batch: Vec<(String, Vec<Fingerprint>)> = (0..3_u64)
            .map(|t| {
                let fps = (0..10)
                    .map(|i| fp(t * 10_000 + i, i as f32 * 0.05))
                    .collect();
                (format!("track_{t}"), fps)
            })
            .collect();
        index.insert_many(&batch).unwrap();

        assert_eq!(index.track_count(), 3);
        assert_eq!(index.hash_count(), 30);
        let q: Vec<_> = (0..10).map(|i| fp(10_000 + i, i as f32 * 0.05)).collect();
        let r = index.query(&q).unwrap();
        assert_eq!(r.track_id, "track_1");
        assert_eq!(r.score, 10);
    }

    #[test]
    fn matches_in_memory_index() {
        let dir = tempfile::tempdir().unwrap();
        let mut mem = Index::default();
        let mut disk = PersistentIndex::open(db_path(&dir)).unwrap();
        for t in 0..4_u64 {
            let fps: Vec<_> = (0..40)
                .map(|i| fp((t * 7 + i * 3) % 50, i as f32 * 0.02 + t as f32 * 0.01))
                .collect();
            mem.insert(&format!("t{t}"), &fps);
            disk.insert(&format!("t{t}"), &fps).unwrap();
        }
        let q: Vec<_> = (0..30).map(|i| fp(i * 3 % 50, i as f32 * 0.02)).collect();
        let (a, b) = (mem.query_topn(&q, 4), disk.query_topn(&q, 4));
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.track_id, y.track_id);
            assert_eq!(x.score, y.score);
        }
    }

    #[test]
    fn save_and_load_round_trip_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = db_path(&dir);
        let fps = vec![fp(1001, 0.0), fp(2002, 0.1), fp(3003, 0.2), fp(4004, 0.3)];

        let mut mem = Index::default();
        mem.insert("test_song", &fps);
        mem.insert("empty_track", &[]);
        mem.save_to_disk(&path).unwrap();
        mem.save_to_disk(&path).unwrap(); // saving again must not duplicate

        let loaded = Index::load_from_disk(&path).unwrap();
        assert_eq!(loaded.track_count(), 2);
        assert_eq!(loaded.hash_count(), 4);
        let r = loaded.query(&fps).unwrap();
        assert_eq!(r.track_id, "test_song");
        assert_eq!(r.score, 4);
    }

    #[test]
    fn remove_track_deletes_only_that_track() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = PersistentIndex::open(db_path(&dir)).unwrap();
        index.insert("a", &[fp(1, 0.0), fp(2, 0.1)]).unwrap();
        index.insert("b", &[fp(1, 0.0), fp(3, 0.1)]).unwrap();

        assert!(index.remove_track("a").unwrap());
        assert!(!index.remove_track("a").unwrap());
        assert_eq!(index.track_count(), 1);
        assert_eq!(index.hash_count(), 2);
        assert_eq!(index.query(&[fp(1, 0.0)]).unwrap().track_id, "b");

        // Re-adding gets a fresh id and works.
        index.insert("a", &[fp(2, 0.1)]).unwrap();
        assert_eq!(index.query(&[fp(2, 0.1)]).unwrap().track_id, "a");
    }

    #[test]
    fn signature_is_stored_and_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let path = db_path(&dir);
        {
            let mut index = PersistentIndex::open(&path).unwrap();
            assert_eq!(index.signature(), None);
            index.ensure_signature("mode=pairwise").unwrap();
            index.ensure_signature("mode=pairwise").unwrap();
        }
        let mut index = PersistentIndex::open(&path).unwrap();
        assert_eq!(index.signature().as_deref(), Some("mode=pairwise"));
        let err = index.ensure_signature("mode=triplet").unwrap_err();
        assert!(matches!(err, WavioError::IncompatibleIndex(_)), "{err}");
    }

    #[test]
    fn config_is_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let path = db_path(&dir);
        {
            let mut index = PersistentIndex::open(&path).unwrap();
            index.set_config(IndexConfig::new(0.2)).unwrap();
        }
        let index = PersistentIndex::open(&path).unwrap();
        assert!((index.config().offset_bin_size - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn newer_schema_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = db_path(&dir);
        drop(PersistentIndex::open(&path).unwrap());
        {
            let db = Database::open(&path).unwrap();
            let txn = db.begin_write().unwrap();
            {
                let mut meta = txn.open_table(METADATA).unwrap();
                meta.insert(KEY_SCHEMA, 99_u32.to_le_bytes().as_slice())
                    .unwrap();
            }
            txn.commit().unwrap();
        }
        let err = PersistentIndex::open(&path).unwrap_err();
        assert!(matches!(err, WavioError::IncompatibleIndex(_)), "{err}");
    }

    #[test]
    fn corrupt_config_is_an_error_not_a_silent_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = db_path(&dir);
        drop(PersistentIndex::open(&path).unwrap());
        {
            let db = Database::open(&path).unwrap();
            let txn = db.begin_write().unwrap();
            {
                let mut meta = txn.open_table(METADATA).unwrap();
                meta.insert(KEY_CONFIG, b"x".as_slice()).unwrap();
            }
            txn.commit().unwrap();
        }
        assert!(matches!(
            PersistentIndex::open(&path).unwrap_err(),
            WavioError::IndexError(_)
        ));
    }

    #[test]
    fn legacy_schema_1_database_is_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.redb");
        {
            // Build a database exactly the way v0.3.0 did.
            let db = Database::create(&path).unwrap();
            let txn = db.begin_write().unwrap();
            {
                let mut hashes = txn.open_table(LEGACY_HASHES).unwrap();
                let mut by_name = txn.open_table(TRACKS_BY_NAME).unwrap();
                let mut by_id = txn.open_table(TRACKS_BY_ID).unwrap();
                let mut meta = txn.open_table(METADATA).unwrap();
                by_name.insert("old_song", 0).unwrap();
                by_id.insert(0, "old_song").unwrap();
                meta.insert(KEY_NEXT_ID, 1_u32.to_le_bytes().as_slice())
                    .unwrap();
                for (h, t) in [(11_u64, 0.0_f32), (22, 0.5), (33, 1.0)] {
                    let enc = bincode::serialize(&vec![(0_u32, t)]).unwrap();
                    hashes.insert(h, enc.as_slice()).unwrap();
                }
            }
            txn.commit().unwrap();
        }

        let index = PersistentIndex::open(&path).unwrap();
        assert_eq!(index.track_count(), 1);
        assert_eq!(index.hash_count(), 3);
        let q = vec![fp(11, 0.0), fp(22, 0.5), fp(33, 1.0)];
        let r = index.query(&q).unwrap();
        assert_eq!(r.track_id, "old_song");
        assert_eq!(r.score, 3);

        // Migration is one-way and stable across reopen.
        drop(index);
        let index = PersistentIndex::open(&path).unwrap();
        assert_eq!(index.hash_count(), 3);
    }

    #[test]
    fn scaled_query_matches_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let mut disk = PersistentIndex::open(db_path(&dir)).unwrap();
        let mut mem = Index::default();
        let track: Vec<_> = (0..150).map(|i| fp(100 + i, i as f32 * 0.07)).collect();
        disk.insert("song", &track).unwrap();
        mem.insert("song", &track);

        let query: Vec<_> = track
            .iter()
            .map(|f| fp(f.hash, f.anchor_time / 1.04))
            .collect();
        let search = ScaleSearch::default();
        let (a, b) = (
            mem.query_scaled(&query, &search).unwrap(),
            disk.query_scaled(&query, &search).unwrap(),
        );
        assert_eq!((a.score, a.speed.to_bits()), (b.score, b.speed.to_bits()));
        assert!((b.speed - 1.04).abs() < 0.006);
    }

    #[test]
    fn pack_round_trips() {
        for (t, a) in [(0, 0.0_f32), (7, 12.5), (u32::MAX, 3600.123)] {
            assert_eq!(unpack(pack(t, a)), (t, a));
        }
    }
}
