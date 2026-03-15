// ─── Lint Configuration ──────────────────────────────────────────────
//
// Level 1: deny all basic clippy warnings — catches common mistakes
#![deny(clippy::all)]
// Level 2: warn on pedantic lints — stricter, opinionated style checks
#![warn(clippy::pedantic)]
// Level 3: warn on missing docs — enforces documentation on public items
#![warn(missing_docs)]
// Level 4: deny unsafe code — no `unsafe` blocks allowed in this crate
#![forbid(unsafe_code)]

//! # wavio
//!
//! Peak-based audio fingerprinting. Zero overhead. Written in Rust.
//!
//! `wavio` is a high-throughput acoustic fingerprinting library built for
//! DSP engineers who need fast, deterministic audio identification without
//! the weight of an ML stack.

pub mod dsp;
pub mod error;
pub mod hash;
pub mod index;
pub mod io;
pub mod utils;

#[cfg(feature = "persist")]
pub mod persist;

/// Python bindings via PyO3.
#[cfg(feature = "python")]
pub mod python;
