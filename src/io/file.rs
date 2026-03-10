use super::base::IOReader;
use crate::dsp::audio;
use crate::error::WavioError;
use crate::utils;

/// Reads audio samples from a file on disk.
///
/// Validates the file path, then delegates to the DSP audio loader
/// to produce normalized mono `f32` samples.
pub struct FileIOReader {
    filepath: String,
}

impl IOReader for FileIOReader {
    fn read(&self) -> Result<Vec<f32>, WavioError> {
        // Validate the file first (exists, supported extension, readable).
        utils::validate_audio_file(&self.filepath)?;

        // Load and decode the WAV file.
        let audio_data = audio::load_wav(&self.filepath)?;
        Ok(audio_data.samples)
    }
}

impl FileIOReader {
    /// Creates a new `FileIOReader` for the given file path.
    #[must_use]
    pub fn new(filepath: &str) -> Self {
        Self {
            filepath: filepath.to_string(),
        }
    }
}
