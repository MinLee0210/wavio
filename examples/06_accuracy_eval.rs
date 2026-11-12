//! Accuracy evaluation harness.
//!
//! Measures how often a short, degraded clip is identified correctly, for
//! each hashing mode, across clip lengths and distortions.
//!
//! ```text
//! cargo run --release --features parallel --example 06_accuracy_eval
//! cargo run --release --features parallel --example 06_accuracy_eval -- --dir data/real_eval
//! ```
//!
//! With `--dir DIR` (WAV files, e.g. from `scripts/prepare_real_eval.sh`) the
//! library is *real recordings* and queries take the realistic path: cut from
//! the original, converted to 48 kHz by `ffmpeg`, optionally MP3-compressed,
//! then loaded and resampled by wavio. This needs `ffmpeg` on the `PATH`, and
//! additionally compares wavio's linear and windowed-sinc resamplers.
//!
//! **Read this before quoting the numbers:** the library is *synthetic*
//! (random melodies with harmonics and percussive bursts). That makes the run
//! fast, deterministic and dependency-free, and it is a fair test of
//! *relative* behavior (pairwise vs triplet, clip length, distortion
//! strength), but absolute accuracy on real music will differ. Treat it as a
//! regression harness, not a leaderboard.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::type_complexity,
    clippy::many_single_char_names
)]

use wavio::dsp::Fingerprinter;
use wavio::dsp::audio::{resample, resample_linear};
use wavio::index::{Index, QueryOptions, ScaleSearch};
use wavio::triplet::TripletHashConfig;

const SR: u32 = 22_050;
const N_TRACKS: usize = 40;
const TRACK_SECS: f32 = 30.0;
const CLIP_LENGTHS: [f32; 3] = [3.0, 5.0, 10.0];

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1_u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

/// A random melody: notes of 0.2-0.6 s drawn from a per-track scale, each
/// with its own harmonic balance, plus occasional percussive noise bursts.
fn synth_track(seed: u64) -> Vec<f32> {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let root = rng.range(110.0, 330.0);
    let scale: Vec<f32> = (0..7)
        .map(|_| root * 2.0_f32.powf(rng.range(0.0, 36.0).round() / 12.0))
        .collect();
    let n = (TRACK_SECS * SR as f32) as usize;
    let mut out = vec![0.0_f32; n];

    let mut t0 = 0usize;
    while t0 < n {
        let len = ((rng.range(0.2, 0.6) * SR as f32) as usize).min(n - t0);
        let f = scale[(rng.next() % scale.len() as u64) as usize];
        let harm: Vec<f32> = (0..4).map(|_| rng.range(0.1, 1.0)).collect();
        let drum = rng.unit() < 0.25;
        for i in 0..len {
            let t = i as f32 / SR as f32;
            let env = (1.0 - i as f32 / len as f32).powf(0.7) * (i as f32 / 200.0).min(1.0);
            let mut s: f32 = harm
                .iter()
                .enumerate()
                .map(|(h, a)| a * (std::f32::consts::TAU * f * (h + 1) as f32 * t).sin())
                .sum();
            if drum && i < len / 4 {
                s += 3.0 * (rng.unit() - 0.5);
            }
            out[t0 + i] = 0.12 * env * s;
        }
        t0 += len;
    }
    out
}

fn add_noise(clip: &[f32], snr_db: f32, seed: u64) -> Vec<f32> {
    let power = clip.iter().map(|s| s * s).sum::<f32>() / clip.len() as f32;
    let amp = (power / 10.0_f32.powf(snr_db / 10.0)).sqrt() * 3.0_f32.sqrt();
    let mut rng = Rng(seed | 1);
    clip.iter()
        .map(|s| s + amp * (rng.unit() * 2.0 - 1.0))
        .collect()
}

/// Speed change: pitch and tempo move together (like a tape / vinyl).
fn speed(clip: &[f32], factor: f32) -> Vec<f32> {
    resample(clip, (SR as f32 * factor) as u32, SR)
}

fn lowpass(clip: &[f32], passes: usize) -> Vec<f32> {
    let mut y = clip.to_vec();
    for _ in 0..passes {
        let mut prev = 0.0;
        for s in &mut y {
            prev += 0.35 * (*s - prev);
            *s = prev;
        }
    }
    y
}

type Distortion = (&'static str, Box<dyn Fn(&[f32], u64) -> Vec<f32>>);

fn distortions() -> Vec<Distortion> {
    vec![
        ("clean", Box::new(|c, _| c.to_vec())),
        ("noise 20 dB", Box::new(|c, s| add_noise(c, 20.0, s))),
        ("noise 10 dB", Box::new(|c, s| add_noise(c, 10.0, s))),
        ("noise 0 dB", Box::new(|c, s| add_noise(c, 0.0, s))),
        ("lowpass", Box::new(|c, _| lowpass(c, 3))),
        ("speed +2%", Box::new(|c, _| speed(c, 1.02))),
        ("speed -3%", Box::new(|c, _| speed(c, 0.97))),
        ("speed +6%", Box::new(|c, _| speed(c, 1.06))),
        ("speed +9%", Box::new(|c, _| speed(c, 1.09))),
        (
            "noise10+spd2",
            Box::new(|c, s| add_noise(&speed(c, 1.02), 10.0, s)),
        ),
    ]
}

struct Mode {
    name: &'static str,
    fingerprinter: Fingerprinter,
    scaled: bool,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(pos) = args.iter().position(|a| a == "--dir") {
        let dir = args.get(pos + 1).expect("--dir needs a directory");
        let filter = args
            .iter()
            .enumerate()
            .find(|(i, a)| *i != pos && *i != pos + 1 && !a.starts_with("--"))
            .map(|(_, a)| a.clone())
            .unwrap_or_default();
        real::run(dir, &filter);
        return;
    }
    synthetic_main();
}

fn synthetic_main() {
    let tracks: Vec<Vec<f32>> = (0..N_TRACKS as u64).map(synth_track).collect();
    // Variants to compare. Pass a substring as the first CLI argument to run
    // only the matching ones (e.g. `-- interp`).
    let triplet = |f: f32, t: f32| TripletHashConfig::new(15, 0.0, 1.0, f, t);
    let mk = |name: &'static str, fp: Fingerprinter| Mode {
        name,
        fingerprinter: fp,
        scaled: false,
    };
    let tp = |cfg: TripletHashConfig, interp: bool| {
        Fingerprinter::default()
            .with_triplet_hashing(cfg)
            .with_peak_interpolation(interp)
    };
    let filter = std::env::args().nth(1).unwrap_or_default();
    let modes: Vec<Mode> = vec![
        mk("pairwise", Fingerprinter::default()),
        mk("triplet (v0.3 behavior)", tp(triplet(0.01, 0.01), false)),
        mk(
            "triplet + freq interpolation",
            tp(triplet(0.01, 0.01), true),
        ),
        mk(
            "hybrid (pairwise + interpolated triplet)",
            Fingerprinter::default().with_hybrid_hashing(triplet(0.01, 0.01)),
        ),
        Mode {
            name: "hybrid + time-scale voting",
            fingerprinter: Fingerprinter::default().with_hybrid_hashing(triplet(0.01, 0.01)),
            scaled: true,
        },
    ]
    .into_iter()
    .filter(|m| m.name.contains(&filter))
    .collect();
    let distortions = distortions();

    println!(
        "Synthetic library: {N_TRACKS} tracks x {TRACK_SECS} s. One query per track per cell \
         (random offset). Cells show top-1 accuracy, %.\n"
    );

    for mode in &modes {
        let mut index = Index::default();
        let mut total_hashes = 0;
        for (i, t) in tracks.iter().enumerate() {
            let fps = mode.fingerprinter.fingerprint(t).expect("fingerprint");
            total_hashes += fps.len();
            index.insert(&format!("t{i}"), &fps);
        }
        println!(
            "== {} hashing ({} hashes, {:.0}/track) ==",
            mode.name,
            total_hashes,
            total_hashes as f32 / N_TRACKS as f32
        );
        print!("{:<14}", "distortion");
        for len in CLIP_LENGTHS {
            print!("{:>9}", format!("{len:.0} s"));
        }
        println!();

        let mut speed_report: Vec<String> = Vec::new();
        for (dname, distort) in &distortions {
            print!("{dname:<14}");
            let mut speeds: Vec<f32> = Vec::new();
            for (li, &len) in CLIP_LENGTHS.iter().enumerate() {
                let mut correct = 0;
                for (i, track) in tracks.iter().enumerate() {
                    let mut rng = Rng((i as u64 + 1) * 7919 + li as u64 * 104_729);
                    let max_start = TRACK_SECS - len - 0.5;
                    let start = (rng.range(0.0, max_start) * SR as f32) as usize;
                    let clip = &track[start..start + (len * SR as f32) as usize];
                    let degraded = distort(clip, i as u64 * 31 + 5);
                    if let Ok(fps) = mode.fingerprinter.fingerprint(&degraded) {
                        let result = if mode.scaled {
                            index.query_scaled(&fps, &ScaleSearch::default())
                        } else {
                            index.query(&fps)
                        };
                        if let Some(r) = result.as_ref().filter(|r| r.track_id == format!("t{i}")) {
                            speeds.push(r.speed);
                        }
                        if result.is_some_and(|r| r.track_id == format!("t{i}")) {
                            correct += 1;
                        }
                    }
                }
                print!("{:>8.0}%", 100.0 * f64::from(correct) / N_TRACKS as f64);
            }
            println!();
            if mode.scaled && dname.starts_with("speed") && !speeds.is_empty() {
                speeds.sort_by(f32::total_cmp);
                speed_report.push(format!("{dname} -> {:.2}", speeds[speeds.len() / 2]));
            }
        }
        if !speed_report.is_empty() {
            println!("estimated speed (median): {}", speed_report.join(", "));
        }
        println!();
    }
}

// ---------------------------------------------------------------------------
// Real-recording mode
// ---------------------------------------------------------------------------

mod real {
    use super::{
        Fingerprinter, Index, QueryOptions, Rng, SR, TripletHashConfig, add_noise, resample,
        resample_linear, speed,
    };
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    const CLIP_LENGTHS: [f32; 3] = [2.0, 3.0, 5.0];
    const QUERY_RATE: u32 = 48_000;

    type Resampler = fn(&[f32], u32, u32) -> Vec<f32>;

    struct Source {
        name: String,
        samples: Vec<f32>,
        rate: u32,
        path: PathBuf,
        secs: f32,
    }

    /// A query clip as it arrives from the outside world: mono, 48 kHz.
    struct Clip {
        distortion: &'static str,
        len_idx: usize,
        track: usize,
        samples: Vec<f32>,
        /// Applied after resampling to the internal rate.
        post: Option<fn(&[f32], u64) -> Vec<f32>>,
    }

    fn read_wav_mono(path: &Path) -> (Vec<f32>, u32) {
        let mut reader = hound::WavReader::open(path).expect("open wav");
        let spec = reader.spec();
        let ch = usize::from(spec.channels);
        let interleaved: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().map(Result::unwrap).collect(),
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1_i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| s.unwrap() as f32 * scale)
                    .collect()
            }
        };
        let mono = interleaved
            .chunks_exact(ch)
            .map(|f| f.iter().sum::<f32>() / ch as f32)
            .collect();
        (mono, spec.sample_rate)
    }

    fn ffmpeg(args: &[&str]) {
        let status = Command::new("ffmpeg")
            .args(["-v", "error", "-y"])
            .args(args)
            .stdin(Stdio::null())
            .status()
            .expect("failed to run ffmpeg (is it installed?)");
        assert!(status.success(), "ffmpeg {args:?} failed");
    }

    /// Cuts `[start, start+len)` from `src` as a 48 kHz mono clip, optionally
    /// round-tripped through MP3 at `kbps`.
    fn make_clip(src: &Path, start: f32, len: f32, mp3_kbps: Option<u32>, tmp: &Path) -> Vec<f32> {
        let (s, l) = (format!("{start:.3}"), format!("{len:.3}"));
        let src = src.to_str().unwrap();
        let rate = QUERY_RATE.to_string();
        let wav = tmp.join("clip.wav");
        match mp3_kbps {
            None => ffmpeg(&[
                "-ss",
                &s,
                "-t",
                &l,
                "-i",
                src,
                "-ac",
                "1",
                "-ar",
                &rate,
                "-c:a",
                "pcm_s16le",
                wav.to_str().unwrap(),
            ]),
            Some(kbps) => {
                let mp3 = tmp.join("clip.mp3");
                ffmpeg(&[
                    "-ss",
                    &s,
                    "-t",
                    &l,
                    "-i",
                    src,
                    "-ac",
                    "1",
                    "-ar",
                    &rate,
                    "-b:a",
                    &format!("{kbps}k"),
                    mp3.to_str().unwrap(),
                ]);
                ffmpeg(&[
                    "-i",
                    mp3.to_str().unwrap(),
                    "-ac",
                    "1",
                    "-ar",
                    &rate,
                    "-c:a",
                    "pcm_s16le",
                    wav.to_str().unwrap(),
                ]);
            }
        }
        let (samples, r) = read_wav_mono(&wav);
        assert_eq!(r, QUERY_RATE);
        samples
    }

    fn noise10(c: &[f32], seed: u64) -> Vec<f32> {
        add_noise(c, 10.0, seed)
    }
    fn noise0(c: &[f32], seed: u64) -> Vec<f32> {
        add_noise(c, 0.0, seed)
    }
    fn speed3(c: &[f32], _: u64) -> Vec<f32> {
        speed(c, 1.03)
    }

    pub fn run(dir: &str, filter: &str) {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .expect("read --dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")))
            .collect();
        paths.sort();
        assert!(!paths.is_empty(), "no .wav files in {dir}");

        let sources: Vec<Source> = paths
            .iter()
            .map(|p| {
                let (samples, rate) = read_wav_mono(p);
                let secs = samples.len() as f32 / rate as f32;
                Source {
                    name: p.file_stem().unwrap().to_string_lossy().into_owned(),
                    samples,
                    rate,
                    path: p.clone(),
                    secs,
                }
            })
            .collect();
        let total: f32 = sources.iter().map(|s| s.secs).sum();
        println!(
            "Real library: {} tracks, {:.0} s total ({} Hz sources). Queries are cut from the \
             originals, converted to {} Hz by ffmpeg, optionally MP3-encoded, then loaded and \
             resampled by wavio.\n",
            sources.len(),
            total,
            sources[0].rate,
            QUERY_RATE
        );

        // ---- Build the query clips once (ffmpeg is the slow part). ----
        let tmp = std::env::temp_dir().join(format!("wavio_eval_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let mut clips: Vec<Clip> = Vec::new();
        for (ti, src) in sources.iter().enumerate() {
            for (li, &len) in CLIP_LENGTHS.iter().enumerate() {
                let mut rng = Rng((ti as u64 + 1) * 7919 + li as u64 * 104_729);
                let start = rng.range(0.0, (src.secs - len - 0.3).max(0.0));
                let mut push = |distortion, samples: Vec<f32>, post| {
                    clips.push(Clip {
                        distortion,
                        len_idx: li,
                        track: ti,
                        samples,
                        post,
                    });
                };
                let clean = make_clip(&src.path, start, len, None, &tmp);
                push("48k clean", clean.clone(), None);
                push(
                    "48k + noise 10 dB",
                    clean.clone(),
                    Some(noise10 as fn(&[f32], u64) -> Vec<f32>),
                );
                push(
                    "48k + noise 0 dB",
                    clean.clone(),
                    Some(noise0 as fn(&[f32], u64) -> Vec<f32>),
                );
                push(
                    "48k + speed +3%",
                    clean,
                    Some(speed3 as fn(&[f32], u64) -> Vec<f32>),
                );
                push(
                    "mp3 128k",
                    make_clip(&src.path, start, len, Some(128), &tmp),
                    None,
                );
                push(
                    "mp3 64k",
                    make_clip(&src.path, start, len, Some(64), &tmp),
                    None,
                );
                push(
                    "mp3 32k",
                    make_clip(&src.path, start, len, Some(32), &tmp),
                    None,
                );
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);

        let mut distortion_names: Vec<&'static str> = Vec::new();
        for c in &clips {
            if !distortion_names.contains(&c.distortion) {
                distortion_names.push(c.distortion);
            }
        }

        let triplet = TripletHashConfig::default();
        let triplet_fp = Fingerprinter::default()
            .with_triplet_hashing(triplet.clone())
            .with_peak_interpolation(true);
        let hybrid_fp = Fingerprinter::default().with_hybrid_hashing(triplet.clone());
        // (name, fingerprinter, rarity weighting)
        let modes: Vec<(&str, Fingerprinter, bool)> = vec![
            ("pairwise", Fingerprinter::default(), false),
            ("pairwise + idf", Fingerprinter::default(), true),
            (
                "triplet no-interp",
                Fingerprinter::default().with_triplet_hashing(triplet),
                false,
            ),
            ("triplet + interp", triplet_fp.clone(), false),
            ("triplet + interp + idf", triplet_fp, true),
            ("hybrid", hybrid_fp.clone(), false),
            ("hybrid + idf", hybrid_fp, true),
        ];
        let resamplers: [(&str, Resampler); 2] = [
            ("linear resampler", resample_linear),
            ("sinc resampler", resample),
        ];

        for (rname, rs) in resamplers {
            // Library: each source resampled to the internal rate.
            let library: Vec<Vec<f32>> =
                sources.iter().map(|s| rs(&s.samples, s.rate, SR)).collect();

            for (mname, fp, idf) in &modes {
                let label = format!("{mname} / {rname}");
                if !label.contains(filter) {
                    continue;
                }
                let mut index = Index::default();
                for (src, samples) in sources.iter().zip(&library) {
                    index.insert(&src.name, &fp.fingerprint(samples).expect("fingerprint"));
                }

                println!("== {label} ==");
                print!("{:<20}", "distortion");
                for len in CLIP_LENGTHS {
                    print!("{:>9}", format!("{len:.0} s"));
                }
                println!("{:>9}", "all");
                for &dname in &distortion_names {
                    print!("{dname:<20}");
                    let (mut hit_all, mut n_all) = (0, 0);
                    for li in 0..CLIP_LENGTHS.len() {
                        let (mut hit, mut n) = (0, 0);
                        for c in clips
                            .iter()
                            .filter(|c| c.distortion == dname && c.len_idx == li)
                        {
                            let mut q = rs(&c.samples, QUERY_RATE, SR);
                            if let Some(post) = c.post {
                                q = post(&q, c.track as u64 * 31 + 5);
                            }
                            n += 1;
                            if let Ok(fps) = fp.fingerprint(&q) {
                                let opts = QueryOptions::new().with_idf_weighting(*idf);
                                if index
                                    .query_with(&fps, 1, &opts)
                                    .first()
                                    .is_some_and(|r| r.track_id == sources[c.track].name)
                                {
                                    hit += 1;
                                }
                            }
                        }
                        print!("{:>8.0}%", 100.0 * f64::from(hit) / f64::from(n));
                        hit_all += hit;
                        n_all += n;
                    }
                    println!("{:>8.0}%", 100.0 * f64::from(hit_all) / f64::from(n_all));
                }
                println!();
            }
        }
    }
}
