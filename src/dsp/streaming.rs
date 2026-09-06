//! Block-based streaming fingerprinting.
//!
//! [`Fingerprinter::fingerprint`](crate::dsp::Fingerprinter::fingerprint)
//! requires the entire sample buffer to be resident in memory. For long
//! files or live/incremental sources (a microphone, a network stream), that
//! is undesirable. [`StreamingFingerprinter`] instead consumes samples in
//! chunks of any size and emits fingerprints incrementally.
//!
//! # Design
//!
//! This processes audio in overlapping fixed-size blocks, re-running the
//! full (unmodified) [`Fingerprinter::fingerprint`] pipeline on each block,
//! rather than maintaining incremental per-frame DSP state.
//!
//! Each block's *finalized* (emitted) region is trimmed by `overlap_secs` on
//! **both** edges (except the very first block's front, which is the true
//! start of the stream and needs no leading context, exactly like a single
//! `Fingerprinter::fingerprint` call on a whole file):
//!
//! - the tail is trimmed so every anchor in the finalized region still has
//!   its full [`Fingerprinter::max_pair_dt`] fan-out window, and every peak
//!   in it has real trailing spectral neighbors, *within this same block*;
//! - the front is trimmed because those samples are the trailing context
//!   carried over from the previous block, and on their own they still lack
//!   *leading* neighbor context (nothing precedes them in this block's
//!   buffer) — that region was already validly computed by whichever
//!   neighboring block had it in the middle, with real context on both
//!   sides.
//!
//! Consequently each block retains `2 * overlap_samples` for the next one:
//! `overlap_samples` to serve as this block's own (unemitted) tail margin,
//! plus another `overlap_samples` so the next block's front-trimmed region
//! has real leading context. With this bookkeeping, consecutive blocks'
//! finalized regions are contiguous and non-overlapping by construction; a
//! "high watermark" cutoff is additionally tracked as a defense-in-depth
//! safety net against ever emitting the same time region twice.

use crate::dsp::fingerprint::Fingerprinter;
use crate::error::WavioError;
use crate::hash::Fingerprint;

/// Configuration for [`StreamingFingerprinter`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct StreamConfig {
    /// Duration (seconds) of each processed block.
    pub block_duration_secs: f32,
    /// Duration (seconds) each block overlaps the next. Must be at least
    /// the active hashing mode's `max_pair_dt` and the peak detector's
    /// trailing-neighborhood duration; see [`StreamingFingerprinter::new`].
    pub overlap_secs: f32,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            block_duration_secs: 10.0,
            overlap_secs: 1.0,
        }
    }
}

impl StreamConfig {
    /// Creates a new `StreamConfig`.
    #[must_use]
    pub fn new(block_duration_secs: f32, overlap_secs: f32) -> Self {
        Self {
            block_duration_secs,
            overlap_secs,
        }
    }
}

/// Incrementally fingerprints an audio stream in overlapping blocks.
///
/// See the module documentation for the streaming design.
#[derive(Debug)]
pub struct StreamingFingerprinter {
    fingerprinter: Fingerprinter,
    block_samples: usize,
    overlap_samples: usize,
    sample_rate: u32,
    buffer: Vec<f32>,
    stream_offset_samples: usize,
    high_watermark_secs: f32,
    is_first_block: bool,
}

impl StreamingFingerprinter {
    /// Creates a new `StreamingFingerprinter`.
    ///
    /// # Errors
    ///
    /// Returns [`WavioError::SpectrogramError`] if `config.overlap_secs` is
    /// smaller than either the active hashing mode's `max_pair_dt` or the
    /// peak detector's trailing-neighborhood duration (both of which would
    /// let a block boundary truncate a real anchor's fan-out window or
    /// falsify a peak's local-maximality test), or if the resulting block
    /// size is smaller than one spectrogram window.
    ///
    /// # Examples
    ///
    /// ```
    /// use wavio::dsp::Fingerprinter;
    /// use wavio::dsp::streaming::{StreamConfig, StreamingFingerprinter};
    ///
    /// let streamer = StreamingFingerprinter::new(
    ///     Fingerprinter::default(),
    ///     &StreamConfig::default(),
    /// );
    /// assert!(streamer.is_ok());
    /// ```
    #[allow(clippy::cast_precision_loss)]
    pub fn new(fingerprinter: Fingerprinter, config: &StreamConfig) -> Result<Self, WavioError> {
        let sample_rate = fingerprinter.peak_config.sample_rate;

        #[allow(clippy::cast_precision_loss)]
        let peak_trailing_context_secs = fingerprinter.peak_config.time_neighborhood as f32
            * fingerprinter.peak_config.hop_size as f32
            / sample_rate as f32;

        let min_overlap_secs = fingerprinter.max_pair_dt().max(peak_trailing_context_secs);

        if config.overlap_secs < min_overlap_secs {
            return Err(WavioError::SpectrogramError(format!(
                "overlap_secs ({}) must be at least {min_overlap_secs} (max(hashing mode's \
                 max_pair_dt, peak detector's trailing-neighborhood duration))",
                config.overlap_secs
            )));
        }

        // Round block/overlap sizes up to the nearest multiple of the
        // spectrogram hop size. This is essential, not cosmetic: it keeps
        // every block boundary on the same FFT frame grid a single batch
        // `Fingerprinter::fingerprint` call over the whole stream would use.
        // Without it, each block's spectrogram windows would be phase-shifted
        // relative to the continuous signal by a fractional hop, changing
        // the exact peaks/hashes computed near every boundary.
        let hop_size = fingerprinter.spectrogram_config.hop_size.max(1);

        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let raw_block_samples = (config.block_duration_secs * sample_rate as f32) as usize;
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let raw_overlap_samples = (config.overlap_secs * sample_rate as f32) as usize;

        let block_samples = raw_block_samples.div_ceil(hop_size) * hop_size;
        let overlap_samples = raw_overlap_samples.div_ceil(hop_size) * hop_size;

        // Every block after the first is trimmed by `overlap_samples` on
        // *both* edges (see `push`'s doc comment for why), so the block must
        // be more than twice the overlap.
        if block_samples <= 2 * overlap_samples
            || block_samples < fingerprinter.spectrogram_config.window_size
        {
            return Err(WavioError::SpectrogramError(format!(
                "block_duration_secs ({}) is too small relative to overlap_secs ({}) and/or \
                 the spectrogram window size ({}) -- block_samples must exceed 2x overlap_samples",
                config.block_duration_secs,
                config.overlap_secs,
                fingerprinter.spectrogram_config.window_size
            )));
        }

        Ok(Self {
            fingerprinter,
            block_samples,
            overlap_samples,
            sample_rate,
            buffer: Vec::new(),
            stream_offset_samples: 0,
            high_watermark_secs: 0.0,
            is_first_block: true,
        })
    }

    /// Feeds more samples into the stream, returning any newly-finalized
    /// fingerprints. `anchor_time` on returned fingerprints is an absolute
    /// offset from the start of the whole stream, not just this block.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying [`Fingerprinter::fingerprint`]
    /// call fails on a completed block.
    ///
    /// # Examples
    ///
    /// ```
    /// use wavio::dsp::Fingerprinter;
    /// use wavio::dsp::streaming::{StreamConfig, StreamingFingerprinter};
    ///
    /// let mut streamer = StreamingFingerprinter::new(
    ///     Fingerprinter::default(),
    ///     &StreamConfig::default(),
    /// ).unwrap();
    ///
    /// let chunk = vec![0.0_f32; 22_050]; // 1 second of silence
    /// let fingerprints = streamer.push(&chunk).unwrap();
    /// assert!(fingerprints.is_empty()); // not enough buffered yet for one block
    /// ```
    #[allow(clippy::cast_precision_loss)]
    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<Fingerprint>, WavioError> {
        self.buffer.extend_from_slice(samples);

        let mut emitted = Vec::new();

        #[allow(clippy::cast_precision_loss)]
        while self.buffer.len() >= self.block_samples {
            let block = &self.buffer[..self.block_samples];
            let block_fingerprints = self.fingerprinter.fingerprint(block)?;

            let block_start_secs = self.stream_offset_samples as f32 / self.sample_rate as f32;

            // A block's first `overlap_samples` are the trailing context
            // carried over from the previous block: they were *also* the
            // previous block's un-emitted tail, and on their own they still
            // lack leading neighbor context (nothing precedes them in this
            // buffer), so they must be trimmed here too -- except for the
            // very first block, which is genuinely the start of the stream
            // and should be treated exactly like a one-shot
            // `Fingerprinter::fingerprint` call on the whole file (which has
            // the same t=0 edge truncation).
            let front_trim_secs = if self.is_first_block {
                0.0
            } else {
                self.overlap_samples as f32 / self.sample_rate as f32
            };
            let finalized_start_secs = block_start_secs + front_trim_secs;
            let finalized_cutoff_secs = block_start_secs
                + (self.block_samples - self.overlap_samples) as f32 / self.sample_rate as f32;

            for fp in block_fingerprints {
                let abs_time = fp.anchor_time + block_start_secs;
                if abs_time >= finalized_start_secs
                    && abs_time < finalized_cutoff_secs
                    && abs_time >= self.high_watermark_secs
                {
                    emitted.push(Fingerprint {
                        hash: fp.hash,
                        anchor_time: abs_time,
                    });
                }
            }

            self.high_watermark_secs = finalized_cutoff_secs;
            self.is_first_block = false;

            // Retain 2x overlap: `overlap_samples` to remain this block's
            // (already emitted) tail context, plus another `overlap_samples`
            // so the *next* block's front trim region has real leading
            // context of its own -- see the comment above.
            let advance = self.block_samples - 2 * self.overlap_samples;
            self.buffer.drain(..advance);
            self.stream_offset_samples += advance;
        }

        Ok(emitted)
    }

    /// Flushes the tail end of the stream (whatever remains buffered,
    /// shorter than one full block) and returns its fingerprints. Call this
    /// once after the last [`StreamingFingerprinter::push`].
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying [`Fingerprinter::fingerprint`]
    /// call fails.
    #[allow(clippy::cast_precision_loss)]
    pub fn finish(&mut self) -> Result<Vec<Fingerprint>, WavioError> {
        if self.buffer.len() < self.fingerprinter.spectrogram_config.window_size {
            self.buffer.clear();
            return Ok(Vec::new());
        }

        #[allow(clippy::cast_precision_loss)]
        let block_start_secs = self.stream_offset_samples as f32 / self.sample_rate as f32;
        let front_trim_secs = if self.is_first_block {
            0.0
        } else {
            self.overlap_samples as f32 / self.sample_rate as f32
        };
        let finalized_start_secs = block_start_secs + front_trim_secs;

        let tail_fingerprints = self.fingerprinter.fingerprint(&self.buffer)?;

        // This is the last block (nothing more will ever be pushed), so
        // there's no trailing overlap to trim -- everything from
        // `finalized_start_secs` onward is final.
        let mut emitted = Vec::new();
        for fp in tail_fingerprints {
            let abs_time = fp.anchor_time + block_start_secs;
            if abs_time >= finalized_start_secs && abs_time >= self.high_watermark_secs {
                emitted.push(Fingerprint {
                    hash: fp.hash,
                    anchor_time: abs_time,
                });
            }
        }

        self.buffer.clear();
        Ok(emitted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A synthetic multi-tone signal with enough structure to produce many
    /// peaks/hashes across its duration.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn synthetic_samples(duration_secs: f32, sample_rate: u32) -> Vec<f32> {
        let n = (duration_secs * sample_rate as f32) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                let tone = 220.0 + 400.0 * (t * 0.7).sin().abs();
                (std::f32::consts::TAU * tone * t).sin() * 0.5
            })
            .collect()
    }

    #[test]
    fn test_new_rejects_overlap_smaller_than_max_pair_dt() {
        let fingerprinter = Fingerprinter::default(); // hash_config.max_dt == 1.0
        let config = StreamConfig::new(10.0, 0.5);
        let result = StreamingFingerprinter::new(fingerprinter, &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_rejects_block_smaller_than_overlap() {
        let fingerprinter = Fingerprinter::default();
        let config = StreamConfig::new(1.0, 1.0);
        let result = StreamingFingerprinter::new(fingerprinter, &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_streaming_matches_batch_fingerprinting() {
        let sample_rate = 22_050;
        let samples = synthetic_samples(25.0, sample_rate);

        let batch_hashes: HashSet<u64> = Fingerprinter::default()
            .fingerprint(&samples)
            .expect("batch fingerprint failed")
            .into_iter()
            .map(|fp| fp.hash)
            .collect();

        let mut streamer =
            StreamingFingerprinter::new(Fingerprinter::default(), &StreamConfig::new(5.0, 1.0))
                .expect("failed to build streaming fingerprinter");

        let mut streamed_hashes: HashSet<u64> = HashSet::new();
        for chunk in samples.chunks(2048) {
            let fps = streamer.push(chunk).expect("push failed");
            streamed_hashes.extend(fps.into_iter().map(|fp| fp.hash));
        }
        let tail = streamer.finish().expect("finish failed");
        streamed_hashes.extend(tail.into_iter().map(|fp| fp.hash));

        // With block boundaries aligned to the spectrogram's hop-size grid
        // and both edges of each block trimmed to full-context regions (see
        // the module docs), streaming should reproduce the batch computation
        // almost exactly -- any residual gap is only possible right at the
        // very last (odd-sized) tail block.
        let intersection = batch_hashes.intersection(&streamed_hashes).count();
        let union = batch_hashes.union(&streamed_hashes).count();
        assert!(union > 0);
        #[allow(clippy::cast_precision_loss)]
        let overlap_ratio = intersection as f64 / union as f64;
        assert!(
            overlap_ratio > 0.99,
            "streamed/batch hash overlap too low: {overlap_ratio:.3} \
             (batch={}, streamed={})",
            batch_hashes.len(),
            streamed_hashes.len()
        );
    }

    #[test]
    fn test_push_then_finish_empty_input() {
        let mut streamer =
            StreamingFingerprinter::new(Fingerprinter::default(), &StreamConfig::default())
                .unwrap();
        let fps = streamer.push(&[]).unwrap();
        assert!(fps.is_empty());
        let tail = streamer.finish().unwrap();
        assert!(tail.is_empty());
    }
}
