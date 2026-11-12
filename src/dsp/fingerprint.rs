//! High-level audio fingerprinter pipeline coordination.

use crate::dsp::audio::load_audio;
use crate::dsp::peaks::PeakExtractorConfig;
#[cfg(not(feature = "parallel"))]
use crate::dsp::peaks::extract_peaks;
#[cfg(feature = "parallel")]
use crate::dsp::peaks::extract_peaks_parallel;

use crate::dsp::spectrogram::{SpectrogramConfig, compute_spectrogram};

use crate::error::WavioError;
#[cfg(not(feature = "parallel"))]
use crate::hash::generate_hashes;
#[cfg(feature = "parallel")]
use crate::hash::generate_hashes_parallel;
use crate::hash::{Fingerprint, HashConfig};
#[cfg(not(feature = "parallel"))]
use crate::triplet::generate_triplet_hashes;
#[cfg(feature = "parallel")]
use crate::triplet::generate_triplet_hashes_parallel;
use crate::triplet::{TRIPLET_TAG, TripletHashConfig};

/// High-level orchestration engine for the audio fingerprinting pipeline.
///
/// Combines configurations for spectrogram generation, peak extraction,
/// and combinatorial hashing. Offers a single entry point to extract
/// fingerprints from samples in-memory or directly from files on disk.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Fingerprinter {
    /// Configuration for spectrogram generation.
    pub spectrogram_config: SpectrogramConfig,
    /// Configuration for peak detection.
    pub peak_config: PeakExtractorConfig,
    /// Configuration for combinatorial hashing.
    pub hash_config: HashConfig,
    /// When set, fingerprinting uses pitch-shift / time-stretch-robust
    /// triplet hashing (see [`crate::triplet`]) instead of the default
    /// pairwise hashing. Set this via [`Fingerprinter::with_triplet_hashing`].
    pub triplet_config: Option<TripletHashConfig>,
    /// When `true` (and `triplet_config` is set), **both** hash families are
    /// emitted and indexed together; see [`Fingerprinter::with_hybrid_hashing`].
    pub hybrid: bool,
}

impl Fingerprinter {
    /// Creates a new `Fingerprinter` with custom configurations.
    ///
    /// Uses the default (pairwise) hashing mode; call
    /// [`Fingerprinter::with_triplet_hashing`] to opt into the robust mode.
    #[must_use]
    pub fn new(
        spectrogram_config: SpectrogramConfig,
        peak_config: PeakExtractorConfig,
        hash_config: HashConfig,
    ) -> Self {
        Self {
            spectrogram_config,
            peak_config,
            hash_config,
            triplet_config: None,
            hybrid: false,
        }
    }

    /// Emits **both** pairwise and triplet hashes for every clip (triplet
    /// hashes are tagged with [`TRIPLET_TAG`] so the families never collide),
    /// and enables sub-bin peak interpolation, which the triplet half needs.
    ///
    /// The two families fail in opposite situations: pairwise hashes are the
    /// most robust against additive noise but collapse under any speed or
    /// pitch change, while triplet hashes survive speed / pitch changes but
    /// are somewhat more fragile in heavy noise. Their votes land in the same
    /// offset bin of the same index, so whichever family survives carries the
    /// match. The cost is roughly 1.5x the hashes per track.
    #[must_use]
    pub fn with_hybrid_hashing(mut self, config: TripletHashConfig) -> Self {
        self.triplet_config = Some(config);
        self.hybrid = true;
        self.peak_config.interpolate = true;
        self
    }

    /// Switches this `Fingerprinter` to pitch-shift / time-stretch-robust
    /// triplet hashing, using the given configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use wavio::dsp::Fingerprinter;
    /// use wavio::triplet::TripletHashConfig;
    ///
    /// let fingerprinter = Fingerprinter::default()
    ///     .with_triplet_hashing(TripletHashConfig::default());
    /// assert!(fingerprinter.triplet_config.is_some());
    /// ```
    #[must_use]
    pub fn with_triplet_hashing(mut self, config: TripletHashConfig) -> Self {
        self.triplet_config = Some(config);
        self.hybrid = false;
        self
    }

    /// Enables or disables sub-bin peak interpolation
    /// (see [`PeakExtractorConfig::interpolate`]).
    #[must_use]
    pub fn with_peak_interpolation(mut self, interpolate: bool) -> Self {
        self.peak_config.interpolate = interpolate;
        self
    }

    /// Returns the maximum anchor-to-target time delta used by the active
    /// hashing mode (triplet if configured, otherwise pairwise).
    ///
    /// Used by [`crate::dsp::streaming::StreamingFingerprinter`] to size its
    /// block overlap so that no anchor's fan-out window is ever truncated by
    /// a block boundary.
    #[must_use]
    pub fn max_pair_dt(&self) -> f32 {
        match &self.triplet_config {
            Some(cfg) if self.hybrid => cfg.max_dt.max(self.hash_config.max_dt),
            Some(cfg) => cfg.max_dt,
            None => self.hash_config.max_dt,
        }
    }

    /// Returns a stable, human-readable description of every setting that
    /// influences the hashes this fingerprinter produces (hash mode, FFT and
    /// peak-picking parameters, hash parameters).
    ///
    /// Two fingerprinters with equal signatures produce comparable hashes;
    /// anything else will silently fail to match. Stores such as
    /// [`PersistentIndex`](crate::persist::PersistentIndex) record this so a
    /// database built with one configuration cannot be queried (or extended)
    /// with another by accident.
    ///
    /// # Examples
    ///
    /// ```
    /// use wavio::dsp::Fingerprinter;
    /// use wavio::triplet::TripletHashConfig;
    ///
    /// let pairwise = Fingerprinter::default();
    /// let triplet = Fingerprinter::default().with_triplet_hashing(TripletHashConfig::default());
    /// assert_ne!(pairwise.signature(), triplet.signature());
    /// assert_eq!(pairwise.signature(), Fingerprinter::default().signature());
    /// ```
    #[must_use]
    pub fn signature(&self) -> String {
        let s = &self.spectrogram_config;
        let p = &self.peak_config;
        let common = format!(
            "sr={};rs=sinc1;win={};hop={};tn={};fn={};thr={};interp={}",
            p.sample_rate,
            s.window_size,
            s.hop_size,
            p.time_neighborhood,
            p.freq_neighborhood,
            p.threshold_db,
            u8::from(p.interpolate)
        );
        let pairwise = {
            let h = &self.hash_config;
            format!(
                "fan={};dt={}..{};bins={};fres={};dtres={}",
                h.fan_value, h.min_dt, h.max_dt, h.freq_bins, h.freq_resolution, h.dt_resolution
            )
        };
        match &self.triplet_config {
            Some(t) if self.hybrid => format!(
                "v1;mode=hybrid;{common};pair[{pairwise}];triplet[fan={};dt={}..{};fres={};tres={}]",
                t.fan_value, t.min_dt, t.max_dt, t.freq_ratio_resolution, t.time_ratio_resolution
            ),
            Some(t) => format!(
                "v1;mode=triplet;{common};fan={};dt={}..{};fres={};tres={}",
                t.fan_value, t.min_dt, t.max_dt, t.freq_ratio_resolution, t.time_ratio_resolution
            ),
            None => format!("v1;mode=pairwise;{common};{pairwise}"),
        }
    }

    /// Generates audio fingerprints from normalized mono samples in-memory.
    ///
    /// Depending on feature flags and compile options, this automatically
    /// uses parallel processing if the `parallel` feature is active.
    ///
    /// # Errors
    ///
    /// Returns [`WavioError`] if spectrogram computation fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use wavio::dsp::Fingerprinter;
    ///
    /// let fingerprinter = Fingerprinter::default();
    /// let samples = vec![0.0; 22050]; // 1 second of silence
    /// let fingerprints = fingerprinter.fingerprint(&samples).unwrap();
    /// // Silence should produce zero fingerprints
    /// assert!(fingerprints.is_empty());
    /// ```
    pub fn fingerprint(&self, samples: &[f32]) -> Result<Vec<Fingerprint>, WavioError> {
        let spec = compute_spectrogram(samples, &self.spectrogram_config)?;

        #[cfg(feature = "parallel")]
        let peaks = extract_peaks_parallel(&spec, &self.peak_config);
        #[cfg(not(feature = "parallel"))]
        let peaks = extract_peaks(&spec, &self.peak_config);

        let pairwise = || {
            #[cfg(feature = "parallel")]
            {
                generate_hashes_parallel(&peaks, &self.hash_config)
            }
            #[cfg(not(feature = "parallel"))]
            {
                generate_hashes(&peaks, &self.hash_config)
            }
        };
        let triplet = |cfg: &TripletHashConfig| {
            #[cfg(feature = "parallel")]
            {
                generate_triplet_hashes_parallel(&peaks, cfg)
            }
            #[cfg(not(feature = "parallel"))]
            {
                generate_triplet_hashes(&peaks, cfg)
            }
        };

        let hashes = match &self.triplet_config {
            Some(cfg) if self.hybrid => {
                let mut all = pairwise();
                all.extend(triplet(cfg).into_iter().map(|fp| Fingerprint {
                    hash: fp.hash | TRIPLET_TAG,
                    anchor_time: fp.anchor_time,
                }));
                all
            }
            Some(cfg) => triplet(cfg),
            None => pairwise(),
        };

        Ok(hashes)
    }

    /// Loads an audio file from disk, processes it, and generates its fingerprints.
    ///
    /// Automatically decodes any supported format (WAV, MP3, FLAC, AAC, etc.,
    /// depending on whether the `symphonia` feature is active) and resamples
    /// to the standard internal sample rate.
    ///
    /// # Errors
    ///
    /// Returns [`WavioError`] if reading, decoding, or processing fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use wavio::dsp::Fingerprinter;
    ///
    /// let fingerprinter = Fingerprinter::default();
    /// let fingerprints = fingerprinter.fingerprint_file("track.mp3").unwrap();
    /// ```
    pub fn fingerprint_file(&self, path: &str) -> Result<Vec<Fingerprint>, WavioError> {
        let audio = load_audio(path)?;
        self.fingerprint(&audio.samples)
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

    fn tonal(secs: f32) -> Vec<f32> {
        (0..(secs * 22_050.0) as usize)
            .map(|i| {
                let t = i as f32 / 22_050.0;
                let f = 300.0 + 120.0 * (t * 1.7).sin().abs() + 40.0 * (t.floor());
                0.4 * (std::f32::consts::TAU * f * t).sin()
                    + 0.2 * (std::f32::consts::TAU * 2.0 * f * t).sin()
            })
            .collect()
    }

    #[test]
    fn test_hybrid_emits_both_families_without_collisions() {
        let samples = tonal(8.0);
        let triplet_cfg = TripletHashConfig::default();
        let hybrid = Fingerprinter::default().with_hybrid_hashing(triplet_cfg.clone());
        let pairwise_only = Fingerprinter::default().with_peak_interpolation(true);
        let triplet_only = Fingerprinter::default()
            .with_triplet_hashing(triplet_cfg)
            .with_peak_interpolation(true);

        let h = hybrid.fingerprint(&samples).unwrap();
        let p = pairwise_only.fingerprint(&samples).unwrap();
        let t = triplet_only.fingerprint(&samples).unwrap();
        assert!(!p.is_empty() && !t.is_empty());
        assert_eq!(h.len(), p.len() + t.len());

        let tagged = h.iter().filter(|f| f.hash & TRIPLET_TAG != 0).count();
        assert_eq!(tagged, t.len());
        assert!(p.iter().all(|f| f.hash & TRIPLET_TAG == 0));
        assert!(
            t.iter().all(|f| f.hash & TRIPLET_TAG == 0),
            "untagged outside hybrid"
        );
    }

    #[test]
    fn test_signatures_distinguish_all_modes() {
        let cfg = TripletHashConfig::default();
        let sigs = [
            Fingerprinter::default().signature(),
            Fingerprinter::default()
                .with_peak_interpolation(true)
                .signature(),
            Fingerprinter::default()
                .with_triplet_hashing(cfg.clone())
                .signature(),
            Fingerprinter::default()
                .with_hybrid_hashing(cfg)
                .signature(),
        ];
        for (i, a) in sigs.iter().enumerate() {
            for b in &sigs[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn test_hybrid_max_pair_dt_covers_both_families() {
        let triplet = TripletHashConfig::new(15, 0.0, 2.5, 0.01, 0.01);
        let f = Fingerprinter::default().with_hybrid_hashing(triplet);
        assert!((f.max_pair_dt() - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_fingerprint_synthetic() {
        let samples = vec![0.0_f32; 1000];

        let spectrogram_config = SpectrogramConfig {
            window_size: 256,
            hop_size: 128,
        };
        let peak_config = PeakExtractorConfig {
            time_neighborhood: 2,
            freq_neighborhood: 2,
            threshold_db: -50.0,
            ..PeakExtractorConfig::default()
        };
        let hash_config = HashConfig::default();

        let fp = Fingerprinter::new(spectrogram_config, peak_config, hash_config);
        let result = fp.fingerprint(&samples);
        assert!(result.is_ok());
    }
}
