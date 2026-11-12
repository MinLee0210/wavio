//! Python bindings (feature `python`).
//!
//! Fingerprints cross the boundary as plain `list[tuple[int, float]]`
//! (`(hash, anchor_time_secs)`), so they can be stored, pickled and mixed
//! freely. Audio goes in as any `float32` buffer (a NumPy array,
//! `array.array('f')`, ...) without copying through Python objects, or as a
//! plain list of floats.

// PyO3 hands arguments over by value (`PyRef`, `Vec`); taking references would
// not compile for `#[pymethods]`.
#![allow(clippy::needless_pass_by_value)]

use pyo3::buffer::PyBuffer;
use pyo3::exceptions::{PyIOError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::dsp::Fingerprinter;
use crate::dsp::streaming::{StreamConfig, StreamingFingerprinter};
use crate::error::WavioError;
use crate::hash::Fingerprint;
use crate::index::{Index, QueryOptions, QueryResult, ScaleSearch};

/// Converts a [`QueryResult`] into a Python dict with the same shape as
/// [`PyIndex::query`]'s return value.
fn query_result_to_dict<'py>(py: Python<'py>, result: &QueryResult) -> Bound<'py, PyDict> {
    let dict = PyDict::new(py);
    let _ = dict.set_item("track_id", &result.track_id);
    let _ = dict.set_item("score", result.score);
    let _ = dict.set_item("offset_secs", result.offset_secs);
    let _ = dict.set_item("confidence", result.confidence);
    let _ = dict.set_item("margin", result.margin);
    let _ = dict.set_item("speed", result.speed);
    let _ = dict.set_item("weighted_score", result.weighted_score);
    dict
}

fn to_fingerprints(pairs: Vec<(u64, f32)>) -> Vec<Fingerprint> {
    pairs
        .into_iter()
        .map(|(hash, anchor_time)| Fingerprint::new(hash, anchor_time))
        .collect()
}

fn from_fingerprints(fps: Vec<Fingerprint>) -> Vec<(u64, f32)> {
    fps.into_iter()
        .map(|fp| (fp.hash, fp.anchor_time))
        .collect()
}

/// Incompatible settings are a caller mistake (`ValueError`); everything else
/// is an environment problem (`IOError`).
fn to_py_err(context: &str, e: &WavioError) -> PyErr {
    match e {
        WavioError::IncompatibleIndex(_) => PyValueError::new_err(format!("{context}: {e}")),
        _ => PyIOError::new_err(format!("{context}: {e}")),
    }
}

/// Reads audio samples from a `float32` buffer (`NumPy` array, `array('f')`) or
/// any sequence of floats.
fn extract_samples(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Vec<f32>> {
    if let Ok(buf) = PyBuffer::<f32>::get(obj) {
        return buf.to_vec(py);
    }
    obj.extract::<Vec<f32>>()
}

fn query_options(idf: bool, scale: Option<(f32, f32)>) -> QueryOptions {
    let mut options = QueryOptions::new().with_idf_weighting(idf);
    if let Some((max_deviation, step)) = scale {
        options = options.with_scale_search(ScaleSearch::new(max_deviation, step, 5));
    }
    options
}

/// A class for extracting audio fingerprints from a file.
#[pyclass]
pub struct PyFingerprinter {
    inner: Fingerprinter,
}

#[pymethods]
impl PyFingerprinter {
    #[new]
    #[pyo3(signature = (
        window_size = 2048,
        hop_size = 512,
        time_neighborhood = 10,
        freq_neighborhood = 10,
        threshold_db = -40.0,
        min_dt = 0.0,
        max_dt = 1.0,
        freq_bins = 1024,
        freq_resolution = 22050.0 / 2048.0,
        dt_resolution = 0.01,
        fan_value = 15,
        mode = "pairwise",
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        window_size: usize,
        hop_size: usize,
        time_neighborhood: usize,
        freq_neighborhood: usize,
        threshold_db: f32,
        min_dt: f32,
        max_dt: f32,
        freq_bins: u32,
        freq_resolution: f32,
        dt_resolution: f32,
        fan_value: usize,
        mode: &str,
    ) -> PyResult<Self> {
        use crate::dsp::peaks::PeakExtractorConfig;
        use crate::dsp::spectrogram::SpectrogramConfig;
        use crate::hash::HashConfig;
        use crate::triplet::TripletHashConfig;

        let spectrogram_config = SpectrogramConfig::new(window_size, hop_size);
        let peak_config = PeakExtractorConfig::new(
            time_neighborhood,
            freq_neighborhood,
            threshold_db,
            22_050, // standard sample rate
            hop_size,
            window_size,
        );
        let hash_config = HashConfig::new(
            fan_value,
            min_dt,
            max_dt,
            freq_bins,
            freq_resolution,
            dt_resolution,
        );

        let base = Fingerprinter::new(spectrogram_config, peak_config, hash_config);
        let inner = match mode {
            "pairwise" => base,
            "triplet" => base
                .with_triplet_hashing(TripletHashConfig::default())
                .with_peak_interpolation(true),
            "hybrid" => base.with_hybrid_hashing(TripletHashConfig::default()),
            other => {
                return Err(pyo3::exceptions::PyValueError::new_err(format!(
                    "unknown mode {other:?}; expected \"pairwise\", \"triplet\" or \"hybrid\""
                )));
            }
        };
        Ok(PyFingerprinter { inner })
    }

    /// The hashing mode: "pairwise", "triplet" or "hybrid".
    #[getter]
    fn mode(&self) -> &'static str {
        match (&self.inner.triplet_config, self.inner.hybrid) {
            (None, _) => "pairwise",
            (Some(_), true) => "hybrid",
            (Some(_), false) => "triplet",
        }
    }

    /// A description of every setting that affects the hashes. Two
    /// fingerprinters with different signatures produce incomparable hashes.
    #[getter]
    fn signature(&self) -> String {
        self.inner.signature()
    }

    /// Fingerprints a file into a list of `(hash, anchor_time)` pairs.
    /// Releases the GIL during computation.
    fn fingerprint_file(&self, py: Python, path: &str) -> PyResult<Vec<(u64, f32)>> {
        let path_owned = path.to_string();
        let fingerprinter = self.inner.clone();

        let fingerprints = py.allow_threads(move || -> PyResult<Vec<Fingerprint>> {
            fingerprinter
                .fingerprint_file(&path_owned)
                .map_err(|e| to_py_err("Fingerprint pipeline error", &e))
        })?;
        Ok(from_fingerprints(fingerprints))
    }

    /// Fingerprints in-memory audio: mono `float32` samples at 22,050 Hz,
    /// given as a `NumPy` array / `array('f')` (read without copying through
    /// Python objects) or a list of floats. Releases the GIL during
    /// computation.
    fn fingerprint_samples(
        &self,
        py: Python<'_>,
        samples: &Bound<'_, PyAny>,
    ) -> PyResult<Vec<(u64, f32)>> {
        let samples = extract_samples(py, samples)?;
        let fingerprinter = self.inner.clone();
        let fingerprints = py.allow_threads(move || {
            fingerprinter
                .fingerprint(&samples)
                .map_err(|e| to_py_err("Fingerprint pipeline error", &e))
        })?;
        Ok(from_fingerprints(fingerprints))
    }

    #[getter]
    fn window_size(&self) -> usize {
        self.inner.spectrogram_config.window_size
    }

    #[getter]
    fn hop_size(&self) -> usize {
        self.inner.spectrogram_config.hop_size
    }

    #[getter]
    fn time_neighborhood(&self) -> usize {
        self.inner.peak_config.time_neighborhood
    }

    #[getter]
    fn freq_neighborhood(&self) -> usize {
        self.inner.peak_config.freq_neighborhood
    }

    #[getter]
    fn threshold_db(&self) -> f32 {
        self.inner.peak_config.threshold_db
    }

    #[getter]
    fn min_dt(&self) -> f32 {
        self.inner.hash_config.min_dt
    }

    #[getter]
    fn max_dt(&self) -> f32 {
        self.inner.hash_config.max_dt
    }

    #[getter]
    fn fan_value(&self) -> usize {
        self.inner.hash_config.fan_value
    }
}

/// An in-memory/persistent index for audio fingerprints.
#[pyclass]
pub struct PyIndex {
    inner: Index,
}

#[pymethods]
impl PyIndex {
    #[new]
    #[pyo3(signature = (offset_bin_size = 0.05))]
    fn new(offset_bin_size: f32) -> Self {
        use crate::index::IndexConfig;
        PyIndex {
            inner: Index::new(IndexConfig::new(offset_bin_size)),
        }
    }

    /// Load an index from a persistent database file.
    #[staticmethod]
    #[cfg(feature = "persist")]
    fn load(path: &str) -> PyResult<Self> {
        let index =
            Index::load_from_disk(path).map_err(|e| to_py_err("Failed to load database", &e))?;
        Ok(PyIndex { inner: index })
    }

    /// Save the current index to a persistent database file.
    #[cfg(feature = "persist")]
    fn save(&self, path: &str) -> PyResult<()> {
        self.inner
            .save_to_disk(path)
            .map_err(|e| to_py_err("Failed to save database", &e))
    }

    /// Insert fingerprints associated with a specific track ID/name.
    fn insert(&mut self, track_id: &str, fingerprints: Vec<(u64, f32)>) {
        self.inner.insert(track_id, &to_fingerprints(fingerprints));
    }

    /// Whether a track with this name is already indexed.
    fn contains_track(&self, track_id: &str) -> bool {
        self.inner.contains_track(track_id)
    }

    /// Query the index using a list of fingerprints. Returns a dict on match,
    /// or `None`. With `idf=True`, rare hashes outweigh common ones.
    #[pyo3(signature = (fingerprints, idf = false))]
    fn query<'py>(
        &self,
        py: Python<'py>,
        fingerprints: Vec<(u64, f32)>,
        idf: bool,
    ) -> Option<Bound<'py, PyDict>> {
        self.inner
            .query_with(&to_fingerprints(fingerprints), 1, &query_options(idf, None))
            .first()
            .map(|result| query_result_to_dict(py, result))
    }

    /// Like `query`, but also searches playback-speed hypotheses within
    /// +-`max_deviation` (default 8%) so sped-up / slowed-down clips keep
    /// their votes together. The result's `speed` is the estimated factor.
    #[pyo3(signature = (fingerprints, max_deviation = 0.08, step = 0.01, idf = false))]
    fn query_scaled<'py>(
        &self,
        py: Python<'py>,
        fingerprints: Vec<(u64, f32)>,
        max_deviation: f32,
        step: f32,
        idf: bool,
    ) -> Option<Bound<'py, PyDict>> {
        let options = query_options(idf, Some((max_deviation, step)));
        self.inner
            .query_with(&to_fingerprints(fingerprints), 1, &options)
            .first()
            .map(|result| query_result_to_dict(py, result))
    }

    /// Query the index and return up to `n` ranked matches (best first) as a
    /// list of dicts, using the same shape as `query`.
    #[pyo3(signature = (fingerprints, n, idf = false))]
    fn query_topn<'py>(
        &self,
        py: Python<'py>,
        fingerprints: Vec<(u64, f32)>,
        n: usize,
        idf: bool,
    ) -> Vec<Bound<'py, PyDict>> {
        self.inner
            .query_with(&to_fingerprints(fingerprints), n, &query_options(idf, None))
            .iter()
            .map(|result| query_result_to_dict(py, result))
            .collect()
    }

    /// Query the index using a list of fingerprints, rejecting any match
    /// whose confidence is below `min_confidence`. Returns a dict on match,
    /// or `None`.
    fn query_with_min_confidence<'py>(
        &self,
        py: Python<'py>,
        fingerprints: Vec<(u64, f32)>,
        min_confidence: f32,
    ) -> Option<Bound<'py, PyDict>> {
        self.inner
            .query_with_min_confidence(&to_fingerprints(fingerprints), min_confidence)
            .map(|result| query_result_to_dict(py, &result))
    }

    /// The settings signature this index was tagged with (see
    /// `PyFingerprinter.signature`), or `None`.
    #[getter]
    fn signature(&self) -> Option<String> {
        self.inner.signature().map(str::to_owned)
    }

    /// Tags the index with the signature of the settings that produced its
    /// fingerprints; it is saved with the index and checked on reopen.
    fn set_signature(&mut self, signature: &str) {
        self.inner.set_signature(signature);
    }

    #[getter]
    fn track_count(&self) -> usize {
        self.inner.track_count()
    }

    #[getter]
    fn hash_count(&self) -> usize {
        self.inner.hash_count()
    }

    #[getter]
    fn offset_bin_size(&self) -> f32 {
        self.inner.config().offset_bin_size
    }
}

/// Fingerprints an unbounded audio stream in overlapping blocks, so the whole
/// signal never has to be in memory (microphones, network streams, long files).
///
/// ```python
/// streamer = wavio.PyStreamingFingerprinter(wavio.PyFingerprinter())
/// hashes = []
/// for chunk in chunks:              # any float32 buffers, any sizes
///     hashes += streamer.push(chunk)
/// hashes += streamer.finish()
/// ```
#[pyclass]
pub struct PyStreamingFingerprinter {
    inner: StreamingFingerprinter,
}

#[pymethods]
impl PyStreamingFingerprinter {
    /// `overlap_secs` must be at least the hashing mode's longest pair
    /// distance (1.0 s by default); `ValueError` otherwise.
    #[new]
    #[pyo3(signature = (fingerprinter, block_duration_secs = 10.0, overlap_secs = 1.0))]
    fn new(
        fingerprinter: PyRef<'_, PyFingerprinter>,
        block_duration_secs: f32,
        overlap_secs: f32,
    ) -> PyResult<Self> {
        let config = StreamConfig::new(block_duration_secs, overlap_secs);
        let inner = StreamingFingerprinter::new(fingerprinter.inner.clone(), &config)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(Self { inner })
    }

    /// Feeds more samples; returns the fingerprints that became final.
    /// Their `anchor_time` is relative to the start of the whole stream.
    fn push(&mut self, py: Python<'_>, samples: &Bound<'_, PyAny>) -> PyResult<Vec<(u64, f32)>> {
        let samples = extract_samples(py, samples)?;
        let inner = &mut self.inner;
        let fps = py.allow_threads(|| {
            inner
                .push(&samples)
                .map_err(|e| to_py_err("Streaming error", &e))
        })?;
        Ok(from_fingerprints(fps))
    }

    /// Flushes the remaining buffered audio. Call once after the last `push`.
    fn finish(&mut self, py: Python<'_>) -> PyResult<Vec<(u64, f32)>> {
        let inner = &mut self.inner;
        let fps =
            py.allow_threads(|| inner.finish().map_err(|e| to_py_err("Streaming error", &e)))?;
        Ok(from_fingerprints(fps))
    }
}

/// An on-disk fingerprint index that answers queries straight from disk,
/// appends in O(log n), and remembers the settings it was built with.
///
/// ```python
/// db = wavio.PyPersistentIndex("library.redb")
/// db.ensure_fingerprinter(fingerprinter)   # ValueError on a settings mismatch
/// db.insert("song", fingerprinter.fingerprint_file("song.wav"))
/// db.query(fingerprinter.fingerprint_file("clip.wav"))
/// ```
#[cfg(feature = "persist")]
#[pyclass]
pub struct PyPersistentIndex {
    inner: crate::persist::PersistentIndex,
}

#[cfg(feature = "persist")]
#[pymethods]
impl PyPersistentIndex {
    /// Opens (creating if needed) the database at `path`. Databases from
    /// older versions are migrated; ones from a newer version raise
    /// `ValueError`.
    #[new]
    fn new(path: &str) -> PyResult<Self> {
        let inner = crate::persist::PersistentIndex::open(path)
            .map_err(|e| to_py_err("Failed to open database", &e))?;
        Ok(Self { inner })
    }

    /// Adds a track's fingerprints (one transaction).
    fn insert(&mut self, track_id: &str, fingerprints: Vec<(u64, f32)>) -> PyResult<()> {
        self.inner
            .insert(track_id, &to_fingerprints(fingerprints))
            .map_err(|e| to_py_err("Insert failed", &e))
    }

    /// Adds many `(track_id, fingerprints)` pairs in a single transaction.
    fn insert_many(&mut self, batch: Vec<(String, Vec<(u64, f32)>)>) -> PyResult<()> {
        let batch: Vec<(String, Vec<Fingerprint>)> = batch
            .into_iter()
            .map(|(name, fps)| (name, to_fingerprints(fps)))
            .collect();
        self.inner
            .insert_many(&batch)
            .map_err(|e| to_py_err("Insert failed", &e))
    }

    /// Removes a track and its fingerprints; returns whether it existed.
    fn remove_track(&mut self, track_id: &str) -> PyResult<bool> {
        self.inner
            .remove_track(track_id)
            .map_err(|e| to_py_err("Remove failed", &e))
    }

    /// Whether a track with this name is indexed.
    fn contains_track(&self, track_id: &str) -> bool {
        self.inner.contains_track(track_id)
    }

    /// Best match as a dict, or `None`. Releases the GIL while reading.
    #[pyo3(signature = (fingerprints, idf = false))]
    fn query<'py>(
        &self,
        py: Python<'py>,
        fingerprints: Vec<(u64, f32)>,
        idf: bool,
    ) -> Option<Bound<'py, PyDict>> {
        let fps = to_fingerprints(fingerprints);
        let options = query_options(idf, None);
        let results = py.allow_threads(|| self.inner.query_with(&fps, 1, &options));
        results.first().map(|r| query_result_to_dict(py, r))
    }

    /// Like `query`, with playback-speed search; `speed` holds the estimate.
    #[pyo3(signature = (fingerprints, max_deviation = 0.08, step = 0.01, idf = false))]
    fn query_scaled<'py>(
        &self,
        py: Python<'py>,
        fingerprints: Vec<(u64, f32)>,
        max_deviation: f32,
        step: f32,
        idf: bool,
    ) -> Option<Bound<'py, PyDict>> {
        let fps = to_fingerprints(fingerprints);
        let options = query_options(idf, Some((max_deviation, step)));
        let results = py.allow_threads(|| self.inner.query_with(&fps, 1, &options));
        results.first().map(|r| query_result_to_dict(py, r))
    }

    /// Up to `n` ranked matches, best first.
    #[pyo3(signature = (fingerprints, n, idf = false))]
    fn query_topn<'py>(
        &self,
        py: Python<'py>,
        fingerprints: Vec<(u64, f32)>,
        n: usize,
        idf: bool,
    ) -> Vec<Bound<'py, PyDict>> {
        let fps = to_fingerprints(fingerprints);
        let options = query_options(idf, None);
        let results = py.allow_threads(|| self.inner.query_with(&fps, n, &options));
        results
            .iter()
            .map(|r| query_result_to_dict(py, r))
            .collect()
    }

    /// The stored settings signature, or `None` for a new / legacy database.
    #[getter]
    fn signature(&self) -> Option<String> {
        self.inner.signature()
    }

    /// Records the fingerprinter's settings on first use, and raises
    /// `ValueError` if the database was built with different ones.
    fn ensure_fingerprinter(&mut self, fingerprinter: PyRef<'_, PyFingerprinter>) -> PyResult<()> {
        self.inner
            .ensure_signature(&fingerprinter.inner.signature())
            .map_err(|e| to_py_err("Incompatible fingerprinter", &e))
    }

    /// Like `ensure_fingerprinter` but never writes to the database.
    fn check_fingerprinter(&self, fingerprinter: PyRef<'_, PyFingerprinter>) -> PyResult<()> {
        self.inner
            .check_signature(&fingerprinter.inner.signature())
            .map_err(|e| to_py_err("Incompatible fingerprinter", &e))
    }

    #[getter]
    fn track_count(&self) -> usize {
        self.inner.track_count()
    }

    #[getter]
    fn hash_count(&self) -> usize {
        self.inner.hash_count()
    }
}

/// A Python module implemented in Rust. The name of this function must match
/// the `lib.name` setting in the `Cargo.toml`, else Python will not be able to
/// import the module.
#[pymodule]
fn wavio(_py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyFingerprinter>()?;
    m.add_class::<PyIndex>()?;
    m.add_class::<PyStreamingFingerprinter>()?;
    #[cfg(feature = "persist")]
    m.add_class::<PyPersistentIndex>()?;
    Ok(())
}
