# wavio — Architecture and Design Decisions

This document details the architectural design, signal processing pipeline, bit-packing layouts, and engineering trade-offs of the `wavio` peak-based acoustic fingerprinting engine.

---

## 1. Pipeline Overview

The `wavio` pipeline ingests raw audio, generates a 2D time-frequency power spectrogram, extracts key local maxima (constellation points), and constructs combinatorial pairs of these points to produce search hashes.

```text
               +-----------------------+
               |     Audio Source      |
               | (WAV/MP3/FLAC/AAC/..) |
               +-----------------------+
                           |
                           v
               +-----------------------+
               |     Mono Downmix      |
               |     & Normalization   |
               +-----------------------+
                           |
                           v
               +-----------------------+
               |  Resampler (Linear)   |
               |    to 22,050 Hz       |
               +-----------------------+
                           |
                           v
               +-----------------------+
               |  Hann Window & FFT    |
               |  (2048 Size, 512 Hop) |
               +-----------------------+
                           |
                           v
               +-----------------------+
               | dB Power Spectrogram  |
               |  (ndarray Shape: FxB) |
               +-----------------------+
                           |
                           v
               +-----------------------+
               | 2D Local-Max Filter   |
               |  (Constellation Peaks)|
               +-----------------------+
                           |
                           v
               +-----------------------+
               | Combinatorial Hash    |
               |   (Pairing Target)    |
               +-----------------------+
                           |
                           v
               +-----------------------+
               | Bit-Packing (64-bit)  |
               |   (u64 Fingerprints)  |
               +-----------------------+
```

---

## 2. DSP & Algorithmic Parameters

### A. Internal Sample Rate (22,050 Hz)
*   **Decision**: Downsample all audio inputs to a constant internal sampling rate of `22,050 Hz`.
*   **Rationale**: 
    *   Human voice and music instrumentation are concentrated heavily below 5 kHz. According to the Nyquist theorem, a sampling rate of `22,050 Hz` can perfectly capture frequencies up to `11,025 Hz`, which is more than enough for fingerprinting.
    *   Downsampling from standard consumer sample rates (`44,100 Hz` or `48,000 Hz`) reduces the size of FFT buffers, processing time, and memory usage by 50%+.

### A2. Resampling

Anything not already at 22,050 Hz goes through `dsp::audio::resample`, a Kaiser-windowed-sinc filter (8 zero crossings, β = 8, ~80 dB stop-band). For downsampling the kernel is stretched so its cutoff is the *new* Nyquist, which is what prevents aliasing; per-phase taps are precomputed for rate pairs with a small reduced ratio (44.1k and 48k → 22.05k), making the inner loop a plain dot product. `resample_linear` (the previous behavior) is kept for comparison. The resampler version is part of `Fingerprinter::signature()` (`rs=sinc1`), so databases built with a different one are flagged.

### B. Spectrogram Windowing & FFT Size (2048 Samples / 512 Hop)
*   **FFT Size (`window_size` = 2048)**:
    *   At `22,050 Hz`, a window size of 2048 represents `92.9 ms` of audio.
    *   This provides a frequency resolution of `22,050 / 2048 ≈ 10.77 Hz per bin`.
*   **Hop Size (`hop_size` = 512)**:
    *   The window advances by 512 samples (`23.2 ms` hop time).
    *   This represents a 75% overlap, guaranteeing that transient peaks aren't missed or overly attenuated by the Hann window edges.

### C. 2D Peak Neighborhood (10 Frames x 10 Bins)
*   **Decision**: Extract local maxima within a neighborhood of $\pm 10$ frames and $\pm 10$ frequency bins.
*   **Rationale**: 
    *   Prevents extracting too many adjacent peaks representing the same audio event.
    *   Balances density (target: 200-500 peaks per 10s of audio) with uniqueness, preventing hash combinatorics explosion while ensuring high-fidelity matches.

---

## 3. Combinatorial Hashing & Bit-Packing

To achieve robustness against noise and time-stretching, peaks are not hashed individually. Instead, they are paired. 

For each anchor peak, up to `fan_value` (default: 15) target peaks are selected within a time-delta window (`0.0s` to `1.0s`).

### 64-bit Hash Bit Layout

The pair is packed into a single `u64` integer:

```text
 63          60 59                  40 39                  20 19                   0
+--------------+----------------------+----------------------+----------------------+
|   Reserved   |      freq1_bin       |      freq2_bin       |       delta_t        |
|   (4 bits)   |      (20 bits)       |      (20 bits)       |      (20 bits)       |
+--------------+----------------------+----------------------+----------------------+
```

*   **`freq1_bin` (20 bits)**: Quantized frequency bin of the anchor peak (max value 1,048,575).
*   **`freq2_bin` (20 bits)**: Quantized frequency bin of the target peak (max value 1,048,575).
*   **`delta_t` (20 bits)**: Quantized time difference between anchor and target (max value 1,048,575). At `10 ms` resolution, this can encode time differences up to `10,485` seconds (nearly 3 hours).

---

## 3b. Triplet Hashing (Pitch-Shift / Time-Stretch-Robust Mode)

The pairwise hash above encodes **absolute** `(freq1_bin, freq2_bin, delta_t)`. Any uniform pitch shift (all frequencies scaled by a constant factor `s`) or time stretch (all time deltas scaled by a constant factor `r`) changes every one of those three values, so the hash changes — this is the root cause of the pitch/time-stretch limitation below.

`wavio::triplet` instead hashes **ratios** within a triplet of peaks `(anchor, t1, t2)`:

*   `freq_ratio_1 = t1.freq / anchor.freq`, `freq_ratio_2 = t2.freq / anchor.freq` — under a uniform pitch shift, numerator and denominator both scale by `s`, so the ratio is **exactly unchanged** (modulo FFT bin quantization noise on the underlying peak frequencies).
*   `time_ratio = (t2.time - anchor.time) / (t1.time - anchor.time)` — under a uniform time stretch, both deltas scale by `r`, so this ratio is likewise unchanged.

This is the same principle behind Panako's Constant-Q triplet hashing. `anchor_time` on the resulting fingerprint is still `anchor.time`, so this mode is a drop-in alternative hash source — `Index`/`PersistentIndex` require no changes.

### Triplet Bit Layout

Uses the same 64-bit budget as the pairwise hash, for symmetry:

```text
 63          60 59              40 39              20 19               0
+--------------+------------------+------------------+------------------+
|   Reserved   |  freq_ratio_1 q  |  freq_ratio_2 q  |   time_ratio q   |
|   (4 bits)   |     (20 bits)    |     (20 bits)    |     (20 bits)    |
+--------------+------------------+------------------+------------------+
```

Each ratio is quantized as `round(log2(ratio) / resolution)`, bias-shifted into an unsigned 20-bit range — quantizing in `log2` space matches the multiplicative (scale-factor) nature of the invariance this mode relies on.

For each anchor, up to `fan_value` subsequent peaks are collected (identical candidate selection to the pairwise algorithm), and each **adjacent pair** among them forms one triplet — bounding output to `fan_value - 1` hashes per anchor, the same order of magnitude as the pairwise hash's `fan_value` pairs, avoiding the `C(fan_value, 2)` blow-up of an all-pairs approach.

Enable it via `Fingerprinter::with_triplet_hashing(TripletHashConfig::default())`, or `wavio-cli --robust`. For best results also enable sub-bin peak interpolation (§3c); `--robust` does.

## 3c. Sub-bin Peak Interpolation, Hybrid Hashing, and What Was Measured

These three additions were driven by `examples/06_accuracy_eval.rs`, not by theory; several plausible ideas failed their measurements and are recorded here so they are not retried.

**Why plain triplets were fragile.** Peaks sit on exact FFT-bin centers (~10.8 Hz apart), so a frequency *ratio* inherits a quantization error of up to ±0.5 bin / bin-index (±2.5% at 220 Hz) — much larger than the 0.01 `log2` quantization step. Almost any perturbation flips hashes.

**Sub-bin frequency interpolation** (`PeakExtractorConfig::interpolate`). A parabola is fitted through the peak's dB value and its two frequency neighbors; the vertex gives a sub-bin frequency. This cuts the ratio error several-fold and raised 3 s accuracy at +2% speed from 85% to 98%. It is off by default so pairwise hashes keep matching earlier releases.

*Time is not interpolated.* Without interpolation, peaks in the same frame share an identical timestamp and the triplet code discards them as degenerate (`dt1 ≈ 0`). With interpolated times they become near-coincident and form triplets whose `dt2/dt1` is pure jitter: the hash count nearly doubled and speed-change accuracy collapsed (90% → 50%).

**Hybrid hashing** (`Fingerprinter::with_hybrid_hashing`). Pairwise hashes are the most noise-robust but fail under any speed/pitch change; triplet hashes are the reverse. Hybrid emits both from the same peaks, tagging triplet hashes with bit 63 (`TRIPLET_TAG`; neither family uses it) so they cannot collide. Both families vote into the same `(track, offset)` bins, so whichever survives carries the match. Cost ≈ 1.5× hashes per track.

**Real recordings changed the picture** (`06_accuracy_eval --dir`): the synthetic harness showed hybrid dominating, but on real audio pairwise alone is as good in noise (hybrid's gain is speed/pitch robustness), and triplet-only is far weaker in noise than the synthetic data suggested — three peaks must all survive, so its odds are roughly the cube of a peak's survival probability against the square for pairwise.

**Tried and rejected** (all measured): interpolating time; all-pairs triplets (no gain, worse in noise, ~2× hashes); coarser ratio quantization (0.02–0.05); a minimum anchor distance; a separable max-filter for peak picking (2× *slower* — the early-exit scan only pays the full window for actual peaks, so the "441 comparisons per bin" intuition is wrong).

## 3d. Scoring

A track's score is its **tallest pair of adjacent offset bins** (default bin 50 ms), so jittery votes straddling a bin boundary are not split in half; the reported offset is the vote-weighted mean of the pair. Results carry:

*   `confidence` = `score / query_fingerprints` — how much of the query was explained.
*   `margin` = `(score − runner_up) / score` — how decisive the win is. It does not shrink when the query contains audio that isn't in the index, so it is usually the better reject signal for open-set use.

Ties are broken by `TrackId`, so results are deterministic.

**Rarity (IDF) weighting** (`QueryOptions::idf_weighting`, opt-in). Each vote is weighted `1 / (1 + ln(postings))`, where `postings` is how often that hash occurs in the index, and ranking uses the weighted sum. Coarse hashes (triplet ratios) and repetitive music create many accidental collisions that can out-vote the truth; down-weighting frequent hashes counters that. `score` remains the raw aligned-hash count and `weighted_score` is what ranking used. Measured on real audio: helps triplet and hybrid under noise by a few points, neutral elsewhere, never hurts.

**Time-scale-aware voting** (`ScaleSearch`, opt-in). A speed change makes `db_time − query_time` drift linearly, smearing votes. For the top few candidates the matches are re-binned under a grid of hypotheses `db ≈ s·q + offset` (default ±8%, 1% steps) and the best wins; ties among hypotheses take the median (taking the one nearest 1.0 biases estimates toward 1). Measured result: it does **not** improve identification accuracy (misses are recall-limited, not smear-limited) but it recovers the speed (reported 1.02 / 0.97 / 1.06 for true 1.02 / 0.97 / 1.06).

## 3e. On-disk Format (schema version 2)

`PersistentIndex` (`redb`) stores:

*   `postings` — multimap `u64 hash → u64`, each value `(track_id << 32) | anchor_time.to_bits()`. One B-tree insert per fingerprint (O(log n)), independent of how many tracks share the hash; identical entries collapse, so re-saving is idempotent.
*   `tracks_by_name`, `tracks_by_id`, and `metadata` (`schema_version`, `next_id`, bincode `IndexConfig`, and `signature`).

`signature` is `Fingerprinter::signature()` — hash mode plus every FFT/peak/hash parameter. `ensure_signature` / `check_signature` fail with `IncompatibleIndex` on mismatch, replacing the old behavior where a mode mismatch silently returned "no match". Opening a database with a *newer* schema fails; schema 1 (the old per-hash `Vec` table) is migrated on first open.

Queries read only the postings for the query's own hashes. `Index::load_from_disk` / `save_to_disk` remain for loading everything into RAM.

## 4. Known Limitations

*   **Time Stretching & Pitch Shifting**: The default pairwise hash mode is brittle here: significant time stretching (>5%) shifts target delta times beyond the quantization resolution, resulting in hash mismatches, and pitch shifting changes absolute frequency bin locations, preventing matching. The opt-in triplet hashing mode (§3b) addresses this at the hash level — ratio-based hashes are largely invariant to both distortions — but the index's time-offset histogram still correlates on absolute `anchor_time`, which drifts under real time stretch (though not under pitch shift alone). So triplet mode meaningfully improves match *recall* (whether the right track is found at all) under both distortions. Opt-in time-scale voting (§3d) now recovers the speed factor and re-aligns the offset for the top candidates, though it does not by itself raise identification accuracy.
*   **Monophonic Noise/Overlap**: In highly noisy environments or where voice/noise completely dominates the audio, the extracted local maxima shifts away from the original song's spectral peaks, degrading match scores. Pairwise and hybrid hashing are the most robust here; triplet-only is the weakest (3 s clips at 0 dB SNR: ~42–52%).
*   **Small real-audio evaluation**: the real-recording numbers come from 20 short ringtone-style tracks (247 s; many are loops/arpeggios, which also makes them a hard case for alignment). 60 queries per cell means differences under ~8 points are noise. A larger, more varied set (full songs, speech over music, room recordings) is still needed. The synthetic harness is optimistic, particularly for noise.
*   **Resampling cost**: `resample` is a Kaiser-windowed sinc (~80 ms per 3-minute track serial, ~17 ms with the `parallel` feature, which splits the independent output samples across threads with bit-identical results; the old `resample_linear` is ~27 ms but aliases).
*   **Storage Size**: High `fan_value` parameters lead to a combinatorial explosion of fingerprints. Indexing a large library (>100,000 tracks) requires considerable RAM or large disk persistent indexes.
