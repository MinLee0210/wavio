# wavio Benchmark Baselines

> Generated: 2026-03-15  
> Platform: macOS, release profile (`cargo bench --features parallel`)  
> `RUSTFLAGS` default (no explicit target-cpu)  
> Profile: `[profile.release]` with `lto = true`, `codegen-units = 1`, `strip = true`

---

## Benchmark Descriptions

| Benchmark | What it measures |
|---|---|
| `fingerprint_single_3min` | Full DSP pipeline on a synthetic 3-minute mono clip at 22,050 Hz (spectrogram + peak extraction + hash generation) |
| `index_insert_1k_tracks` | Inserting 1,000 pre-fingerprinted tracks (20 hashes each) into an in-memory `Index` |
| `index_query_1k` | Running 1,000 queries against a pre-built 1k-track in-memory `Index` |

---

## Results

| Benchmark | Median | Lower bound | Upper bound | Outliers |
|---|---|---|---|---|
| `fingerprint_single_3min` | 88.64 ms | 87.98 ms | 89.65 ms | 4/100 (4%) |
| `index_insert_1k_tracks` | 1.04 ms | 1.04 ms | 1.05 ms | 5/100 (5%) |
| `index_query_1k` (1,000 queries total) | 568 µs | 567 µs | 570 µs | 14/100 (14%) |

### Derived throughput

| Metric | Value |
|---|---|
| Fingerprint speed | ~11.3 tracks/sec (serial pipeline, 3-min tracks) |
| Index insertion rate | ~960,000 hashes/sec (1k tracks × 20 hashes) |
| Query latency (per query) | ~0.57 µs |
| Query throughput | ~1.76 million queries/sec |

---

## How to Reproduce

```bash
# Serial pipeline (default)
cargo bench

# With rayon parallel feature enabled
cargo bench --features parallel
```

Criterion writes detailed HTML reports to `target/criterion/`.

---

## Notes

- The `fingerprint_single_3min` benchmark uses a synthetic 440 Hz sine wave. Real music will produce more peaks and thus more hashes, which will increase fingerprint time slightly.
- The `index_insert_1k_tracks` benchmark uses synthetic fingerprints (20 per track). Real tracks generate ~200–500 hashes per 10s clip.
- Benchmark figures will vary with CPU, core count, and system load. Re-run on the target platform before using these numbers for capacity planning.
- `DashMap` was evaluated for concurrent index writes and was determined unnecessary: the CPU-heavy fingerprinting step is parallelized by rayon, while HashMap insertion is serial but cheap.
