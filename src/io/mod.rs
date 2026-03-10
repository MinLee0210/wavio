//! I/O abstractions for reading audio data from various sources.

mod base;
mod file;

pub use base::IOReader;
pub use file::FileIOReader;