# Benchmarks

> Generated: 2026-03-15 · Platform: macOS, release profile · Criterion  
> Profile: `lto = true`, `codegen-units = 1`, `strip = true`

---

## Results

| Benchmark | Median | Lower bound | Upper bound |
|-----------|--------|-------------|-------------|
| `fingerprint_single_3min` | **88.64 ms** | 87.98 ms | 89.65 ms |
| `index_insert_1k_tracks` | **1.04 ms** | 1.04 ms | 1.05 ms |
| `index_query_1k` (1,000 queries) | **568 µs** | 567 µs | 570 µs |

### Derived Throughput

| Metric | Value |
|--------|-------|
| Fingerprint speed | ~11.3 tracks/sec (serial, 3-min tracks) |
| Index insertion rate | ~960,000 hashes/sec |
| Query latency (per query) | ~0.57 µs |
| Query throughput | ~1.76M queries/sec |

---

## What Each Benchmark Measures

| Benchmark | Description |
|-----------|-------------|
| `fingerprint_single_3min` | Full DSP pipeline on a synthetic 3-minute mono clip at 22,050 Hz |
| `index_insert_1k_tracks` | Inserting 1,000 pre-fingerprinted tracks (20 hashes each) |
| `index_query_1k` | Running 1,000 queries against a 1k-track index |

---

## How to Reproduce

```bash
# Serial pipeline
cargo bench

# With rayon parallelism
cargo bench --features parallel
```

Criterion writes detailed HTML reports to `target/criterion/`.

---

## Notes

- Benchmarks use synthetic 440 Hz sine waves. Real music produces more peaks and hashes.
- The 20-hashes-per-track in `index_insert_1k_tracks` is conservative. Real tracks generate ~200–500 hashes per 10s clip.
- Numbers vary by CPU, core count, and system load. Re-run on your target platform for capacity planning.
