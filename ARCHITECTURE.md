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

## 4. Known Limitations

*   **Time Stretching & Pitch Shifting**: Significant time stretching (>5%) shifts target delta times beyond the quantization resolution, resulting in hash mismatches. Pitch shifting changes absolute frequency bin locations, preventing matching.
*   **Monophonic Noise/Overlap**: In highly noisy environments or where voice/noise completely dominates the audio, the extracted local maxima shifts away from the original song's spectral peaks, degrading match scores.
*   **Storage Size**: High `fan_value` parameters lead to a combinatorial explosion of fingerprints. Indexing a large library (>100,000 tracks) requires considerable RAM or large disk persistent indexes.
