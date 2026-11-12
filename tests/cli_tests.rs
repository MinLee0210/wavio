#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::float_cmp,
    clippy::similar_names,
    clippy::too_many_lines
)]
use assert_cmd::Command;

#[test]
fn test_cli_info_no_db() {
    let mut cmd = Command::cargo_bin("wavio-cli").unwrap();
    cmd.arg("info").arg("--db").arg("nonexistent_db_file.bin");

    cmd.assert().failure().stderr(predicates::str::contains(
        "Database file \"nonexistent_db_file.bin\" does not exist.",
    ));
}

#[test]
fn test_cli_query_no_db() {
    let mut cmd = Command::cargo_bin("wavio-cli").unwrap();
    cmd.arg("query")
        .arg("--db")
        .arg("nonexistent_db_file.bin")
        .arg("data/sample.wav");

    cmd.assert().failure().stderr(predicates::str::contains(
        "Database file \"nonexistent_db_file.bin\" does not exist.",
    ));
}

#[test]
fn test_cli_index_creates_db() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test_index.db");

    let mut cmd = Command::cargo_bin("wavio-cli").unwrap();
    cmd.arg("index")
        .arg("--db")
        .arg(&db_path)
        .arg(temp_dir.path()); // Empty directory, so 0 files indexed

    cmd.assert()
        .success()
        .stdout(predicates::str::contains("Saving index to"));

    // db file should exist
    assert!(db_path.exists());
}

// ---------------------------------------------------------------------------
// End-to-end: real WAV files through the CLI
// ---------------------------------------------------------------------------

use std::path::Path;

const SR: u32 = 22_050;

/// Harmonic tones with slow modulation (rich enough to yield many peaks).
fn synth(seed: f32, secs: f32) -> Vec<f32> {
    let n = (secs * SR as f32) as usize;
    let freqs = [220.0_f32, 330.0, 440.0, 550.0, 660.0, 770.0, 880.0];
    (0..n)
        .map(|i| {
            let t = i as f32 / SR as f32;
            freqs
                .iter()
                .enumerate()
                .map(|(k, &f)| {
                    let phase = (k as f32 + seed) * 0.3;
                    let amp = 0.12 * (1.0 + 0.5 * (t * (0.5 + seed * 0.07) + phase).sin());
                    let vib = 1.0 + 0.02 * (t * (0.9 + seed * 0.11) + phase).sin();
                    amp * (std::f32::consts::TAU * f * (1.0 + seed * 0.013) * vib * t).sin()
                })
                .sum::<f32>()
                .clamp(-1.0, 1.0)
        })
        .collect()
}

fn write_wav(path: &Path, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for &s in samples {
        w.write_sample((s * f32::from(i16::MAX)) as i16).unwrap();
    }
    w.finalize().unwrap();
}

fn cli() -> Command {
    Command::cargo_bin("wavio-cli").unwrap()
}

fn stdout_of(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn hashes_in(info: &str) -> u64 {
    info.lines()
        .find_map(|l| l.strip_prefix("Total hashes: "))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[test]
fn test_cli_index_query_roundtrip_and_safety_checks() {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("music");
    std::fs::create_dir(&music).unwrap();
    let track_a = synth(1.0, 14.0);
    write_wav(&music.join("track_a.wav"), &track_a);
    write_wav(&music.join("track_b.wav"), &synth(5.0, 14.0));

    let clip = dir.path().join("clip.wav");
    write_wav(&clip, &track_a[3 * SR as usize..9 * SR as usize]); // 6 s from t=3 s

    let db = dir.path().join("lib.redb");

    // Index.
    stdout_of(cli().arg("index").arg("--db").arg(&db).arg(&music));
    let info = stdout_of(cli().arg("info").arg("--db").arg(&db));
    assert!(info.contains("Tracks indexed: 2"), "{info}");
    assert!(info.contains("mode=pairwise"), "{info}");
    let hashes_after_first = hashes_in(&info);
    assert!(hashes_after_first > 0);

    // Query finds the right track at roughly the right offset.
    let out = stdout_of(cli().arg("query").arg("--db").arg(&db).arg(&clip));
    assert!(out.contains("Match found: track_a"), "{out}");
    assert!(out.contains("Margin over runner-up:"), "{out}");
    let offset: f32 = out
        .lines()
        .find_map(|l| l.strip_prefix("Offset: "))
        .unwrap()
        .trim_end_matches('s')
        .parse()
        .unwrap();
    assert!((offset - 3.0).abs() < 0.2, "offset was {offset}");

    // Rarity weighting still finds the same track.
    let out = stdout_of(cli().args(["query", "--idf", "--db"]).arg(&db).arg(&clip));
    assert!(out.contains("Match found: track_a"), "{out}");

    // Re-indexing is a no-op (no duplicated hashes).
    let out = stdout_of(cli().arg("index").arg("--db").arg(&db).arg(&music));
    assert!(out.contains("already indexed"), "{out}");
    let info = stdout_of(cli().arg("info").arg("--db").arg(&db));
    assert_eq!(hashes_in(&info), hashes_after_first);

    // --force replaces rather than duplicates.
    stdout_of(
        cli()
            .arg("index")
            .arg("--force")
            .arg("--db")
            .arg(&db)
            .arg(&music),
    );
    let info = stdout_of(cli().arg("info").arg("--db").arg(&db));
    assert_eq!(hashes_in(&info), hashes_after_first);
    assert!(info.contains("Tracks indexed: 2"), "{info}");

    // Mixing hashing modes is refused loudly instead of returning "no match".
    cli()
        .args(["--robust", "query", "--db"])
        .arg(&db)
        .arg(&clip)
        .assert()
        .failure()
        .stderr(predicates::str::contains("Incompatible index"));
    cli()
        .args(["--robust", "index", "--db"])
        .arg(&db)
        .arg(&music)
        .assert()
        .failure()
        .stderr(predicates::str::contains("Incompatible index"));

    // An unrelated clip is rejected by --min-confidence.
    let other = dir.path().join("other.wav");
    write_wav(&other, &synth(9.0, 6.0));
    let out = stdout_of(
        cli()
            .args(["query", "--min-confidence", "0.5", "--db"])
            .arg(&db)
            .arg(&other),
    );
    assert!(out.contains("No match found"), "{out}");
}

#[test]
fn test_cli_hybrid_mode_and_speed_detection() {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("music");
    std::fs::create_dir(&music).unwrap();
    let track = synth(2.0, 16.0);
    write_wav(&music.join("song.wav"), &track);
    write_wav(&music.join("other.wav"), &synth(7.0, 16.0));
    let db = dir.path().join("hybrid.redb");

    stdout_of(
        cli()
            .args(["--hybrid", "index", "--db"])
            .arg(&db)
            .arg(&music),
    );
    let info = stdout_of(cli().arg("info").arg("--db").arg(&db));
    assert!(info.contains("mode=hybrid"), "{info}");

    // A 6 s clip played ~4% fast (resampled), cut from 2 s in.
    let clip: Vec<f32> = track[2 * SR as usize..8 * SR as usize].to_vec();
    let fast: Vec<f32> = (0..(clip.len() as f32 / 1.04) as usize)
        .map(|i| {
            let x = i as f32 * 1.04;
            let (i0, f) = (x as usize, x.fract());
            clip[i0] * (1.0 - f) + clip[(i0 + 1).min(clip.len() - 1)] * f
        })
        .collect();
    let clip_path = dir.path().join("fast.wav");
    write_wav(&clip_path, &fast);

    let out = stdout_of(
        cli()
            .args(["--hybrid", "query", "--detect-speed", "--db"])
            .arg(&db)
            .arg(&clip_path),
    );
    assert!(out.contains("Match found: song"), "{out}");
    let speed: f32 = out
        .lines()
        .find_map(|l| l.strip_prefix("Estimated speed: "))
        .unwrap()
        .trim_end_matches('x')
        .parse()
        .unwrap();
    assert!((speed - 1.04).abs() < 0.025, "estimated speed {speed}");

    // Plain-mode CLI must refuse the hybrid database.
    cli()
        .args(["query", "--db"])
        .arg(&db)
        .arg(&clip_path)
        .assert()
        .failure()
        .stderr(predicates::str::contains("Incompatible index"));
}
