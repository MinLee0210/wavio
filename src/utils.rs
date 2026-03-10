//! Shared utility functions for file validation and path handling.

use std::fs;
use std::path::Path;

use crate::error::WavioError;

/// Supported audio file extensions.
const SUPPORTED_EXTENSIONS: &[&str] = &["wav"];

/// Validates that a file path points to a readable, supported audio file.
///
/// Performs three checks in order:
/// 1. The file exists on disk.
/// 2. The file has a supported audio extension (currently `.wav`).
/// 3. The file can be opened for reading (permissions, not locked, etc.).
///
/// Returns the canonicalized path on success.
///
/// # Errors
///
/// - [`WavioError::FileNotFound`] if the path does not exist.
/// - [`WavioError::UnsupportedAudioFormat`] if the extension is missing or not supported.
/// - [`WavioError::FileNotReadable`] if the file cannot be opened for reading.
pub fn validate_audio_file(path: &str) -> Result<String, WavioError> {
    let file_path = Path::new(path);

    // 1. Existence check.
    if !file_path.exists() {
        return Err(WavioError::FileNotFound(path.to_string()));
    }

    // 2. Extension check.
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase);

    match ext {
        Some(ref e) if SUPPORTED_EXTENSIONS.contains(&e.as_str()) => {}
        Some(e) => {
            return Err(WavioError::UnsupportedAudioFormat(format!(
                "'.{e}' is not a supported audio format"
            )));
        }
        None => {
            return Err(WavioError::UnsupportedAudioFormat(
                "file has no extension".to_string(),
            ));
        }
    }

    // 3. Readability check -- attempt to open the file.
    fs::File::open(file_path).map_err(|e| {
        WavioError::FileNotReadable(format!("{path}: {e}"))
    })?;

    // Return the canonical path so downstream code works with absolute paths.
    file_path
        .canonicalize()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| WavioError::IoError(format!("{path}: {e}")))
}
