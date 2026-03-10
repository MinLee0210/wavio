# Contributing to wavio

Thank you for considering a contribution to wavio. This document covers the
conventions and expectations for anyone who wants to submit patches.

---

## Development Setup

```bash
# Clone the repository
git clone https://github.com/MinLee0210/wavio.git
cd wavio

# Build in debug mode
cargo build

# Run the full test suite
cargo test

# Run lints (must pass before merging)
cargo clippy -- -D warnings
cargo fmt --check
```

---

## Project Structure

```
src/
  lib.rs          Crate root -- lint configuration, module declarations
  error.rs        Unified error type (WavioError)
  utils.rs        Shared helpers (file validation, etc.)
  dsp/
    mod.rs         DSP pipeline re-exports
    audio.rs       WAV/audio loading, mono downmix, f32 normalization
    spectrogram.rs Sliding-window FFT, power spectrum
    peaks.rs       2D local-max constellation point extraction
  hash.rs          Combinatorial hashing of peak pairs
  index.rs         In-memory and on-disk fingerprint index + query engine
  io/
    mod.rs         I/O trait re-exports
    base.rs        IOReader trait definition
    file.rs        File-based IOReader implementation
```

---

## Code Style

- **Formatting**: Run `cargo fmt` before every commit. The CI gate rejects
  unformatted code.
- **Lints**: The crate uses `#![deny(clippy::all)]` and
  `#![warn(clippy::pedantic)]`. Fix all warnings before opening a PR.
- **Documentation**: Every public item (`pub fn`, `pub struct`, `pub enum`,
  `pub trait`) requires a `///` doc comment. Module-level `//!` comments are
  required for every module file.
- **Unsafe code**: Forbidden (`#![forbid(unsafe_code)]`). No exceptions.

---

## Commit Messages

Use the conventional format:

```
<type>(<scope>): <short summary>

<optional body>
```

Types: `feat`, `fix`, `refactor`, `test`, `docs`, `ci`, `bench`, `chore`.

Examples:
- `feat(dsp): implement Hann window function`
- `fix(index): correct off-by-one in histogram binning`
- `test(hash): add determinism test for combinatorial hashing`

---

## Testing

- Unit tests live alongside the code in `#[cfg(test)] mod tests { ... }` blocks.
- Integration tests go in the `tests/` directory.
- Benchmarks go in the `benches/` directory using `criterion`.
- All PRs must pass `cargo test` with no failures.

---

## Pull Request Checklist

Before opening a PR, verify all of the following:

- [ ] `cargo build` succeeds with no errors
- [ ] `cargo test` passes with no failures
- [ ] `cargo clippy -- -D warnings` produces no warnings
- [ ] `cargo fmt --check` reports no formatting issues
- [ ] New public items have `///` doc comments
- [ ] Changes are covered by at least one test

---

## API Design Principles

1. **Configuration via structs with `Default`**: Tunable parameters belong in a
   config struct that implements `Default` with sensible values. This allows
   users to override only what they care about.
2. **Errors, not panics**: All fallible operations return `Result<T, WavioError>`.
   Never call `.unwrap()` or `.expect()` in library code.
3. **Feature flags for optional dependencies**: Heavy or niche dependencies
   (e.g., `symphonia`, `sled`, `pyo3`) must be gated behind Cargo feature flags.
4. **Determinism**: The fingerprinting pipeline must be deterministic --
   identical input always produces identical output, regardless of platform or
   thread count.

---

## License

By contributing, you agree that your contributions will be licensed under the
MIT License, the same license that covers the project.
