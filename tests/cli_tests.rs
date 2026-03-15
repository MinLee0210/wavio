use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

fn get_wav_path() -> PathBuf {
    // We assume there's a way to generate a simple WAV or we'll just test the error handling.
    // Given the difficulty of generating a WAV here without hound logic, 
    // let's test the CLI's basic existence and simple errors 
    // like missing database or invalid inputs.
    PathBuf::from("data/sample.wav")
}

#[test]
fn test_cli_info_no_db() {
    let mut cmd = Command::cargo_bin("wavio-cli").unwrap();
    cmd.arg("info")
       .arg("--db").arg("nonexistent_db_file.bin");
    
    cmd.assert()
       .failure()
       .stderr(predicates::str::contains("Database file \"nonexistent_db_file.bin\" does not exist."));
}

#[test]
fn test_cli_query_no_db() {
    let mut cmd = Command::cargo_bin("wavio-cli").unwrap();
    cmd.arg("query")
       .arg("--db").arg("nonexistent_db_file.bin")
       .arg("data/sample.wav");
    
    cmd.assert()
       .failure()
       .stderr(predicates::str::contains("Database file \"nonexistent_db_file.bin\" does not exist."));
}

#[test]
fn test_cli_index_creates_db() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test_index.db");

    let mut cmd = Command::cargo_bin("wavio-cli").unwrap();
    cmd.arg("index")
       .arg("--db").arg(&db_path)
       .arg(temp_dir.path()); // Empty directory, so 0 files indexed
    
    cmd.assert()
       .success()
       .stdout(predicates::str::contains("Saving index to"));

    // db file should exist
    assert!(db_path.exists());
}
