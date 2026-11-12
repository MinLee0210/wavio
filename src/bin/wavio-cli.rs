// The CLI prints paths with `{:?}` on purpose (quoted, so paths with spaces are
// unambiguous, and tests assert on that exact wording); `main` is a flat
// subcommand dispatcher.
#![allow(clippy::unnecessary_debug_formatting, clippy::too_many_lines)]

use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use std::time::Instant;

use wavio::dsp::Fingerprinter;
use wavio::hash::Fingerprint;
use wavio::index::{QueryOptions, ScaleSearch};
use wavio::persist::PersistentIndex;
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
    /// setting it was indexed with; the database records the settings it was
    /// built with and this tool refuses to mix them.
    #[arg(long, global = true)]
    robust: bool,

    /// Index and query with **both** pairwise and triplet hashes. Best overall
    /// accuracy (robust to heavy noise *and* speed / pitch changes) at about
    /// 1.5x the index size. Takes precedence over `--robust`.
    #[arg(long, global = true)]
    hybrid: bool,
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

        /// Reject matches whose lead over the runner-up track is below this
        /// fraction of the winning score (0.0-1.0)
        #[arg(long, default_value_t = 0.0)]
        min_margin: f32,

        /// Also search playback-speed hypotheses (+-8%) and report the
        /// estimated speed of the clip relative to the indexed track
        #[arg(long)]
        detect_speed: bool,

        /// Weight each matching hash by its rarity, so hashes that occur in
        /// many places count for less. Helps the triplet/hybrid modes in noise
        #[arg(long)]
        idf: bool,
    },
    /// Print information about a database
    Info {
        /// Path to the database file
        #[arg(short, long)]
        db: PathBuf,
    },
}

fn make_fingerprinter(cli: &Cli) -> Fingerprinter {
    if cli.hybrid {
        Fingerprinter::default().with_hybrid_hashing(TripletHashConfig::default())
    } else if cli.robust {
        Fingerprinter::default()
            .with_triplet_hashing(TripletHashConfig::default())
            .with_peak_interpolation(true)
    } else {
        Fingerprinter::default()
    }
}

fn fingerprint_file(path: &Path, cli: &Cli) -> anyhow::Result<Vec<Fingerprint>> {
    let path = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("path {path:?} is not valid UTF-8"))?;
    Ok(make_fingerprinter(cli).fingerprint_file(path)?)
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
                                matches!(
                                    ext_lower.as_str(),
                                    "wav" | "mp3" | "flac" | "m4a" | "aac" | "ogg"
                                )
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

            let mut store = PersistentIndex::open(db)?;
            let signature = make_fingerprinter(&cli).signature();
            if store.signature().is_none() && store.track_count() > 0 {
                eprintln!(
                    "warning: {db:?} was created before settings were recorded, so it cannot be \
                     checked against the current settings. Assuming they match."
                );
            }
            store.ensure_signature(&signature)?;

            if !*force {
                files.retain(|file| {
                    let Some(name) = file.file_stem().and_then(|s| s.to_str()) else {
                        return true;
                    };
                    if store.contains_track(name) {
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
            // The results are then written to the database in one transaction.
            let named_files: Vec<(String, PathBuf)> = files
                .into_iter()
                .filter_map(|file| {
                    let name = file.file_stem().and_then(|s| s.to_str())?.to_string();
                    Some((name, file))
                })
                .collect();

            let work = |(name, file): &(String, PathBuf)| {
                let result = fingerprint_file(file, &cli);
                pb.inc(1);
                (name.clone(), result)
            };
            #[cfg(feature = "parallel")]
            let results: Vec<(String, anyhow::Result<Vec<Fingerprint>>)> =
                named_files.par_iter().map(work).collect();
            #[cfg(not(feature = "parallel"))]
            let results: Vec<(String, anyhow::Result<Vec<Fingerprint>>)> =
                named_files.iter().map(work).collect();

            let mut batch: Vec<(String, Vec<Fingerprint>)> = Vec::new();
            for (name, result) in results {
                match result {
                    Ok(hashes) => {
                        if cli.verbose {
                            pb.println(format!("Indexed '{}': {} hashes", name, hashes.len()));
                        }
                        batch.push((name, hashes));
                    }
                    Err(e) => {
                        pb.println(format!("Failed to index '{name}': {e}"));
                    }
                }
            }
            pb.finish_with_message("Indexing complete.");

            println!("Saving index to {db:?}...");
            if *force {
                for (name, _) in &batch {
                    store.remove_track(name)?;
                }
            }
            store.insert_many(&batch)?;
            println!("Done.");
        }
        Commands::Query {
            db,
            input,
            min_confidence,
            min_margin,
            detect_speed,
            idf,
        } => {
            if !db.exists() {
                anyhow::bail!("Database file {db:?} does not exist. Index first.");
            }
            let store = PersistentIndex::open(db)?;
            store.check_signature(&make_fingerprinter(&cli).signature())?;

            let start = Instant::now();
            let hashes = fingerprint_file(input, &cli)?;
            let fingerprint_time = start.elapsed();

            let query_start = Instant::now();
            let mut options = QueryOptions::new().with_idf_weighting(*idf);
            if *detect_speed {
                options = options.with_scale_search(ScaleSearch::default());
            }
            let result = store.query_with(&hashes, 1, &options).into_iter().next();
            let query_time = query_start.elapsed();

            if cli.verbose {
                println!(
                    "Extracted {} hashes in {:?}",
                    hashes.len(),
                    fingerprint_time
                );
                println!("Query performed in {query_time:?}");
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
                } else if r.margin < *min_margin {
                    println!(
                        "Best candidate '{}' leads the runner-up by only {:.1}% (threshold {:.1}%) — treating as no match.",
                        r.track_id,
                        r.margin * 100.0,
                        min_margin * 100.0
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
                    println!("Margin over runner-up: {:.1}%", r.margin * 100.0);
                    println!("Offset: {:.2}s", r.offset_secs);
                    if *detect_speed {
                        println!("Estimated speed: {:.2}x", r.speed);
                    }
                }
                None => {
                    println!("No match found.");
                }
            }
        }
        Commands::Info { db } => {
            if !db.exists() {
                anyhow::bail!("Database file {db:?} does not exist.");
            }
            let store = PersistentIndex::open(db)?;
            println!("Database: {db:?}");
            println!("Tracks indexed: {}", store.track_count());
            println!("Total hashes: {}", store.hash_count());
            match store.signature() {
                Some(sig) => println!("Built with: {sig}"),
                None => println!("Built with: (unknown — predates settings tracking)"),
            }
        }
    }

    Ok(())
}
