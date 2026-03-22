# Configuration

All DSP parameters are controlled via configuration structs. Each has a `Default` implementation with sensible values tuned for music.

---

## `SpectrogramConfig`

Controls the FFT spectrogram.

| Field | Default | Tune when… |
|-------|---------|------------|
| `window_size` | `2048` | Need more frequency detail → increase. Need more time detail → decrease. |
| `hop_size` | `512` | Smaller = more frames (slower, finer resolution). Larger = faster, coarser. |

```rust
let config = SpectrogramConfig {
    window_size: 4096,  // better frequency resolution
    hop_size: 1024,     // fewer frames
};
```

!!! tip "Power-of-two window sizes"
    `window_size` should be a power of two (512, 1024, 2048, 4096) for optimal FFT performance.

---

## `PeakExtractorConfig`

Controls constellation peak detection.

| Field | Default | Tune when… |
|-------|---------|------------|
| `time_neighborhood` | `10` | Larger = sparser peaks (faster queries, less recall) |
| `freq_neighborhood` | `10` | Larger = sparser peaks in frequency dimension |
| `threshold_db` | `-40.0` | Too many peaks → raise threshold. Too few → lower it. |
| `sample_rate` | `22_050` | Must match your audio's sample rate |
| `hop_size` | `512` | Must match `SpectrogramConfig.hop_size` |
| `window_size` | `2048` | Must match `SpectrogramConfig.window_size` |

!!! warning "Keep configs in sync"
    `sample_rate`, `hop_size`, and `window_size` in `PeakExtractorConfig` must match your `SpectrogramConfig` and actual audio sample rate. Mismatched values will produce incorrect time/frequency coordinates.

**Target:** 200–500 peaks per 10 seconds of audio.

---

## `HashConfig`

Controls combinatorial hashing of peak pairs.

| Field | Default | Tune when… |
|-------|---------|------------|
| `fan_value` | `15` | More pairs → better recall, more hashes, slower |
| `min_dt` | `0.0` | Set `> 0` to skip very close peaks |
| `max_dt` | `1.0` | Wider window → more hashes, higher collision rate |
| `freq_bins` | `1024` | Number of quantization bins for frequency |
| `freq_resolution` | `~10.77` | Hz per bin (derived from `sample_rate / window_size`) |
| `dt_resolution` | `0.01` | Time quantization step in seconds |

**Target:** 1,000–5,000 hashes per track.

---

## `IndexConfig`

Controls the query time-offset histogram.

| Field | Default | Tune when… |
|-------|---------|------------|
| `offset_bin_size` | `0.05` | Smaller = finer offset resolution, lower peak height. Larger = more robust. |

---

## Recommended Defaults

The defaults work well for pop/rock music at standard sample rates. Use custom configs for:

| Use case | Changes to try |
|----------|----------------|
| **Speech** | `threshold_db: -30.0`, `max_dt: 2.0`, `fan_value: 10` |
| **Noisy environment** | `threshold_db: -25.0`, `time_neighborhood: 15` |
| **Short clips (< 5s)** | `fan_value: 20`, `offset_bin_size: 0.01` |
| **Large database (10k+ tracks)** | `fan_value: 10` (reduces hash count) |
