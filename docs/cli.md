# CLI Reference

The `wavio-cli` binary provides three subcommands for indexing, querying, and inspecting fingerprint databases.

!!! info "Installation"
    ```bash
    cargo build --release --bin wavio-cli --features persist
    ```

---

## Global Flags

| Flag | Description |
|------|-------------|
| `--verbose`, `-v` | Print detailed output (peak count, hash count, timing) |
| `--help`, `-h` | Show help |
| `--version`, `-V` | Show version |

---

## `index`

Index WAV files into a persistent database.

```bash
wavio-cli index --db <DB_PATH> <INPUT_PATH>
```

- `INPUT_PATH` can be a single WAV file or a directory
- When given a directory, indexes all `.wav` files found
- If the database already exists, new tracks are added to it

### Examples

```bash
# Index a single file
wavio-cli index --db music.db song.wav

# Index an entire folder
wavio-cli index --db music.db ./music/

# Verbose — see hash counts per track
wavio-cli --verbose index --db music.db ./music/
# Indexed 'hotel_california': 4521 hashes
# Indexed 'bohemian_rhapsody': 6832 hashes
# ...
```

A progress bar is shown during batch indexing.

---

## `query`

Identify an audio clip against the database.

```bash
wavio-cli query --db <DB_PATH> <FILE>
```

### Output

```
Match found: hotel_california
Score: 312
Offset: 47.35s
```

If no match is found:

```
No match found.
```

### Examples

```bash
# Basic query
wavio-cli query --db music.db clip.wav

# Verbose — see fingerprinting time and query time
wavio-cli --verbose query --db music.db clip.wav
# Extracted 842 hashes in 23.4ms
# Query performed in 0.12ms
# Match found: hotel_california
# Score: 312
# Offset: 47.35s
```

---

## `info`

Print database statistics.

```bash
wavio-cli info --db <DB_PATH>
```

### Output

```
Database: "./music.db"
Tracks indexed: 142
Total hashes: 581,493
```

---

## Error Messages

The CLI is designed to produce human-readable error messages:

```bash
$ wavio-cli query --db nonexistent.db clip.wav
Error: Database file "nonexistent.db" does not exist. Index first.

$ wavio-cli query --db music.db missing.wav
Error: File not found: missing.wav
```
