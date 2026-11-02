//! The behavior of a dictation, with a fake engine and real live sessions.

use crate::*;
use crossbeam_channel::{Receiver, Sender, unbounded};
use sayso_sdk::cleanup::{CleanError, Cleaner, CleanupRequest, CleanupResponse};
use sayso_sdk::embedded::{
    AudioFeed, CapturedAudio, DownloadProgress, EngineError, EngineEvent, EngineModels, FeedItem,
    Microphone, ModelId, ModelInfo, ModelState, OpenError, Role, Segment, SpeechEngine, SpeechPart,
    Turn,
};
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const RATE: usize = 16_000;

fn model(id: &str, name: &str, size: u64, roles: Vec<Role>, on_disk: bool) -> ModelInfo {
    ModelInfo {
        id: id.into(),
        name: name.into(),
        size_bytes: size,
        languages: vec!["en".into(), "de".into()],
        roles,
        state: if on_disk {
            ModelState::Downloaded
        } else {
            ModelState::NotDownloaded
        },
        recommended: false,
    }
}

fn mac_catalog(on_disk: bool) -> Vec<ModelInfo> {
    vec![
        model(
            "parakeet-tdt-v3",
            "Parakeet TDT v3",
            483_000_000,
            vec![Role::Final],
            on_disk,
        ),
        model(
            "parakeet-eou-120m",
            "Parakeet EOU 120M",
            222_000_000,
            vec![Role::Streaming],
            on_disk,
        ),
        model(
            "nemotron-multilingual",
            "Nemotron Multilingual",
            664_000_000,
            vec![Role::Streaming],
            on_disk,
        ),
    ]
}

#[derive(Default)]
struct Fake {
    watchers: Mutex<Vec<Sender<EngineEvent>>>,
    catalog: Mutex<Vec<ModelInfo>>,
    finals: Mutex<Vec<&'static str>>,
    final_models: Mutex<Vec<String>>,
    unloaded: Mutex<Vec<String>>,
    downloads: Mutex<Vec<String>>,
    load_wait: Mutex<Duration>,
    stream_words: Mutex<String>,
}

impl SpeechEngine for Fake {
    fn load(&self, _: &ModelId) -> Result<(), EngineError> {
        let wait = *self.load_wait.lock().unwrap();
        std::thread::sleep(wait);
        Ok(())
    }
    fn transcribe(&self, _: &[f32], _: &ModelId, _: &str) -> Result<String, EngineError> {
        Err(EngineError::new("not used"))
    }
    fn transcribe_segments(
        &self,
        samples: &[f32],
        model: &ModelId,
        _: &str,
    ) -> Result<Vec<Segment>, EngineError> {
        self.final_models
            .lock()
            .unwrap()
            .push(model.as_str().to_owned());
        let mut finals = self.finals.lock().unwrap();
        let text = if finals.is_empty() {
            "Final words.".to_owned()
        } else {
            finals.remove(0).to_owned()
        };
        Ok(vec![Segment {
            start_ms: 0,
            end_ms: samples.len() as u64 / 16,
            text,
            words: None,
        }])
    }
    fn detect_speech(&self, _: &ModelId, _: &[f32]) -> Result<Vec<SpeechPart>, EngineError> {
        Ok(Vec::new())
    }
    fn diarize(&self, _: &ModelId, _: &[f32], _: Option<u32>) -> Result<Vec<Turn>, EngineError> {
        Ok(Vec::new())
    }
    fn start_stream(&self, _: u64, _: &ModelId, _: &str) -> Result<(), EngineError> {
        Ok(())
    }
    fn start_vad(&self, _: u64, _: &ModelId) -> Result<(), EngineError> {
        Ok(())
    }
    fn push_audio(&self, session: u64, _: &[f32]) -> Result<(), EngineError> {
        let tentative = self.stream_words.lock().unwrap().clone();
        let event = EngineEvent::Partial {
            session,
            committed: String::new(),
            tentative,
        };
        self.watchers
            .lock()
            .unwrap()
            .retain(|w| w.send(event.clone()).is_ok());
        Ok(())
    }
    fn end_stream(&self, _: u64) {}
    fn end_vad(&self, _: u64) -> Result<(), EngineError> {
        Ok(())
    }
    fn events(&self) -> Receiver<EngineEvent> {
        let (tx, rx) = unbounded();
        self.watchers.lock().unwrap().push(tx);
        rx
    }
}

impl EngineModels for Fake {
    fn catalog(&self) -> Result<Vec<ModelInfo>, EngineError> {
        Ok(self.catalog.lock().unwrap().clone())
    }
    fn download(
        &self,
        model: &str,
        _: Option<u64>,
        on_progress: &mut dyn FnMut(&DownloadProgress),
    ) -> Result<(), EngineError> {
        self.downloads.lock().unwrap().push(model.to_string());
        on_progress(&DownloadProgress {
            model: model.into(),
            fraction: 1.0,
            bytes_done: 1,
            bytes_total: 1,
        });
        Ok(())
    }
    fn delete(&self, _: &str) -> Result<(), EngineError> {
        Ok(())
    }
    fn load(&self, _: &str) -> Result<(), EngineError> {
        Ok(())
    }
    fn unload(&self, model: &str) -> Result<(), EngineError> {
        self.unloaded.lock().unwrap().push(model.to_string());
        Ok(())
    }
}

/// A microphone that plays the audio of a test.
#[derive(Default)]
struct Mic {
    frames: Mutex<Option<Sender<FeedItem>>>,
}

struct MicHandle(Arc<Mic>);

impl Microphone for MicHandle {
    fn open(&self) -> Result<AudioFeed, OpenError> {
        let (tx, items) = unbounded();
        *self.0.frames.lock().unwrap() = Some(tx);
        Ok(AudioFeed {
            items,
            lease: Box::new(Lease(self.0.clone())),
            excludes: Vec::new(),
        })
    }
}

/// Dropping it closes the channel of frames, as the real capture does.
struct Lease(Arc<Mic>);

impl Drop for Lease {
    fn drop(&mut self) {
        self.0.frames.lock().unwrap().take();
    }
}

impl Mic {
    /// Sends audio. The session reads it as if it came at this speed.
    fn play(&self, samples: Vec<f32>) {
        let guard = self.frames.lock().unwrap();
        let tx = guard.as_ref().expect("the microphone is open");
        for chunk in samples.chunks(320) {
            tx.send(FeedItem::Audio(CapturedAudio::untimed(chunk.to_vec())))
                .unwrap();
        }
    }
}

/// Something like speech: a voice of 140 Hz in syllables.
fn voice(ms: usize) -> Vec<f32> {
    (0..ms * RATE / 1000)
        .map(|n| {
            let t = n as f32 / RATE as f32;
            let tone: f32 = (1..=5)
                .map(|h| (2.0 * std::f32::consts::PI * 140.0 * h as f32 * t).sin() / h as f32)
                .sum();
            if (t * 5.0).fract() < 0.7 {
                tone * 0.2
            } else {
                tone * 0.04
            }
        })
        .collect()
}

struct Allowed(Result<(), Failure>);

impl Preflight for Allowed {
    fn check(&self, _: Option<&str>) -> Result<(), Failure> {
        self.0.clone()
    }
}

/// A cleanup that adds a mark, or fails.
struct TestCleaner(Result<&'static str, ()>);

impl Cleaner for TestCleaner {
    fn clean(&self, request: &CleanupRequest) -> Result<CleanupResponse, CleanError> {
        match self.0 {
            Ok(text) => Ok(CleanupResponse {
                text: text.to_owned(),
                model: request.model.clone(),
                elapsed_ms: 1,
                load_ms: 0,
                parts: 1,
                tokens_in: 1,
                tokens_out: 1,
            }),
            Err(()) => Err(CleanError::Runtime("the runtime crashed".into())),
        }
    }
}

struct Rig {
    dictation: Dictation,
    fake: Arc<Fake>,
    mic: Arc<Mic>,
    updates: Receiver<Update>,
    _dir: tempfile::TempDir,
}

struct RigOptions {
    catalog: Vec<ModelInfo>,
    cleaner: Result<&'static str, ()>,
    preflight: Result<(), Failure>,
    deepgram_url: Option<String>,
    idle_unload: Duration,
}

impl Default for RigOptions {
    fn default() -> Self {
        RigOptions {
            catalog: mac_catalog(true),
            cleaner: Ok("Cleaned words."),
            preflight: Ok(()),
            deepgram_url: None,
            idle_unload: Duration::from_secs(3600),
        }
    }
}

/// Puts the files of a cleanup model where the check for "ready" looks.
fn install_cleanup(dir: &Path, id: &str) {
    let model = sayso_sdk::cleanup::find(id).unwrap();
    let folder = sayso_sdk::cleanup::download::model_dir(dir, model);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(model.file), b"x").unwrap();
    for file in model.license_files {
        std::fs::write(folder.join(file), b"x").unwrap();
    }
    std::fs::write(
        folder.join("ready.json"),
        format!(
            "{{\"revision\":\"{}\",\"sha256\":\"{}\"}}",
            model.revision, model.sha256
        ),
    )
    .unwrap();
}

fn rig(options: RigOptions) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::default());
    *fake.catalog.lock().unwrap() = options.catalog;
    let mic = Arc::new(Mic::default());
    let cleanup_dir = dir.path().join("cleanup");
    let deps = Deps {
        engine: EngineSource::Custom {
            engine: fake.clone(),
            models: fake.clone(),
        },
        temp_dir: dir.path().join("recordings"),
        cleanup_dir,
        cleaner: Arc::new(TestCleaner(options.cleaner)),
        preload: None,
        unload_cleanup: None,
        preflight: Arc::new(Allowed(options.preflight)),
        microphone: Some(Arc::new(MicHandle(mic.clone()))),
        deepgram_url: options.deepgram_url,
        idle_unload: options.idle_unload,
    };
    let dictation = Dictation::new(deps);
    let updates = dictation.updates();
    Rig {
        dictation,
        fake,
        mic,
        updates,
        _dir: dir,
    }
}

fn local_no_cleanup() -> Settings {
    Settings {
        cleanup_on: false,
        stop_after: StopAfter::Never,
        ..Settings::default()
    }
}

fn request(settings: Settings) -> Request {
    Request {
        settings,
        key: None,
    }
}

impl Rig {
    /// Reads updates until `want` matches one, and returns it.
    fn wait_for(&self, what: &str, want: impl Fn(&Update) -> bool) -> Update {
        let until = Instant::now() + Duration::from_secs(20);
        let mut seen = Vec::new();
        while Instant::now() < until {
            if let Ok(update) = self.updates.recv_timeout(Duration::from_millis(50)) {
                if want(&update) {
                    return update;
                }
                seen.push(update);
            }
        }
        panic!("no {what}; saw {seen:?}");
    }

    fn wait_listening(&self) {
        self.wait_for("listening", |u| *u == Update::Phase(Phase::Listening));
    }

    fn done(&self) -> Update {
        self.wait_for("the end", |u| {
            matches!(u, Update::Done { .. } | Update::Failed(_))
        })
    }
}

// Pure parts.

#[test]
fn the_draft_keeps_its_three_parts_and_adds_spaces_only_where_words_touch() {
    let mut draft = Draft::new("Please build it on bigboi.", " Thanks.");
    draft.set_words("then push the fix");
    assert_eq!(
        draft.whole(),
        "Please build it on bigboi. then push the fix Thanks."
    );
    assert_eq!(&draft.whole()[draft.words_range()], "then push the fix");
    draft.set_words("");
    assert_eq!(draft.whole(), "Please build it on bigboi. Thanks.");

    let mut before_a_word = Draft::new("", "world");
    before_a_word.set_words("hello");
    assert_eq!(before_a_word.whole(), "hello world");
    let mut before_a_mark = Draft::new("Oh", ".");
    before_a_mark.set_words("yes");
    assert_eq!(before_a_mark.whole(), "Oh yes.");
}

#[test]
fn an_edit_outside_the_words_moves_into_the_draft() {
    let mut draft = Draft::new("One ", " three");
    draft.set_words("two");
    assert_eq!(draft.whole(), "One two three");
    assert_eq!(draft.on_edit("Zero One two three"), Edit::InDraft);
    assert_eq!(&draft.whole()[draft.words_range()], "two");
    assert_eq!(draft.on_edit("Zero One two three!"), Edit::InDraft);
    assert_eq!(draft.whole(), "Zero One two three!");
    assert_eq!(&draft.whole()[draft.words_range()], "two");
    draft.set_words("two more");
    assert_eq!(draft.whole(), "Zero One two more three!");
}

#[test]
fn an_edit_of_the_words_takes_them_over() {
    let mut draft = Draft::new("One ", " three");
    draft.set_words("two");
    assert_eq!(draft.on_edit("One tw three"), Edit::TookOver);
    assert_eq!(draft.on_edit("Different"), Edit::TookOver);
}

#[test]
fn final_parts_replace_the_preview_words_that_they_cover() {
    let mut words = LiveWords::default();
    words.on_partial("then push", " the fix");
    assert_eq!(words.text(), "then push the fix");
    words.on_segment("Then push.");
    assert_eq!(words.text(), "Then push. the fix");
    words.on_partial("then push the", " fix to pr");
    assert_eq!(words.text(), "Then push. the fix to pr");
    words.on_segment("The fix to PR 62.");
    words.on_partial("then push the fix to pr", " sixty two and tell");
    assert!(
        words.text().starts_with("Then push. The fix to PR 62."),
        "{}",
        words.text()
    );
    assert!(words.text().ends_with("and tell"), "{}", words.text());
}

#[test]
fn a_final_part_with_other_words_finds_its_end_by_the_last_word() {
    let mut words = LiveWords::default();
    words.on_partial("gonna push the fix now and tell", "");
    // The final pass has fewer words. "fix" ends it.
    words.on_segment("Push the fix.");
    assert_eq!(words.text(), "Push the fix. now and tell");
}

#[test]
fn the_size_reads_as_in_the_design() {
    assert_eq!(format_size(483_000_000), "483 MB");
    assert_eq!(format_size(1_344_000_000), "1.3 GB");
}

#[test]
fn stop_after_has_the_choices_of_the_design_and_30_seconds_by_default() {
    assert_eq!(Settings::default().stop_after, StopAfter::S30);
    assert_eq!(
        StopAfter::ALL.map(StopAfter::limit),
        [
            Some(Duration::from_secs(15)),
            Some(Duration::from_secs(30)),
            Some(Duration::from_secs(60)),
            None
        ]
    );
}

#[test]
fn settings_and_the_key_survive_a_restart_and_the_key_file_is_private() {
    let dir = tempfile::tempdir().unwrap();
    let store = SettingsStore::new(dir.path());
    assert_eq!(store.load(), Settings::default());
    let settings = Settings {
        engine: Engine::Deepgram,
        cleanup_on: false,
        stop_after: StopAfter::Never,
        ..Settings::default()
    };
    store.save(&settings).unwrap();
    assert_eq!(store.load(), settings);

    let keys = KeyStore::new(dir.path());
    assert_eq!(keys.load(), None);
    keys.save(" secret\n").unwrap();
    assert_eq!(keys.load().as_deref(), Some("secret"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(dir.path().join("deepgram.key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let text = std::fs::read_to_string(dir.path().join("dictation.toml")).unwrap();
    assert!(!text.contains("secret"), "the key has its own file");
    keys.remove().unwrap();
    assert_eq!(keys.load(), None);
    keys.remove().unwrap();
}

// Which models.

fn lines(needs: &[ModelNeed]) -> Vec<String> {
    needs.iter().filter_map(ModelNeed::line).collect()
}

#[test]
fn english_on_a_mac_engine_uses_the_small_preview_and_the_design_total() {
    let dir = tempfile::tempdir().unwrap();
    let plan = crate::models::plan_models(&Settings::default(), &mac_catalog(false), dir.path());
    assert_eq!(plan.preview.as_deref(), Some("parakeet-eou-120m"));
    assert_eq!(plan.final_model, "parakeet-tdt-v3");
    assert_eq!(
        lines(&plan.missing()),
        [
            "Parakeet TDT v3, final text",
            "Parakeet EOU 120M, live preview",
            "Cleanup 1 by Spokenly, cleanup"
        ]
    );
    let total: u64 = plan.missing().iter().map(|n| n.size_bytes).sum();
    assert_eq!(format_size(total), "1.3 GB");
}

#[test]
fn another_language_on_a_mac_engine_uses_the_multilingual_preview() {
    let dir = tempfile::tempdir().unwrap();
    let settings = Settings {
        language: "de".into(),
        ..Settings::default()
    };
    let plan = crate::models::plan_models(&settings, &mac_catalog(false), dir.path());
    assert_eq!(plan.preview.as_deref(), Some("nemotron-multilingual"));
}

#[test]
fn a_final_model_with_a_preview_needs_no_second_model() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = vec![model(
        "parakeet-tdt-v3",
        "Parakeet TDT v3",
        483_000_000,
        vec![Role::Final, Role::Streaming],
        false,
    )];
    let plan = crate::models::plan_models(&Settings::default(), &catalog, dir.path());
    assert_eq!(plan.preview.as_deref(), Some("parakeet-tdt-v3"));
    assert_eq!(
        lines(&plan.missing()),
        [
            "Parakeet TDT v3, final text",
            "Cleanup 1 by Spokenly, cleanup"
        ]
    );
    let total: u64 = plan.missing().iter().map(|n| n.size_bytes).sum();
    assert_eq!(format_size(total), "1.1 GB");
}

#[test]
fn deepgram_needs_no_local_final_model_and_keeps_the_preview_local() {
    let dir = tempfile::tempdir().unwrap();
    let settings = Settings {
        engine: Engine::Deepgram,
        cleanup_on: false,
        ..Settings::default()
    };
    let plan = crate::models::plan_models(&settings, &mac_catalog(false), dir.path());
    assert_eq!(plan.final_model, "deepgram-nova-3");
    assert_eq!(plan.preview.as_deref(), Some("parakeet-eou-120m"));
    assert_eq!(lines(&plan.missing()), ["Parakeet EOU 120M, live preview"]);
    assert_eq!(plan.local_final, None, "the final model is not on disk");
}

#[test]
fn s1_mini_always_shows_with_superwhisper() {
    let choices = cleanup_choices();
    assert_eq!(choices[0].1, "Cleanup 1 by Spokenly, 639 MB", "the default");
    assert!(
        choices
            .iter()
            .any(|(_, label)| label == "S1-mini by Superwhisper, 484 MB")
    );
}

// A whole dictation.

#[test]
fn a_dictation_goes_from_starting_to_listening_to_finishing_with_live_words() {
    let rig = rig(RigOptions::default());
    *rig.fake.stream_words.lock().unwrap() = "then push the fix".into();
    rig.dictation.start(request(local_no_cleanup()));
    rig.wait_for("starting", |u| *u == Update::Phase(Phase::Starting));
    rig.wait_listening();
    rig.mic.play(voice(1500));
    rig.wait_for(
        "the words",
        |u| matches!(u, Update::Words(w) if w.contains("then push")),
    );
    rig.dictation.stop();
    rig.wait_for("finishing", |u| *u == Update::Phase(Phase::Finishing));
    let done = rig.done();
    assert!(
        matches!(&done, Update::Done { text, cleanup_failed: false, silence: None } if text.contains("Final words.")),
        "{done:?}"
    );
    rig.wait_for("idle", |u| *u == Update::Phase(Phase::Idle));
    assert!(!rig.dictation.is_active());
}

#[test]
fn a_cancel_ends_at_once_and_sends_nothing_after_it() {
    let rig = rig(RigOptions::default());
    *rig.fake.stream_words.lock().unwrap() = "words".into();
    rig.dictation.start(request(local_no_cleanup()));
    rig.wait_listening();
    rig.mic.play(voice(500));
    rig.dictation.cancel();
    rig.wait_for("cancelled", |u| *u == Update::Cancelled);
    rig.wait_for("idle", |u| *u == Update::Phase(Phase::Idle));
    assert!(!rig.dictation.is_active());
    std::thread::sleep(Duration::from_millis(300));
    let after: Vec<Update> = rig.updates.try_iter().collect();
    assert!(after.is_empty(), "{after:?}");
}

#[test]
fn a_new_dictation_can_follow_a_cancel_while_the_model_still_loads() {
    let rig = rig(RigOptions::default());
    *rig.fake.load_wait.lock().unwrap() = Duration::from_millis(300);
    rig.dictation.start(request(local_no_cleanup()));
    rig.dictation.cancel();
    *rig.fake.load_wait.lock().unwrap() = Duration::ZERO;
    rig.dictation.start(request(local_no_cleanup()));
    rig.wait_listening();
    rig.dictation.stop();
    assert!(matches!(rig.done(), Update::Done { .. }));
}

#[test]
fn a_stop_while_the_models_load_still_ends_the_dictation() {
    let rig = rig(RigOptions::default());
    *rig.fake.load_wait.lock().unwrap() = Duration::from_millis(200);
    rig.dictation.start(request(local_no_cleanup()));
    rig.dictation.stop();
    assert!(matches!(rig.done(), Update::Done { .. }));
}

#[test]
fn no_permission_and_no_microphone_are_failures_with_no_session() {
    for failure in [Failure::NoPermission, Failure::NoMicrophone] {
        let rig = rig(RigOptions {
            preflight: Err(failure.clone()),
            ..RigOptions::default()
        });
        rig.dictation.start(request(local_no_cleanup()));
        assert_eq!(rig.done(), Update::Failed(failure));
        assert!(!rig.dictation.is_active());
    }
}

#[test]
fn missing_models_stop_the_start_and_name_the_models() {
    let rig = rig(RigOptions {
        catalog: mac_catalog(false),
        ..RigOptions::default()
    });
    rig.dictation.start(request(Settings::default()));
    let Update::Failed(Failure::ModelsMissing(needs)) = rig.done() else {
        panic!("the models are missing");
    };
    // The third is the cleanup model. Its download is real, so a test does not
    // start it. The next test covers the downloads.
    assert_eq!(needs.len(), 3);
}

#[test]
fn a_downloaded_set_of_speech_models_is_fetched_in_order_and_can_be_cancelled() {
    let rig = rig(RigOptions {
        catalog: mac_catalog(false),
        ..RigOptions::default()
    });
    let settings = local_no_cleanup();
    let needs = rig.dictation.needs(&settings).unwrap();
    rig.dictation.cancel_download();
    rig.dictation.download(needs);
    rig.wait_for("the end of the downloads", |u| *u == Update::DownloadsDone);
    assert_eq!(
        *rig.fake.downloads.lock().unwrap(),
        ["parakeet-tdt-v3", "parakeet-eou-120m"]
    );
}

#[test]
fn the_silence_limit_ends_like_stop_and_says_so() {
    let rig = rig(RigOptions::default());
    let settings = Settings {
        cleanup_on: false,
        stop_after: StopAfter::S15,
        ..Settings::default()
    };
    rig.dictation.start(request(settings));
    rig.wait_listening();
    rig.mic.play(voice(1000));
    // 15 s of silence reach the limit. The clock is the audio, so this is fast.
    rig.mic.play(vec![0.0; 16 * RATE]);
    let done = rig.done();
    assert!(
        matches!(&done, Update::Done { silence: Some(s), .. } if *s >= Duration::from_secs(15)),
        "{done:?}"
    );
}

fn with_cleanup(rig: &Rig) -> Settings {
    install_cleanup(
        &rig._dir.path().join("cleanup"),
        &Settings::default().cleanup_model,
    );
    Settings {
        stop_after: StopAfter::Never,
        ..Settings::default()
    }
}

#[test]
fn the_cleanup_replaces_the_text_of_the_final_pass() {
    let rig = rig(RigOptions::default());
    let settings = with_cleanup(&rig);
    rig.dictation.start(request(settings));
    rig.wait_listening();
    rig.mic.play(voice(1500));
    rig.dictation.stop();
    assert_eq!(
        rig.done(),
        Update::Done {
            text: "Cleaned words.".into(),
            cleanup_failed: false,
            silence: None
        }
    );
}

#[test]
fn a_failed_cleanup_keeps_the_text_of_the_final_pass() {
    let rig = rig(RigOptions {
        cleaner: Err(()),
        ..RigOptions::default()
    });
    let settings = with_cleanup(&rig);
    rig.dictation.start(request(settings));
    rig.wait_listening();
    rig.mic.play(voice(1500));
    rig.dictation.stop();
    let done = rig.done();
    assert!(
        matches!(&done, Update::Done { text, cleanup_failed: true, .. } if text.contains("Final words.")),
        "{done:?}"
    );
}

#[test]
fn a_language_that_the_cleanup_model_does_not_know_keeps_the_text() {
    let rig = rig(RigOptions::default());
    let settings = Settings {
        language: "it".into(),
        ..with_cleanup(&rig)
    };
    rig.dictation.start(request(settings));
    rig.wait_listening();
    rig.mic.play(voice(1500));
    rig.dictation.stop();
    let done = rig.done();
    assert!(
        matches!(&done, Update::Done { text, cleanup_failed: true, .. } if text.contains("Final words.")),
        "{done:?}"
    );
}

#[test]
fn an_empty_dictation_does_not_run_the_cleanup_and_reports_no_failure() {
    let rig = rig(RigOptions {
        cleaner: Err(()),
        ..RigOptions::default()
    });
    let settings = with_cleanup(&rig);
    rig.dictation.start(request(settings));
    rig.wait_listening();
    rig.dictation.stop();
    assert_eq!(
        rig.done(),
        Update::Done {
            text: String::new(),
            cleanup_failed: false,
            silence: None
        }
    );
}

#[test]
fn the_models_leave_the_memory_after_the_idle_time() {
    let rig = rig(RigOptions {
        idle_unload: Duration::from_millis(100),
        ..RigOptions::default()
    });
    rig.dictation.start(request(local_no_cleanup()));
    rig.wait_listening();
    rig.dictation.stop();
    rig.done();
    let until = Instant::now() + Duration::from_secs(5);
    while rig.fake.unloaded.lock().unwrap().is_empty() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        rig.fake
            .unloaded
            .lock()
            .unwrap()
            .contains(&"parakeet-tdt-v3".to_owned())
    );
}

// Deepgram.

fn deepgram_settings() -> Request {
    Request {
        settings: Settings {
            engine: Engine::Deepgram,
            cleanup_on: false,
            stop_after: StopAfter::Never,
            ..Settings::default()
        },
        key: Some("key".into()),
    }
}

/// An address where nothing answers.
fn dead_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}/v1")
}

#[test]
fn deepgram_without_a_key_does_not_start() {
    let rig = rig(RigOptions::default());
    let mut request = deepgram_settings();
    request.key = None;
    rig.dictation.start(request);
    assert_eq!(rig.done(), Update::Failed(Failure::NoKey));
}

#[test]
fn a_lost_connection_asks_and_finish_on_this_device_uses_the_local_model() {
    let rig = rig(RigOptions {
        deepgram_url: Some(dead_url()),
        ..RigOptions::default()
    });
    rig.dictation.start(deepgram_settings());
    rig.wait_listening();
    rig.mic.play(voice(1500));
    rig.dictation.stop();
    let lost = rig.wait_for("the loss", |u| matches!(u, Update::CloudLost { .. }));
    assert_eq!(
        lost,
        Update::CloudLost {
            loss: CloudLoss::Connection,
            can_finish_on_device: true
        }
    );
    rig.dictation.cloud(CloudChoice::FinishOnDevice);
    let done = rig.done();
    assert!(
        matches!(&done, Update::Done { text, .. } if text.contains("Final words.")),
        "{done:?}"
    );
    assert_eq!(
        *rig.fake.final_models.lock().unwrap(),
        ["parakeet-tdt-v3"],
        "the local model wrote the part"
    );
}

#[test]
fn finish_on_this_device_is_not_offered_without_the_final_model() {
    let mut catalog = mac_catalog(true);
    catalog[0].state = ModelState::NotDownloaded;
    let rig = rig(RigOptions {
        catalog,
        deepgram_url: Some(dead_url()),
        ..RigOptions::default()
    });
    rig.dictation.start(deepgram_settings());
    rig.wait_listening();
    rig.mic.play(voice(1500));
    rig.dictation.stop();
    let lost = rig.wait_for("the loss", |u| matches!(u, Update::CloudLost { .. }));
    assert!(matches!(
        lost,
        Update::CloudLost {
            can_finish_on_device: false,
            ..
        }
    ));
    rig.dictation.cancel();
    rig.wait_for("cancelled", |u| *u == Update::Cancelled);
}

#[test]
fn try_again_asks_again_while_the_connection_stays_lost() {
    let rig = rig(RigOptions {
        deepgram_url: Some(dead_url()),
        ..RigOptions::default()
    });
    rig.dictation.start(deepgram_settings());
    rig.wait_listening();
    rig.mic.play(voice(1500));
    rig.dictation.stop();
    rig.wait_for("the loss", |u| matches!(u, Update::CloudLost { .. }));
    rig.dictation.cloud(CloudChoice::TryAgain);
    rig.wait_for("the loss again", |u| matches!(u, Update::CloudLost { .. }));
    rig.dictation.cancel();
    rig.wait_for("cancelled", |u| *u == Update::Cancelled);
}
