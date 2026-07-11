# Installation

## Rust

Add `wavio` to your `Cargo.toml`:

```toml
[dependencies]
wavio = "0.1"
```

### Feature Flags

Enable optional features based on your needs:

```toml
[dependencies]
wavio = { version = "0.1", features = ["persist", "parallel"] }
```

| Flag | What it adds | Extra dependencies |
|------|-------------|-------------------|
| `parallel` | Rayon-based parallel fingerprinting and peak extraction | `rayon`, `dashmap` |
| `persist` | On-disk index persistence via sled | `sled`, `serde`, `bincode` |
| `python` | Python bindings via PyO3 | `pyo3` |

### From Source

```bash
git clone https://github.com/MinLee0210/wavio.git
cd wavio
cargo build --release --all-features
```

---

## Python

**Requirements:** Python 3.8+, Rust toolchain, `maturin`.

### Step 1: Install Rust

If you don't have Rust installed:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Step 2: Install maturin

```bash
pip install maturin
```

### Step 3: Build from source

```bash
git clone https://github.com/MinLee0210/wavio.git
cd wavio/python
maturin develop --features python
```

### Step 4: Verify

```python
import wavio
print(dir(wavio))  # ['PyFingerprinter', 'PyIndex', ...]
```

!!! note "PyPI package coming soon"
    A `pip install wavio` package is planned for v0.2. For now, install from source using maturin.

---

## CLI

The CLI binary requires the `persist` feature:

```bash
# Install from crates.io
cargo install wavio --features persist

# Or build from source
git clone https://github.com/MinLee0210/wavio.git
cd wavio
cargo build --release --bin wavio-cli --features persist
```

The binary will be at `./target/release/wavio-cli`.
