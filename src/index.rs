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
#[non_exhaustive]
pub struct QueryResult {
    /// Name of the best-matching track.
    pub track_id: String,
    /// Number of time-aligned hash hits (histogram peak height).
    pub score: u32,
    /// Estimated time offset (seconds) between the query clip and the
    /// indexed track. Positive means the query starts later in the track.
    pub offset_secs: f32,
    /// `score` normalized by the number of fingerprints in the query
    /// (`score / query_fingerprint_count`), roughly in `[0.0, 1.0]`.
    ///
    /// This indicates how *decisive* the match is, independent of clip
    /// length: a short, noisy query that barely edges out competing tracks
    /// will have a low confidence even if `score` is nonzero, while a clean
    /// match against most of the query's fingerprints approaches `1.0`.
    pub confidence: f32,
    /// How far this match is ahead of the next-best track, as a fraction of
    /// its own score: `(score - runner_up_score) / score`, in `[0.0, 1.0]`.
    ///
    /// `1.0` means no other track scored at all; values near `0.0` mean a
    /// close call between two tracks (a likely false positive or a cover /
    /// near-duplicate). Unlike `confidence`, this does not shrink just
    /// because the query clip contains audio that is not in the index.
    pub margin: f32,
    /// Estimated playback-speed factor of the query relative to the indexed
    /// track (`1.02` = the clip is 2% faster). Always `1.0` unless a
    /// time-scale search was requested via [`ScaleSearch`].
    pub speed: f32,
    /// The ranking score: `score` with each vote weighted by the rarity of
    /// its hash (see [`QueryOptions::idf_weighting`]). Equal to `score` when
    /// no weighting was requested. `margin` is computed from this value.
    pub weighted_score: f32,
}

/// Options for [`Index::query_with`] / `PersistentIndex::query_with`.
///
/// Every field defaults to off, which reproduces [`Index::query_topn`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct QueryOptions {
    /// Search playback-speed hypotheses (see [`ScaleSearch`]).
    pub scale_search: Option<ScaleSearch>,
    /// Weight each vote by `1 / (1 + ln(postings))`, where `postings` is how
    /// many times that hash occurs in the index, so common hashes count for
    /// less than distinctive ones.
    ///
    /// Coarse hashes (triplet ratios, hashes from looping or repetitive
    /// music) collide often; unweighted, those collisions can out-vote the
    /// truth. Ranking uses the weighted score; `QueryResult::score` still
    /// reports the raw number of aligned hashes.
    pub idf_weighting: bool,
}

impl QueryOptions {
    /// All options off.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enables time-scale-aware voting.
    #[must_use]
    pub fn with_scale_search(mut self, search: ScaleSearch) -> Self {
        self.scale_search = Some(search);
        self
    }

    /// Enables or disables rarity (IDF) weighting.
    #[must_use]
    pub fn with_idf_weighting(mut self, enabled: bool) -> Self {
        self.idf_weighting = enabled;
        self
    }
}

/// Parameters for time-scale-aware voting.
///
/// A speed change (tape / vinyl / resampling) makes the offset between query
/// and track drift linearly across the clip, which smears votes over many
/// histogram bins and costs score. With a scale search, the tallest few
/// candidates are re-scored under a grid of speed hypotheses
/// `db_time ≈ speed · query_time + offset`, and the best hypothesis wins.
/// Searching more hypotheses also raises the chance of a spurious alignment,
/// so keep `max_deviation` no larger than the speed changes you expect.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct ScaleSearch {
    /// Largest speed deviation to try, as a fraction (`0.08` = 0.92x..1.08x).
    pub max_deviation: f32,
    /// Spacing between speed hypotheses (`0.01` = 1%).
    pub step: f32,
    /// How many top-ranked tracks (by plain score) to re-score.
    pub candidates: usize,
}

impl Default for ScaleSearch {
    fn default() -> Self {
        Self {
            max_deviation: 0.08,
            step: 0.01,
            candidates: 5,
        }
    }
}

impl ScaleSearch {
    /// Creates a `ScaleSearch`.
    #[must_use]
    pub fn new(max_deviation: f32, step: f32, candidates: usize) -> Self {
        Self {
            max_deviation,
            step,
            candidates,
        }
    }
}

// ---------------------------------------------------------------------------
// Vote tallying (shared by `Index` and `PersistentIndex`)
// ---------------------------------------------------------------------------

/// Accumulates time-offset votes per track and turns them into ranked
/// [`QueryResult`]s.
///
/// Both the in-memory and the on-disk index feed `(track, db_time,
/// query_time)` triples into one of these, so the scoring rules live in
/// exactly one place.
///
/// A track's score is the tallest *two adjacent* offset bins summed. A true
/// alignment has timing jitter (peak quantization, resampling), so its votes
/// straddle a bin boundary about as often as not; summing neighbours stops
/// that from splitting a real peak in half. The reported offset is the
/// vote-weighted mean of those two bins, which recovers sub-bin precision.
#[derive(Debug)]
pub(crate) struct VoteTally {
    bin_size: f32,
    histograms: HashMap<TrackId, HashMap<i64, Cell>>,
    /// Raw `(db_time, query_time, weight)` matches per track; recorded only
    /// when a scale search was requested.
    raw: Option<RawMatches>,
    search: Option<ScaleSearch>,
    idf: bool,
}

/// Per-track raw `(db_time, query_time, weight)` matches.
type RawMatches = HashMap<TrackId, Vec<(f32, f32, f32)>>;

/// Votes in one offset bin: how many, and their total weight.
#[derive(Debug, Default, Clone, Copy)]
struct Cell {
    raw: u32,
    weight: f32,
}

/// Best alignment found for one track.
#[derive(Debug, Clone, Copy)]
struct Window {
    raw: u32,
    weight: f32,
    offset_secs: f32,
    speed: f32,
}

impl VoteTally {
    pub(crate) fn new(bin_size: f32, options: &QueryOptions) -> Self {
        Self {
            bin_size,
            histograms: HashMap::new(),
            raw: options.scale_search.map(|_| HashMap::new()),
            search: options.scale_search,
            idf: options.idf_weighting,
        }
    }

    /// Records one matching hash: the track it was found in, the time it
    /// occurs in that track, the time it occurs in the query, and how many
    /// postings the hash has in the index (for rarity weighting).
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    pub(crate) fn add(&mut self, track: TrackId, db_time: f32, query_time: f32, postings: usize) {
        let weight = if self.idf {
            1.0 / (1.0 + (postings.max(1) as f32).ln())
        } else {
            1.0
        };
        let bin = ((db_time - query_time) / self.bin_size).round() as i64;
        let cell = self
            .histograms
            .entry(track)
            .or_default()
            .entry(bin)
            .or_default();
        cell.raw += 1;
        cell.weight += weight;
        if let Some(raw) = &mut self.raw {
            raw.entry(track)
                .or_default()
                .push((db_time, query_time, weight));
        }
    }

    /// Best alignment for one track: the heaviest pair of adjacent bins.
    /// (Weights are compared exactly on purpose: ties are broken
    /// deterministically, and unweighted votes are exact small integers.)
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::float_cmp
    )]
    fn best_window(&self, bins: &HashMap<i64, Cell>) -> Window {
        let cell = |b: i64| bins.get(&b).copied().unwrap_or_default();
        let mut best: Option<(f32, i64)> = None;
        for &b in bins.keys() {
            for start in [b - 1, b] {
                let weight = cell(start).weight + cell(start + 1).weight;
                let better = match best {
                    None => true,
                    Some((w, st)) => weight > w || (weight == w && start < st),
                };
                if better {
                    best = Some((weight, start));
                }
            }
        }
        let Some((weight, start)) = best else {
            return Window {
                raw: 0,
                weight: 0.0,
                offset_secs: 0.0,
                speed: 1.0,
            };
        };
        let (c0, c1) = (cell(start), cell(start + 1));
        let offset_bins = (start as f64 * f64::from(c0.weight)
            + (start + 1) as f64 * f64::from(c1.weight))
            / f64::from(c0.weight + c1.weight);
        Window {
            raw: c0.raw + c1.raw,
            weight,
            offset_secs: (offset_bins * f64::from(self.bin_size)) as f32,
            speed: 1.0,
        }
    }

    /// Re-scores one track's raw matches under a grid of speed hypotheses.
    ///
    /// Neighboring hypotheses often tie (the two-bin vote window absorbs a
    /// small drift), so among the top-scoring ones the **median** is taken
    /// rather than the one nearest 1.0, which would bias estimates toward 1.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::float_cmp
    )]
    fn best_scaled(&self, matches: &[(f32, f32, f32)], search: &ScaleSearch) -> Window {
        let steps = (search.max_deviation / search.step).round() as i32;

        let mut best_weight = 0.0_f32;
        let mut tied: Vec<Window> = Vec::new();
        for k in -steps..=steps {
            let speed = 1.0 + k as f32 * search.step;
            let mut bins: HashMap<i64, Cell> = HashMap::new();
            for &(db, q, w) in matches {
                let bin = ((db - speed * q) / self.bin_size).round() as i64;
                let cell = bins.entry(bin).or_default();
                cell.raw += 1;
                cell.weight += w;
            }
            let mut window = self.best_window(&bins);
            window.speed = speed;
            if window.weight > best_weight {
                best_weight = window.weight;
                tied.clear();
            }
            if window.weight == best_weight {
                tied.push(window);
            }
        }
        tied.get(tied.len() / 2).copied().unwrap_or(Window {
            raw: 0,
            weight: 0.0,
            offset_secs: 0.0,
            speed: 1.0,
        })
    }

    /// Ranks all voted tracks and returns the best `n`, resolving names
    /// lazily (only for the returned tracks) through `name_of`.
    ///
    /// Ties are broken by `TrackId` so results are deterministic.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn rank<F>(self, n: usize, query_len: usize, name_of: F) -> Vec<QueryResult>
    where
        F: Fn(TrackId) -> Option<String>,
    {
        if n == 0 || query_len == 0 {
            return Vec::new();
        }
        let by_weight = |a: &(TrackId, Window), b: &(TrackId, Window)| {
            b.1.weight.total_cmp(&a.1.weight).then(a.0.cmp(&b.0))
        };

        let mut ranked: Vec<(TrackId, Window)> = self
            .histograms
            .iter()
            .map(|(&tid, bins)| (tid, self.best_window(bins)))
            .collect();
        ranked.sort_by(by_weight);

        if let (Some(search), Some(raw)) = (&self.search, &self.raw) {
            for entry in ranked.iter_mut().take(search.candidates) {
                if let Some(matches) = raw.get(&entry.0) {
                    let scaled = self.best_scaled(matches, search);
                    if scaled.weight > entry.1.weight {
                        entry.1 = scaled;
                    }
                }
            }
            ranked.sort_by(by_weight);
        }

        let query_len = query_len as f32;
        let mut results = Vec::with_capacity(n.min(ranked.len()));
        for (i, (tid, w)) in ranked.iter().enumerate() {
            if results.len() >= n {
                break;
            }
            let Some(name) = name_of(*tid) else { continue };
            let runner_up = ranked.get(i + 1).map_or(0.0, |r| r.1.weight);
            results.push(QueryResult {
                track_id: name,
                score: w.raw,
                offset_secs: w.offset_secs,
                confidence: w.raw as f32 / query_len,
                margin: ((w.weight - runner_up) / w.weight).clamp(0.0, 1.0),
                speed: w.speed,
                weighted_score: w.weight,
            });
        }
        results
    }
}

// ---------------------------------------------------------------------------
// Index configuration
// ---------------------------------------------------------------------------

/// Configuration for the in-memory index.
#[derive(Debug, Clone)]
#[non_exhaustive]
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

impl IndexConfig {
    /// Creates a new `IndexConfig`.
    #[must_use]
    pub fn new(offset_bin_size: f32) -> Self {
        Self { offset_bin_size }
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
    /// Fingerprinting-settings signature (see
    /// [`Fingerprinter::signature`](crate::dsp::Fingerprinter::signature)),
    /// carried through save / load so a mismatched index can be rejected.
    signature: Option<String>,
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
            signature: None,
        }
    }

    /// Returns the fingerprinting-settings signature this index was tagged
    /// with, if any.
    #[must_use]
    pub fn signature(&self) -> Option<&str> {
        self.signature.as_deref()
    }

    /// Tags this index with the signature of the settings that produced its
    /// fingerprints (see
    /// [`Fingerprinter::signature`](crate::dsp::Fingerprinter::signature)).
    /// The tag is written by [`Index::save_to_disk`] and checked when the
    /// database is reopened.
    pub fn set_signature(&mut self, signature: impl Into<String>) {
        self.signature = Some(signature.into());
    }

    /// Returns a reference to the index configuration.
    #[must_use]
    pub fn config(&self) -> &IndexConfig {
        &self.config
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
    ///
    /// # Examples
    ///
    /// ```
    /// use wavio::hash::Fingerprint;
    /// use wavio::index::Index;
    ///
    /// let mut index = Index::default();
    /// let fps = vec![
    ///     Fingerprint::new(12345, 0.0),
    ///     Fingerprint::new(67890, 0.5),
    /// ];
    /// index.insert("my_song", &fps);
    ///
    /// assert_eq!(index.track_count(), 1);
    /// ```
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
    /// 4. Each track is scored by its tallest pair of adjacent bins; the
    ///    highest-scoring track wins (ties broken deterministically).
    ///
    /// Returns `None` if no matching hashes are found.
    ///
    /// # Examples
    ///
    /// ```
    /// use wavio::hash::Fingerprint;
    /// use wavio::index::Index;
    ///
    /// let mut index = Index::default();
    /// let fps = vec![
    ///     Fingerprint::new(12345, 0.0),
    ///     Fingerprint::new(67890, 0.5),
    /// ];
    /// index.insert("my_song", &fps);
    ///
    /// let query_fps = vec![
    ///     Fingerprint::new(12345, 0.0),
    /// ];
    /// let result = index.query(&query_fps).unwrap();
    /// assert_eq!(result.track_id, "my_song");
    /// assert_eq!(result.score, 1);
    /// ```
    #[must_use]
    pub fn query(&self, fingerprints: &[Fingerprint]) -> Option<QueryResult> {
        self.query_topn(fingerprints, 1).into_iter().next()
    }

    /// Returns whether a track with the given name has already been indexed.
    ///
    /// # Examples
    ///
    /// ```
    /// use wavio::hash::Fingerprint;
    /// use wavio::index::Index;
    ///
    /// let mut index = Index::default();
    /// assert!(!index.contains_track("my_song"));
    /// index.insert("my_song", &[Fingerprint::new(1, 0.0)]);
    /// assert!(index.contains_track("my_song"));
    /// ```
    #[must_use]
    pub fn contains_track(&self, name: &str) -> bool {
        self.tracks.name_to_id.contains_key(name)
    }

    /// Like [`Index::query`], but returns `None` unless the best match's
    /// `confidence` is at least `min_confidence`.
    ///
    /// Useful for rejecting weak/spurious matches (e.g. a short or noisy
    /// query clip that barely edges out unrelated tracks).
    ///
    /// # Examples
    ///
    /// ```
    /// use wavio::hash::Fingerprint;
    /// use wavio::index::Index;
    ///
    /// let mut index = Index::default();
    /// index.insert("song_a", &[Fingerprint::new(1, 0.0), Fingerprint::new(2, 0.1)]);
    ///
    /// // Only one of two hashes matches -> confidence 0.5.
    /// let query = vec![Fingerprint::new(1, 0.0), Fingerprint::new(999, 0.1)];
    /// assert!(index.query_with_min_confidence(&query, 0.9).is_none());
    /// assert!(index.query_with_min_confidence(&query, 0.4).is_some());
    /// ```
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
    /// Unlike [`Index::query`], which returns only the single best-matching
    /// track, this considers every track's own best histogram bin and
    /// returns them sorted by `score` descending. Useful when scores are
    /// close or callers want to inspect runner-up candidates.
    ///
    /// # Examples
    ///
    /// ```
    /// use wavio::hash::Fingerprint;
    /// use wavio::index::Index;
    ///
    /// let mut index = Index::default();
    /// index.insert("song_a", &[Fingerprint::new(1, 0.0), Fingerprint::new(2, 0.1)]);
    /// index.insert("song_b", &[Fingerprint::new(1, 0.0)]);
    ///
    /// let query = vec![Fingerprint::new(1, 0.0), Fingerprint::new(2, 0.1)];
    /// let results = index.query_topn(&query, 2);
    /// assert_eq!(results.len(), 2);
    /// assert_eq!(results[0].track_id, "song_a");
    /// assert_eq!(results[1].track_id, "song_b");
    /// ```
    #[must_use]
    pub fn query_topn(&self, fingerprints: &[Fingerprint], n: usize) -> Vec<QueryResult> {
        self.query_with(fingerprints, n, &QueryOptions::default())
    }

    /// Like [`Index::query`], but searches a grid of playback-speed
    /// hypotheses (see [`ScaleSearch`]) so a sped-up or slowed-down clip
    /// keeps its votes together. The result's `speed` is the estimated
    /// factor.
    ///
    /// # Examples
    ///
    /// ```
    /// use wavio::hash::Fingerprint;
    /// use wavio::index::{Index, ScaleSearch};
    ///
    /// let mut index = Index::default();
    /// let track: Vec<_> = (0..60).map(|i| Fingerprint::new(1000 + i, i as f32 * 0.1)).collect();
    /// index.insert("song", &track);
    ///
    /// // The same clip played 5% faster: query times are 1/1.05 of the originals.
    /// let query: Vec<_> = track
    ///     .iter()
    ///     .map(|f| Fingerprint::new(f.hash, f.anchor_time / 1.05))
    ///     .collect();
    /// let r = index.query_scaled(&query, &ScaleSearch::default()).unwrap();
    /// assert_eq!(r.track_id, "song");
    /// assert!((r.speed - 1.05).abs() < 0.011);
    /// ```
    #[must_use]
    pub fn query_scaled(
        &self,
        fingerprints: &[Fingerprint],
        search: &ScaleSearch,
    ) -> Option<QueryResult> {
        self.query_topn_scaled(fingerprints, 1, search)
            .into_iter()
            .next()
    }

    /// Top-`n` variant of [`Index::query_scaled`].
    #[must_use]
    pub fn query_topn_scaled(
        &self,
        fingerprints: &[Fingerprint],
        n: usize,
        search: &ScaleSearch,
    ) -> Vec<QueryResult> {
        self.query_with(
            fingerprints,
            n,
            &QueryOptions::default().with_scale_search(*search),
        )
    }

    /// Queries the index with explicit [`QueryOptions`] (rarity weighting,
    /// time-scale search) and returns up to `n` ranked matches, best first.
    ///
    /// # Examples
    ///
    /// ```
    /// use wavio::hash::Fingerprint;
    /// use wavio::index::{Index, QueryOptions};
    ///
    /// let mut index = Index::default();
    /// index.insert("a", &[Fingerprint::new(1, 0.0), Fingerprint::new(2, 0.1)]);
    /// let q = [Fingerprint::new(1, 0.0), Fingerprint::new(2, 0.1)];
    /// let r = index.query_with(&q, 1, &QueryOptions::new().with_idf_weighting(true));
    /// assert_eq!(r[0].track_id, "a");
    /// assert_eq!(r[0].score, 2);
    /// ```
    #[must_use]
    pub fn query_with(
        &self,
        fingerprints: &[Fingerprint],
        n: usize,
        options: &QueryOptions,
    ) -> Vec<QueryResult> {
        let mut tally = VoteTally::new(self.config.offset_bin_size, options);
        for fp in fingerprints {
            if let Some(entries) = self.table.get(&fp.hash) {
                for &(track_id, db_time) in entries {
                    tally.add(track_id, db_time, fp.anchor_time, entries.len());
                }
            }
        }
        tally.rank(n, fingerprints.len(), |tid| {
            self.tracks.name(tid).map(str::to_owned)
        })
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
    pub fn save_to_disk<P: AsRef<std::path::Path>>(
        &self,
        path: P,
    ) -> Result<(), crate::error::WavioError> {
        use crate::persist::PersistentIndex;

        let mut persistent = PersistentIndex::open(&path)?;
        if let Some(signature) = &self.signature {
            persistent.ensure_signature(signature)?;
        }
        persistent.set_config(self.config.clone())?;

        // One pass over the table, grouping entries by track.
        let mut by_track: HashMap<TrackId, Vec<Fingerprint>> = HashMap::new();
        for (hash, entries) in &self.table {
            for &(tid, anchor_time) in entries {
                by_track.entry(tid).or_default().push(Fingerprint {
                    hash: *hash,
                    anchor_time,
                });
            }
        }

        let mut batch: Vec<(&str, Vec<Fingerprint>)> = Vec::with_capacity(by_track.len());
        for (tid, fps) in by_track {
            if let Some(name) = self.tracks.name(tid) {
                batch.push((name, fps));
            }
        }
        // Tracks with no fingerprints still get registered.
        for name in self.tracks.id_to_name.values() {
            if !batch.iter().any(|(n, _)| n == name) {
                batch.push((name.as_str(), Vec::new()));
            }
        }

        persistent.insert_many(&batch)?;
        persistent.flush()
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
    pub fn load_from_disk<P: AsRef<std::path::Path>>(
        path: P,
    ) -> Result<Self, crate::error::WavioError> {
        use crate::persist::PersistentIndex;

        let persistent = PersistentIndex::open(path)?;
        persistent.load_into_memory()
    }

    /// Inserts a batch of already-fingerprinted tracks into the index.
    ///
    /// Behaves identically to calling [`Index::insert`] in a loop. Fingerprint
    /// *computation* is the CPU-heavy, per-track-independent part of indexing
    /// and is where parallelism pays off -- compute each track's fingerprints
    /// in parallel (see `wavio-cli`'s `index` subcommand), then hand the
    /// results to this method, since insertion itself is too cheap to
    /// benefit from a thread pool.
    pub fn insert_batch(&mut self, batch: &[(String, Vec<Fingerprint>)]) {
        for (name, fps) in batch {
            self.insert(name, fps);
        }
    }

    /// Serial alias of [`Index::insert_batch`], kept for compatibility.
    #[cfg(feature = "parallel")]
    #[deprecated(
        since = "0.4.0",
        note = "this never ran in parallel; use `Index::insert_batch`"
    )]
    pub fn insert_batch_parallel(&mut self, batch: &[(String, Vec<Fingerprint>)]) {
        self.insert_batch(batch);
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
        let query_fps: Vec<Fingerprint> =
            (0..50).map(|i| fp(50_000 + i, i as f32 * 0.02)).collect();

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
        let track_fps: Vec<Fingerprint> = (0..20).map(|i| fp(i + 100, i as f32 * 0.05)).collect();
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

        let track_fps: Vec<Fingerprint> = (0..100).map(|i| fp(i + 1000, i as f32 * 0.01)).collect();
        index.insert("song_a", &track_fps);

        // Query with only 30 of the 100 hashes (partial clip).
        let query_fps: Vec<Fingerprint> = (20..50).map(|i| fp(i + 1000, i as f32 * 0.01)).collect();

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
        let fps: Vec<Fingerprint> = (0..20).map(|i| fp(i + 500, i as f32 * 0.03)).collect();
        index.insert("song_a", &fps);

        let result = index.query(&fps).unwrap();
        assert_eq!(result.track_id, "song_a");
        assert_eq!(result.score, 20);
    }

    // ---- Scoring: margin, ties, boundary-straddling jitter ----

    #[test]
    fn test_margin_reflects_lead_over_runner_up() {
        let mut index = Index::default();
        let full: Vec<_> = (0..10).map(|i| fp(i, i as f32 * 0.1)).collect();
        index.insert("strong", &full);
        index.insert("weak", &full[..4]);

        let r = index.query(&full).unwrap();
        assert_eq!(r.track_id, "strong");
        assert_eq!((r.score, r.confidence), (10, 1.0));
        assert!((r.margin - 0.6).abs() < 1e-6, "margin = {}", r.margin);

        let top = index.query_topn(&full, 2);
        assert_eq!(top[0].track_id, "strong");
        assert_eq!(top[1].track_id, "weak");
        assert!((top[1].margin - 1.0).abs() < 1e-6); // nothing behind it
    }

    #[test]
    fn test_sole_candidate_has_full_margin() {
        let mut index = Index::default();
        index.insert("only", &[fp(1, 0.0)]);
        assert!((index.query(&[fp(1, 0.0)]).unwrap().margin - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_exact_tie_has_zero_margin_and_is_deterministic() {
        let mut index = Index::default();
        let fps = [fp(1, 0.0), fp(2, 0.1)];
        index.insert("b_track", &fps);
        index.insert("a_track", &fps);
        for _ in 0..20 {
            let r = index.query(&fps).unwrap();
            assert_eq!(r.track_id, "b_track"); // lower TrackId wins ties
            assert!(r.margin.abs() < 1e-6);
        }
    }

    #[test]
    fn test_votes_split_across_bin_boundary_still_count_together() {
        // Offsets of +0.024 and +0.026 fall into different 50 ms bins
        // (0 and 1). Jitter like this must not halve the score.
        let mut index = Index::default();
        let db: Vec<_> = (0..10)
            .map(|i| fp(100 + i, i as f32 * 0.3 + 0.025))
            .collect();
        index.insert("song", &db);
        let query: Vec<_> = (0..10)
            .map(|i| {
                fp(
                    100 + i,
                    i as f32 * 0.3 + if i % 2 == 0 { 0.001 } else { -0.001 },
                )
            })
            .collect();
        let r = index.query(&query).unwrap();
        assert_eq!(r.score, 10);
        assert!(
            (r.offset_secs - 0.025).abs() < 0.01,
            "offset = {}",
            r.offset_secs
        );
    }

    #[test]
    fn test_topn_zero_and_empty() {
        let mut index = Index::default();
        index.insert("a", &[fp(1, 0.0)]);
        assert!(index.query_topn(&[fp(1, 0.0)], 0).is_empty());
        assert!(index.query_topn(&[], 3).is_empty());
    }

    #[test]
    fn test_signature_round_trip_in_memory() {
        let mut index = Index::default();
        assert_eq!(index.signature(), None);
        index.set_signature("v1;mode=pairwise");
        assert_eq!(index.signature(), Some("v1;mode=pairwise"));
    }

    // ---- Time-scale-aware voting ----

    #[test]
    fn test_scaled_query_recovers_speed_and_offset() {
        let mut index = Index::default();
        let track: Vec<_> = (0..200).map(|i| fp(500 + i, i as f32 * 0.08)).collect();
        index.insert("song", &track);
        index.insert(
            "decoy",
            &(0..200)
                .map(|i| fp(9000 + i, i as f32 * 0.08))
                .collect::<Vec<_>>(),
        );

        // Clip starts 2 s into the song and is played 6% fast: db = 1.06*q + 2.
        let query: Vec<_> = track
            .iter()
            .filter(|f| f.anchor_time >= 2.0 && f.anchor_time < 8.0)
            .map(|f| fp(f.hash, (f.anchor_time - 2.0) / 1.06))
            .collect();

        let plain = index.query(&query).unwrap();
        let scaled = index.query_scaled(&query, &ScaleSearch::default()).unwrap();
        assert_eq!(scaled.track_id, "song");
        assert!(
            (scaled.speed - 1.06).abs() < 0.006,
            "speed = {}",
            scaled.speed
        );
        assert!(
            (scaled.offset_secs - 2.0).abs() < 0.1,
            "offset = {}",
            scaled.offset_secs
        );
        assert!(
            scaled.score > plain.score,
            "{} vs {}",
            scaled.score,
            plain.score
        );
        assert!((plain.speed - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_scaled_query_does_not_invent_speed_for_unscaled_clips() {
        let mut index = Index::default();
        let track: Vec<_> = (0..100).map(|i| fp(i + 1, i as f32 * 0.1)).collect();
        index.insert("song", &track);
        let r = index.query_scaled(&track, &ScaleSearch::default()).unwrap();
        assert!((r.speed - 1.0).abs() < f32::EPSILON, "speed = {}", r.speed);
        assert_eq!(r.score, 100);
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
