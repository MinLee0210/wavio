# wavio Benchmarks

> Platform: Linux x86-64, 12 hardware threads, `powersave` CPU governor, desktop in use.
> Release profile (`lto = true`, `codegen-units = 1`). Criterion, `sample_size(10)` for the heavy groups.
> **Run-to-run noise on this machine is about ±20%**; compare orders of magnitude and A/B ratios, not the last digit.

The previous baseline (macOS, 2026-03) used toy workloads (20-hash tracks, a pure sine for DSP), which measured almost nothing: a pure sine yields almost no peaks, and a 20-hash track is ~0.04% of a real one. It is replaced by the sizes below.

## Workloads

| Group | Workload |
|---|---|
| `dsp/*` | 3 minutes of synthetic multi-tone "music" with noise: 15.7k peaks, 236k pairwise hashes |
| `index/*` | 200 tracks x 50,000 hashes = 10,000,000 entries, hashes drawn from a 2^24 space (so cross-track collisions occur); queries are ~2,900-hash 10 s clips with 30% noise hashes |
| `persist/*` | 50 tracks x 50,000 hashes = 2,500,000 entries on disk |

Every query in the benchmarks is asserted to return the correct track before it is timed.

## Results (v0.4.0, this machine)

| Benchmark | Result |
|---|---|
| `dsp/spectrogram_3min` | ~105 ms |
| `dsp/resample_sinc_44k_3min` / `_48k_` | ~80 ms / ~90 ms serial; **~17 ms / ~21 ms** with `parallel` (linear: ~27 ms) |
| `dsp/extract_peaks_3min` (serial) | ~105-130 ms (unchanged from 0.3.0 within noise) |
| `dsp/extract_peaks_parallel_3min` | ~20 ms (0.3.0: ~35 ms) |
| `dsp/fingerprint_full_3min` (serial) | ~0.23-0.26 s |
| `index/query_10s_clip_in_200_tracks` (per 20 queries) | 13-19 ms, i.e. **~0.65-0.95 ms per query** |
| `persist/query_10s_clip_in_50_tracks` (per 10 queries) | 19-24 ms, i.e. ~2 ms per query |
| `persist/insert_1_track_x_50k` | **~36 ms** (0.3.0 layout: ~291 ms, **8x faster**) |

A/B method for the entries compared with 0.3.0: both versions were built from the same benchmark file (a throwaway `git worktree` of the previous commit), run alternately three times, and compared by median. Results that moved less than the noise (serial peak extraction, in-memory and on-disk query) are reported as unchanged; the new adjacent-bin scoring and the multimap layout cost no measurable query time.

What did **not** work: a separable max-filter for peak picking benchmarked at 267 ms serial / 56 ms parallel vs ~114 / ~35 ms for the early-exit scan, so it was reverted.

## Accuracy

`cargo run --release --features parallel --example 06_accuracy_eval`

Top-1 accuracy (%) with 40 synthetic 30 s tracks, one query per track per cell at a random offset. **The library is synthetic** (random melodies with harmonics and percussive bursts): fine for relative comparison and regression detection, optimistic in absolute terms — in particular pairwise hashing looks perfect on white noise because tonal peaks survive it. "Hashes/track" is the index cost.

```text
== pairwise hashing (584699 hashes, 14617/track) ==
distortion          3 s      5 s     10 s
clean              100%     100%     100%
noise 20 dB        100%     100%     100%
noise 10 dB        100%     100%     100%
noise 0 dB         100%     100%     100%
lowpass            100%     100%     100%
speed +2%            5%       2%       2%
speed -3%            2%       2%       5%
speed +6%            5%      18%      18%
speed +9%            2%       5%       2%
noise10+spd2         0%       0%       5%

== triplet (v0.3 behavior) hashing (294698 hashes, 7367/track) ==
distortion          3 s      5 s     10 s
clean              100%     100%     100%
noise 20 dB         90%      98%     100%
noise 10 dB         90%      95%     100%
noise 0 dB          52%      75%      98%
lowpass            100%     100%     100%
speed +2%           85%      98%      98%
speed -3%           88%      92%     100%
speed +6%           78%      82%      92%
speed +9%           62%      70%      90%
noise10+spd2        55%      75%      95%

== triplet + freq interpolation hashing (294698 hashes, 7367/track) ==
distortion          3 s      5 s     10 s
clean              100%     100%     100%
noise 20 dB         90%      95%     100%
noise 10 dB         90%      95%     100%
noise 0 dB          42%      72%      98%
lowpass            100%     100%     100%
speed +2%           98%     100%     100%
speed -3%           95%     100%     100%
speed +6%           90%      95%     100%
speed +9%           90%     100%     100%
noise10+spd2        80%      92%      98%

== hybrid (pairwise + interpolated triplet) hashing (879397 hashes, 21985/track) ==
distortion          3 s      5 s     10 s
clean              100%     100%     100%
noise 20 dB        100%     100%     100%
noise 10 dB        100%     100%     100%
noise 0 dB          98%     100%     100%
lowpass            100%     100%     100%
speed +2%           95%     100%     100%
speed -3%           95%     100%     100%
speed +6%           92%      95%     100%
speed +9%           90%     100%     100%
noise10+spd2        80%      95%      98%

== hybrid + time-scale voting hashing (879397 hashes, 21985/track) ==
distortion          3 s      5 s     10 s
clean              100%     100%     100%
noise 20 dB        100%     100%     100%
noise 10 dB        100%     100%     100%
noise 0 dB          98%     100%     100%
lowpass            100%     100%     100%
speed +2%           95%     100%     100%
speed -3%           92%     100%     100%
speed +6%           92%      98%     100%
speed +9%           92%     100%     100%
noise10+spd2        80%      92%      98%
estimated speed (median): speed +2% -> 1.02, speed -3% -> 0.97, speed +6% -> 1.06, speed +9% -> 1.08
```

### Reading the table

*   **pairwise** is the best in noise but is useless once audio is sped up or slowed down.
*   **triplet** survives speed changes but is the weakest in heavy noise; **sub-bin frequency interpolation** fixes most of its speed fragility (+2%, 3 s: 85% → 98%).
*   **hybrid** keeps pairwise's noise robustness *and* triplet's speed robustness (0 dB noise, 3 s: 98%; +-6% speed: 92-95%), at ~1.5x the hashes of pairwise.
*   **time-scale voting** doesn't raise accuracy, but its speed estimate is accurate (last line).
*   The remaining misses are short (3 s) clips combining noise and speed change (80%); at 5-10 s everything above 92%.

## Real recordings

`scripts/prepare_real_eval.sh /usr/share/sounds/ubuntu/ringtones data/real_eval` then
`cargo run --release --features parallel --example 06_accuracy_eval -- --dir data/real_eval`

20 real stereo 44.1 kHz recordings (Ubuntu's bundled ringtones; 247 s total; many are short loops, which is itself a hard case for alignment). Queries of 2, 3 and 5 s are cut from the originals, converted to **48 kHz** by `ffmpeg`, optionally **MP3-encoded**, then loaded and resampled by wavio, so the library (44.1 → 22.05 kHz) and the query (44.1 → 48 → 22.05 kHz) take different resampling paths. Cells pool all three clip lengths: **60 queries per cell, so differences under ~8 points are not significant.** Sinc resampler.

| Distortion | pairwise | pairwise + idf | triplet + interp | triplet + interp + idf | hybrid | hybrid + idf |
|---|---:|---:|---:|---:|---:|---:|
| 48 kHz clean | 100% | 100% | 100% | 100% | 100% | 100% |
| MP3 128 kbps | 100% | 100% | 100% | 100% | 100% | 100% |
| MP3 32 kbps | 100% | 100% | 98% | 100% | 100% | 100% |
| + noise 10 dB | 93% | 93% | 27% | 37% | 83% | 87% |
| + noise 0 dB | 60% | 62% | 12% | 15% | 52% | 55% |
| + speed +3% | 18% | 10% | 100% | 100% | 100% | 100% |

Resampler comparison (same data, linear vs sinc; all other rows identical or within one query): hybrid 10 dB noise 80% → 83%, hybrid 0 dB noise 40% → 52%, pairwise unchanged (93% / 60%).

What this changed about earlier conclusions:

*   The synthetic library overstated noise robustness (pairwise at 0 dB: 100% synthetic, 60% real) and triplet-only is far weaker in real noise than synthetic data suggested.
*   Hybrid's edge over pairwise is **speed/pitch robustness**, not noise: under noise alone pairwise is as good (93% vs 83-87%, not significant) at two-thirds of the index size.
*   MP3 compression, even at 32 kbps, is a non-issue for every mode.
*   IDF weighting never hurts and helps the coarse-hash modes under noise by a few points.
*   The sinc resampler is a correctness fix; its accuracy effect on this data is within noise.

## How to Reproduce

```bash
cargo bench --features parallel,persist      # Criterion groups above
cargo run --release --features parallel --example 06_accuracy_eval
cargo run --release --features parallel --example 06_accuracy_eval -- hybrid   # only matching modes
scripts/prepare_real_eval.sh <audio-dir> data/real_eval && cargo run --release --features parallel --example 06_accuracy_eval -- --dir data/real_eval
```

Criterion writes detailed HTML reports to `target/criterion/`.
