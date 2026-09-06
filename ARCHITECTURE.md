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

Enable it via `Fingerprinter::with_triplet_hashing(TripletHashConfig::default())`, or `wavio-cli --robust`. A database must be queried with the same mode it was indexed with — mixing modes silently degrades to "no match" rather than erroring, since the on-disk hashes don't record which mode produced them.

## 4. Known Limitations

*   **Time Stretching & Pitch Shifting**: The default pairwise hash mode is brittle here: significant time stretching (>5%) shifts target delta times beyond the quantization resolution, resulting in hash mismatches, and pitch shifting changes absolute frequency bin locations, preventing matching. The opt-in triplet hashing mode (§3b) addresses this at the hash level — ratio-based hashes are largely invariant to both distortions — but the index's time-offset histogram still correlates on absolute `anchor_time`, which drifts under real time stretch (though not under pitch shift alone). So triplet mode meaningfully improves match *recall* (whether the right track is found at all) under both distortions, and offset/confidence concentration specifically under pitch shift, but precise offset estimation under time stretch remains an open problem.
*   **Monophonic Noise/Overlap**: In highly noisy environments or where voice/noise completely dominates the audio, the extracted local maxima shifts away from the original song's spectral peaks, degrading match scores.
*   **Storage Size**: High `fan_value` parameters lead to a combinatorial explosion of fingerprints. Indexing a large library (>100,000 tracks) requires considerable RAM or large disk persistent indexes.
