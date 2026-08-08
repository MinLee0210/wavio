use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::dsp::Fingerprinter;
use crate::hash::Fingerprint;
use crate::index::Index;

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
    ) -> Self {
        use crate::dsp::peaks::PeakExtractorConfig;
        use crate::dsp::spectrogram::SpectrogramConfig;
        use crate::hash::HashConfig;

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

        PyFingerprinter {
            inner: Fingerprinter::new(spectrogram_config, peak_config, hash_config),
        }
    }

    /// Extends a file into a list of (hash, time_offset) pairs.
    /// Releases the GIL during computation.
    fn fingerprint_file(&self, py: Python, path: &str) -> PyResult<Vec<(u64, f32)>> {
        let path_owned = path.to_string();
        let fingerprinter = self.inner.clone();
        
        let fingerprints = py.allow_threads(move || -> PyResult<Vec<Fingerprint>> {
            let hashes = fingerprinter.fingerprint_file(&path_owned).map_err(|e| {
                pyo3::exceptions::PyIOError::new_err(format!("Fingerprint pipeline error: {e}"))
            })?;
            Ok(hashes)
        })?;

        // Convert Fingerprint structs to Python tuples
        Ok(fingerprints
            .into_iter()
            .map(|fp| (fp.hash, fp.anchor_time))
            .collect())
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
        let index = Index::load_from_disk(path).map_err(|e| {
            pyo3::exceptions::PyIOError::new_err(format!("Failed to load database: {e}"))
        })?;
        Ok(PyIndex { inner: index })
    }

    /// Save the current index to a persistent database file.
    #[cfg(feature = "persist")]
    fn save(&self, path: &str) -> PyResult<()> {
        self.inner.save_to_disk(path).map_err(|e| {
            pyo3::exceptions::PyIOError::new_err(format!("Failed to save database: {e}"))
        })?;
        Ok(())
    }

    /// Insert fingerprints associated with a specific track ID/name.
    fn insert(&mut self, track_id: &str, fingerprints: Vec<(u64, f32)>) {
        let fps: Vec<Fingerprint> = fingerprints
            .into_iter()
            .map(|(hash, anchor_time)| Fingerprint::new(hash, anchor_time))
            .collect();
        self.inner.insert(track_id, &fps);
    }

    /// Query the index using a list of fingerprints. Returns a dict on match, or None.
    fn query<'py>(&self, py: Python<'py>, fingerprints: Vec<(u64, f32)>) -> Option<Bound<'py, PyDict>> {
        let fps: Vec<Fingerprint> = fingerprints
            .into_iter()
            .map(|(hash, anchor_time)| Fingerprint::new(hash, anchor_time))
            .collect();

        if let Some(result) = self.inner.query(&fps) {
            let dict = PyDict::new(py);
            dict.set_item("track_id", result.track_id).unwrap();
            dict.set_item("score", result.score).unwrap();
            dict.set_item("offset_secs", result.offset_secs).unwrap();
            dict.set_item("confidence", result.confidence).unwrap();
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

    #[getter]
    fn offset_bin_size(&self) -> f32 {
        self.inner.config().offset_bin_size
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
