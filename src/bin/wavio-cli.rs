use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use std::time::Instant;

use wavio::dsp::audio::load_wav;
use wavio::dsp::peaks::{extract_peaks, PeakExtractorConfig};
use wavio::dsp::spectrogram::{compute_spectrogram, SpectrogramConfig};
use wavio::hash::{generate_hashes, Fingerprint, HashConfig};
use wavio::index::Index;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

#[derive(Parser, Debug)]
#[command(author, version, about = "Peak-based audio fingerprinting CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Print verbose output (peak count, hash count, query time)
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Index a folder of audio files into a persistent database
    Index {
        /// Path to the database file
        #[arg(short, long)]
        db: PathBuf,

        /// Path to the audio file or directory to index
        #[arg(value_name = "INPUT_PATH")]
        input: PathBuf,
    },
    /// Query a clip against the database
    Query {
        /// Path to the database file
        #[arg(short, long)]
        db: PathBuf,

        /// Path to the audio clip to query
        #[arg(value_name = "FILE")]
        input: PathBuf,
    },
    /// Print information about a database
    Info {
        /// Path to the database file
        #[arg(short, long)]
        db: PathBuf,
    },
}

fn fingerprint_file(path: &Path) -> anyhow::Result<Vec<Fingerprint>> {
    let audio = load_wav(path.to_str().unwrap())?;
    
    let spec_config = SpectrogramConfig::default();
    let spec = compute_spectrogram(&audio.samples, &spec_config)?;

    let peak_config = PeakExtractorConfig::default();
    let peaks = extract_peaks(&spec, &peak_config);

    let hash_config = HashConfig::default();
    let hashes = generate_hashes(&peaks, &hash_config);

    Ok(hashes)
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Index { db, input } => {
            let mut files = Vec::new();
            if input.is_dir() {
                for entry in std::fs::read_dir(input)? {
                    let entry = entry?;
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(ext) = path.extension() {
                            if ext == "wav" {
                                files.push(path);
                            }
                        }
                    }
                }
            } else {
                files.push(input.clone());
            }

            println!("Found {} files to index...", files.len());

            let mut index = if db.exists() {
                Index::load_from_disk(db)?
            } else {
                Index::default()
            };

            let pb = ProgressBar::new(files.len() as u64);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
                    .unwrap()
                    .progress_chars("=>-"),
            );

            // Sequentially or parallel process based on feature.
            // Using a simple non-parallel loop first. Can parallelize extraction later.
            for file in files {
                if let Some(track_name) = file.file_stem().and_then(|s| s.to_str()) {
                    let name = track_name.to_string();
                    match fingerprint_file(&file) {
                        Ok(hashes) => {
                            if cli.verbose {
                                pb.println(format!("Indexed '{}': {} hashes", name, hashes.len()));
                            }
                            index.insert(&name, &hashes);
                        }
                        Err(e) => {
                            pb.println(format!("Failed to index '{}': {}", name, e));
                        }
                    }
                }
                pb.inc(1);
            }
            pb.finish_with_message("Indexing complete.");

            println!("Saving index to {:?}...", db);
            index.save_to_disk(db)?;
            println!("Done.");
        }
        Commands::Query { db, input } => {
            if !db.exists() {
                anyhow::bail!("Database file {:?} does not exist. Index first.", db);
            }
            let index = Index::load_from_disk(db)?;

            let start = Instant::now();
            let hashes = fingerprint_file(input)?;
            let fingerprint_time = start.elapsed();

            let query_start = Instant::now();
            let result = index.query(&hashes);
            let query_time = query_start.elapsed();

            if cli.verbose {
                println!(
                    "Extracted {} hashes in {:?}",
                    hashes.len(),
                    fingerprint_time
                );
                println!("Query performed in {:?}", query_time);
            }

            match result {
                Some(r) => {
                    println!("Match found: {}", r.track_id);
                    println!("Score: {}", r.score);
                    println!("Offset: {:.2}s", r.offset_secs);
                }
                None => {
                    println!("No match found.");
                }
            }
        }
        Commands::Info { db } => {
            if !db.exists() {
                anyhow::bail!("Database file {:?} does not exist.", db);
            }
            let index = Index::load_from_disk(db)?;
            println!("Database: {:?}", db);
            println!("Tracks indexed: {}", index.track_count());
            println!("Total hashes: {}", index.hash_count());
        }
    }

    Ok(())
}
