use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::dsp::audio::load_wav;
use crate::dsp::peaks::{extract_peaks, PeakExtractorConfig};
use crate::dsp::spectrogram::{compute_spectrogram, SpectrogramConfig};
use crate::hash::{generate_hashes, Fingerprint, HashConfig};
use crate::index::Index;

/// A class for extracting audio fingerprints from a file.
#[pyclass]
pub struct PyFingerprinter;

#[pymethods]
impl PyFingerprinter {
    #[new]
    fn new() -> Self {
        PyFingerprinter
    }

    /// Extends a file into a list of (hash, time_offset) pairs.
    /// Releases the GIL during computation.
    fn fingerprint_file(&self, py: Python, path: &str) -> PyResult<Vec<(u64, f32)>> {
        let path_owned = path.to_string();
        
        let fingerprints = py.allow_threads(move || -> PyResult<Vec<Fingerprint>> {
            let audio_result = load_wav(&path_owned);
            let audio = audio_result.map_err(|e| {
                pyo3::exceptions::PyIOError::new_err(format!("Failed to load audio: {}", e))
            })?;

            let spec_config = SpectrogramConfig::default();
            let spec = compute_spectrogram(&audio.samples, &spec_config).map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!("Spectrogram error: {}", e))
            })?;

            let peak_config = PeakExtractorConfig::default();
            let peaks = extract_peaks(&spec, &peak_config);

            let hash_config = HashConfig::default();
            let hashes = generate_hashes(&peaks, &hash_config);

            Ok(hashes)
        })?;

        // Convert Fingerprint structs to Python tuples
        Ok(fingerprints
            .into_iter()
            .map(|fp| (fp.hash, fp.anchor_time))
            .collect())
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
    fn new() -> Self {
        PyIndex {
            inner: Index::default(),
        }
    }

    /// Load an index from a persistent database file.
    #[staticmethod]
    #[cfg(feature = "persist")]
    fn load(path: &str) -> PyResult<Self> {
        let index = Index::load_from_disk(path).map_err(|e| {
            pyo3::exceptions::PyIOError::new_err(format!("Failed to load database: {}", e))
        })?;
        Ok(PyIndex { inner: index })
    }

    /// Save the current index to a persistent database file.
    #[cfg(feature = "persist")]
    fn save(&self, path: &str) -> PyResult<()> {
        self.inner.save_to_disk(path).map_err(|e| {
            pyo3::exceptions::PyIOError::new_err(format!("Failed to save database: {}", e))
        })?;
        Ok(())
    }

    /// Insert fingerprints associated with a specific track ID/name.
    fn insert(&mut self, track_id: &str, fingerprints: Vec<(u64, f32)>) {
        let fps: Vec<Fingerprint> = fingerprints
            .into_iter()
            .map(|(hash, anchor_time)| Fingerprint { hash, anchor_time })
            .collect();
        self.inner.insert(track_id, &fps);
    }

    /// Query the index using a list of fingerprints. Returns a dict on match, or None.
    fn query<'py>(&self, py: Python<'py>, fingerprints: Vec<(u64, f32)>) -> Option<Bound<'py, PyDict>> {
        let fps: Vec<Fingerprint> = fingerprints
            .into_iter()
            .map(|(hash, anchor_time)| Fingerprint { hash, anchor_time })
            .collect();

        if let Some(result) = self.inner.query(&fps) {
            let dict = PyDict::new(py);
            dict.set_item("track_id", result.track_id).unwrap();
            dict.set_item("score", result.score).unwrap();
            dict.set_item("offset_secs", result.offset_secs).unwrap();
            Some(dict)
        } else {
            None
        }
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
    Ok(())
}
