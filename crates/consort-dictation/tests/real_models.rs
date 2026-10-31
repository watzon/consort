//! A whole dictation with the real engine, the real cleanup runtime and a
//! recorded clip in place of the microphone. It downloads the models of the plan.
//!
//! ```sh
//! CONSORT_DICTATION_ENGINE=/path/to/sayso-engine \
//! CONSORT_DICTATION_DATA=/path/to/a/folder \
//! CONSORT_DICTATION_CLIP=/path/to/16khz-mono-int16.wav \
//! cargo test -p consort-dictation --test real_models -- --ignored --nocapture
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use consort_dictation::{Deps, Dictation, Phase, Preflight, Request, Settings, StopAfter, Update};
use sayso_sdk::{MicFeed, Microphone};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Clip(Vec<f32>, Mutex<Option<crossbeam_channel::Sender<Vec<f32>>>>);

struct ClipHandle(Arc<Clip>);

struct Lease(Arc<Clip>);

impl Drop for Lease {
    fn drop(&mut self) {
        self.0.1.lock().unwrap().take();
    }
}

impl Microphone for ClipHandle {
    fn open(&self) -> Result<MicFeed, String> {
        let (tx, frames) = crossbeam_channel::unbounded();
        *self.0.1.lock().unwrap() = Some(tx);
        Ok(MicFeed {
            frames,
            lease: Box::new(Lease(self.0.clone())),
        })
    }
}

struct Allowed;

impl Preflight for Allowed {
    fn check(&self, _: Option<&str>) -> Result<(), consort_dictation::Failure> {
        Ok(())
    }
}

fn read_wav(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    let at = bytes.windows(4).position(|w| w == b"data").unwrap();
    bytes[at + 8..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
        .collect()
}

/// The memory of this process's child programs, in MB.
fn children_mb() -> u64 {
    let me = std::process::id().to_string();
    let out = std::process::Command::new("ps")
        .args(["-axo", "ppid=,rss=,comm="])
        .output()
        .unwrap();
    let mut total = 0;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split_whitespace();
        if parts.next() == Some(me.as_str()) {
            let kb: u64 = parts.next().unwrap_or("0").parse().unwrap_or(0);
            println!("  {} MB {}", kb / 1024, parts.collect::<Vec<_>>().join(" "));
            total += kb;
        }
    }
    total / 1024
}

#[test]
#[ignore = "needs the engine, the runtime and about 1.3 GB of models"]
fn a_clip_becomes_clean_text() {
    let engine = PathBuf::from(std::env::var("CONSORT_DICTATION_ENGINE").unwrap());
    let data = PathBuf::from(std::env::var("CONSORT_DICTATION_DATA").unwrap());
    let clip = read_wav(&std::env::var("CONSORT_DICTATION_CLIP").unwrap());
    let language = std::env::var("CONSORT_DICTATION_LANGUAGE").unwrap_or_else(|_| "en".into());
    let mic = Arc::new(Clip(clip, Mutex::new(None)));
    let mut deps = Deps::real(engine, data);
    deps.preflight = Arc::new(Allowed);
    deps.microphone = Some(Arc::new(ClipHandle(mic.clone())));
    let dictation = Dictation::new(deps);
    let updates = dictation.updates();
    let settings = Settings {
        stop_after: StopAfter::Never,
        language,
        ..Settings::default()
    };

    let needs = dictation.needs(&settings).unwrap();
    println!("models: {needs:#?}");
    let missing: Vec<_> = needs.into_iter().filter(|n| !n.on_disk).collect();
    if !missing.is_empty() {
        dictation.download(missing);
        loop {
            match updates.recv().unwrap() {
                Update::DownloadsDone => break,
                Update::DownloadFailed(error) => panic!("{error}"),
                Update::DownloadModelDone { model } => println!("downloaded {model}"),
                _ => {}
            }
        }
    }

    let started = Instant::now();
    dictation.start(Request {
        settings: settings.clone(),
        key: None,
    });
    loop {
        match updates.recv().unwrap() {
            Update::Phase(Phase::Listening) => break,
            Update::Failed(failure) => panic!("{failure:?}"),
            _ => {}
        }
    }
    println!("listening after {:?}", started.elapsed());

    // Play the clip in real time, and stop one second after it.
    let player = mic.clone();
    let play = std::thread::spawn(move || {
        let silence = vec![0.0; 16_000];
        for chunk in player.0.chunks(320).chain(silence.chunks(320)) {
            if let Some(tx) = player.1.lock().unwrap().as_ref() {
                let _ = tx.send(chunk.to_vec());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let mut last_words = String::new();
    let mut first_words = None;
    let mut word_updates = 0;
    while !play.is_finished() {
        if let Ok(update) = updates.recv_timeout(Duration::from_millis(100))
            && let Update::Words(words) = update
        {
            first_words.get_or_insert_with(|| started.elapsed());
            word_updates += 1;
            last_words = words;
        }
    }
    println!("first words after {first_words:?}, {word_updates} updates");
    println!("live text: {last_words}");
    println!("memory while listening: {} MB", children_mb());
    let stop = Instant::now();
    dictation.stop();
    let end = loop {
        match updates.recv_timeout(Duration::from_secs(120)).unwrap() {
            Update::Done {
                text,
                cleanup_failed,
                ..
            } => break (text, cleanup_failed),
            Update::Failed(failure) => panic!("{failure:?}"),
            _ => {}
        }
    };
    println!("finished {:?} after the stop", stop.elapsed());
    println!("cleanup failed: {}", end.1);
    println!("final text: {}", end.0);
    println!("memory after cleanup: {} MB", children_mb());
    assert!(!end.0.trim().is_empty());
    assert!(
        !last_words.is_empty(),
        "the words came while the clip played"
    );
}
