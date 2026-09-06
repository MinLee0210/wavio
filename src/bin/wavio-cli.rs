use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use std::time::Instant;

use wavio::dsp::Fingerprinter;
use wavio::hash::Fingerprint;
use wavio::index::Index;
use wavio::triplet::TripletHashConfig;

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

    /// Use pitch-shift / time-stretch-robust triplet hashing instead of the
    /// default pairwise hashing. A database must be queried with the same
    /// setting it was indexed with -- mixing modes silently degrades to "no
    /// match" rather than erroring, since the on-disk hashes don't record
    /// which mode produced them.
    #[arg(long, global = true)]
    robust: bool,
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

        /// Re-index tracks that are already present in the database
        /// (by default, already-indexed tracks are skipped to avoid
        /// duplicating their hashes).
        #[arg(long)]
        force: bool,
    },
    /// Query a clip against the database
    Query {
        /// Path to the database file
        #[arg(short, long)]
        db: PathBuf,

        /// Path to the audio clip to query
        #[arg(value_name = "FILE")]
        input: PathBuf,

        /// Reject matches whose confidence is below this threshold (0.0-1.0)
        #[arg(long, default_value_t = 0.0)]
        min_confidence: f32,
    },
    /// Print information about a database
    Info {
        /// Path to the database file
        #[arg(short, long)]
        db: PathBuf,
    },
}

fn fingerprint_file(path: &Path, robust: bool) -> anyhow::Result<Vec<Fingerprint>> {
    let fingerprinter = if robust {
        Fingerprinter::default().with_triplet_hashing(TripletHashConfig::default())
    } else {
        Fingerprinter::default()
    };
    let hashes = fingerprinter.fingerprint_file(path.to_str().unwrap())?;
    Ok(hashes)
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Index { db, input, force } => {
            let mut files = Vec::new();
            if input.is_dir() {
                for entry in std::fs::read_dir(input)? {
                    let entry = entry?;
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                            let ext_lower = ext.to_lowercase();
                            let is_supported = if cfg!(feature = "symphonia") {
                                matches!(ext_lower.as_str(), "wav" | "mp3" | "flac" | "m4a" | "aac" | "ogg")
                            } else {
                                ext_lower == "wav"
                            };
                            if is_supported {
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

            if !*force {
                files.retain(|file| {
                    let Some(name) = file.file_stem().and_then(|s| s.to_str()) else {
                        return true;
                    };
                    if index.contains_track(name) {
                        println!("Skipping '{name}': already indexed (use --force to re-index).");
                        false
                    } else {
                        true
                    }
                });
            }

            let pb = ProgressBar::new(files.len() as u64);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
                    .unwrap()
                    .progress_chars("=>-"),
            );

            // Fingerprinting (load + FFT + peak extraction + hashing) is the
            // CPU-heavy, per-file-independent part of indexing, so it runs
            // across the thread pool when the `parallel` feature is enabled.
            // Insertion into the index is done afterward, serially, since it
            // mutates shared state.
            #[cfg(feature = "parallel")]
            let named_files: Vec<(String, PathBuf)> = files
                .into_iter()
                .filter_map(|file| {
                    let name = file.file_stem().and_then(|s| s.to_str())?.to_string();
                    Some((name, file))
                })
                .collect();

            #[cfg(feature = "parallel")]
            let results: Vec<(String, anyhow::Result<Vec<Fingerprint>>)> = named_files
                .par_iter()
                .map(|(name, file)| {
                    let result = fingerprint_file(file, cli.robust);
                    pb.inc(1);
                    (name.clone(), result)
                })
                .collect();

            #[cfg(not(feature = "parallel"))]
            let results: Vec<(String, anyhow::Result<Vec<Fingerprint>>)> = files
                .iter()
                .filter_map(|file| {
                    file.file_stem().and_then(|s| s.to_str()).map(|track_name| {
                        let name = track_name.to_string();
                        let result = fingerprint_file(file, cli.robust);
                        pb.inc(1);
                        (name, result)
                    })
                })
                .collect();

            for (name, result) in results {
                match result {
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
            pb.finish_with_message("Indexing complete.");

            println!("Saving index to {:?}...", db);
            index.save_to_disk(db)?;
            println!("Done.");
        }
        Commands::Query {
            db,
            input,
            min_confidence,
        } => {
            if !db.exists() {
                anyhow::bail!("Database file {:?} does not exist. Index first.", db);
            }
            let index = Index::load_from_disk(db)?;

            let start = Instant::now();
            let hashes = fingerprint_file(input, cli.robust)?;
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

            let result = result.filter(|r| {
                if r.confidence < *min_confidence {
                    println!(
                        "Best candidate '{}' at {:.1}% confidence is below threshold ({:.1}%) — treating as no match.",
                        r.track_id,
                        r.confidence * 100.0,
                        min_confidence * 100.0
                    );
                    false
                } else {
                    true
                }
            });

            match result {
                Some(r) => {
                    println!("Match found: {}", r.track_id);
                    println!("Score: {}", r.score);
                    println!("Confidence: {:.1}%", r.confidence * 100.0);
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
