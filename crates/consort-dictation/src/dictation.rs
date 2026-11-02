//! One dictation from the first click to the last word.
//!
//! The SDK calls block, so each dictation has its own thread. The app sends
//! commands through [`Dictation`] and reads [`Update`]s from a channel. A cancel
//! answers at once, and the thread of that dictation then ends with no more
//! updates, however long the call that it waits for takes.

use crate::cloud::{CloudChoice, CloudGate, CloudLayer, CloudLoss};
use crate::models::{ModelNeed, ModelPlan, Purpose, plan_models};
use crate::settings::{Engine, Settings};
use crate::text::LiveWords;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use sayso_sdk::cleanup::{self, Cleaner, CleanupRequest, RuntimeCleaner};
use sayso_sdk::deepgram::Deepgram;
use sayso_sdk::embedded::{
    App, CaptureMicrophone, EngineSource, Event, Microphone, Mode, ModelId, ModelInfo, Options,
    SessionOptions, Source, Speech, SpeechEngine,
};
use sayso_sdk::permission::{self, Permission};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// How long the app waits for the events of the engine in one go. A stop or a
/// cancel takes at most this long to act.
const POLL: Duration = Duration::from_millis(40);
/// One text of a dictation gets this long. The load of the model comes on top.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(8);
/// The models leave the memory after this long with no dictation.
const IDLE_UNLOAD: Duration = Duration::from_secs(10 * 60);
/// The level bars do not need every frame.
const LEVEL_EVERY: Duration = Duration::from_millis(60);
/// The system question about the microphone gets this long.
const PERMISSION_WAIT: Duration = Duration::from_secs(120);
/// A new dictation waits this long for a cancelled one to free its place.
const BUSY_WAIT: Duration = Duration::from_secs(5);
const CLEANUP_ENDPOINT: &str = cleanup::download::DEFAULT_ENDPOINT;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    /// The models load. The audio is kept, so the words come late.
    Starting,
    Listening,
    /// The last parts and the cleanup run. Esc still cancels.
    Finishing,
}

impl Phase {
    pub fn is_active(self) -> bool {
        self != Phase::Idle
    }
}

/// Why a dictation cannot run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    NoPermission,
    NoMicrophone,
    /// These models are not on disk. Dictation starts after the download.
    ModelsMissing(Vec<ModelNeed>),
    /// The engine is Deepgram, and there is no key.
    NoKey,
    /// The engine or the session failed. The text says what happened.
    Engine(String),
}

/// What the app shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    Phase(Phase),
    /// All dictated words so far.
    Words(String),
    /// The sound level, from 0 to 1.
    Level(f32),
    /// Deepgram wrote nothing. The final pass waits for [`Dictation::cloud`].
    CloudLost {
        loss: CloudLoss,
        /// False when the final model is not on disk.
        can_finish_on_device: bool,
    },
    CloudBack,
    /// The end: the text replaces the gray words, and the state is idle.
    Done {
        text: String,
        /// The cleanup did not run, so the text is the final pass.
        cleanup_failed: bool,
        /// The silence limit ended the dictation.
        silence: Option<Duration>,
    },
    Cancelled,
    Failed(Failure),
    DownloadProgress {
        model: String,
        done: u64,
        total: u64,
    },
    DownloadModelDone {
        model: String,
    },
    DownloadsDone,
    DownloadFailed(String),
}

/// One dictation.
#[derive(Clone, Debug)]
pub struct Request {
    pub settings: Settings,
    /// The Deepgram key, when the engine is Deepgram.
    pub key: Option<String>,
}

/// Checks that the microphone can be used. A test gives its own.
pub trait Preflight: Send + Sync {
    fn check(&self, device: Option<&str>) -> Result<(), Failure>;
}

/// The checks of the real system.
pub struct SystemPreflight;

impl Preflight for SystemPreflight {
    fn check(&self, device: Option<&str>) -> Result<(), Failure> {
        match permission::status() {
            Permission::Granted => {}
            Permission::Denied => return Err(Failure::NoPermission),
            Permission::NotDetermined => {
                let (tx, rx) = unbounded();
                permission::request(move |granted| {
                    let _ = tx.send(granted);
                });
                if !rx.recv_timeout(PERMISSION_WAIT).unwrap_or(false) {
                    return Err(Failure::NoPermission);
                }
            }
        }
        let devices = sayso_sdk::embedded::devices();
        if devices.is_empty() {
            return Err(Failure::NoMicrophone);
        }
        // A chosen device that left falls back to the default one.
        let _ = device;
        Ok(())
    }
}

/// Loads a cleanup model ahead.
pub type Preload = Arc<dyn Fn(&str) + Send + Sync>;

/// What a dictation runs on.
pub struct Deps {
    pub engine: EngineSource,
    /// A folder for the temporary recordings of sessions.
    pub temp_dir: PathBuf,
    /// The folder of the cleanup models.
    pub cleanup_dir: PathBuf,
    pub cleaner: Arc<dyn Cleaner>,
    /// Loads a cleanup model ahead, so the fix comes at once.
    pub preload: Option<Preload>,
    /// Takes the cleanup model out of the memory.
    pub unload_cleanup: Option<Arc<dyn Fn() + Send + Sync>>,
    pub preflight: Arc<dyn Preflight>,
    /// None: the capture of `sayso-capture`.
    pub microphone: Option<Arc<dyn Microphone>>,
    /// The address of Deepgram, for tests.
    pub deepgram_url: Option<String>,
    pub idle_unload: Duration,
}

impl Deps {
    /// The real engine, runtime and microphone. `data_dir` is a folder that
    /// Consort owns for the models.
    pub fn real(engine_program: PathBuf, data_dir: PathBuf) -> Deps {
        let models = data_dir.join("speech-models");
        let cleanup_dir = data_dir.join("cleanup-models");
        let runtime = cleanup::Runtime::new(cleanup::Runtime::default_path());
        let cleaner = Arc::new(RuntimeCleaner::new(runtime.clone(), cleanup_dir.clone()));
        let preloader = cleaner.clone();
        Deps {
            engine: EngineSource::program(engine_program, models),
            temp_dir: data_dir.join("recordings"),
            cleanup_dir,
            cleaner,
            preload: Some(Arc::new(move |model| preloader.preload(model))),
            unload_cleanup: Some(Arc::new(move || runtime.stop())),
            preflight: Arc::new(SystemPreflight),
            microphone: None,
            deepgram_url: None,
            idle_unload: IDLE_UNLOAD,
        }
    }
}

/// The flags of one dictation.
#[derive(Default)]
struct Run {
    stop: AtomicBool,
    cancel: AtomicBool,
}

struct Shared {
    deps: Deps,
    capture: Option<Arc<CaptureMicrophone>>,
    microphone: Arc<dyn Microphone>,
    updates: Sender<Update>,
    gate: Arc<CloudGate>,
    speech: Mutex<Option<Arc<Speech>>>,
    /// Held while a dictation runs, so the next one waits for the last call of
    /// a cancelled one.
    busy: Mutex<()>,
    current: Mutex<Option<Arc<Run>>>,
    /// Models that this app loaded.
    loaded: Mutex<Vec<String>>,
    last_use: Mutex<Instant>,
    cancel_download: Arc<AtomicBool>,
    run_id: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Each value stays valid when a thread panics: it is replaced as a whole.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The dictation of this app. It is cheap to make: the engine starts with the
/// first request that needs it.
#[derive(Clone)]
pub struct Dictation {
    shared: Arc<Shared>,
    updates: Receiver<Update>,
}

impl Dictation {
    pub fn new(mut deps: Deps) -> Dictation {
        let (tx, updates) = unbounded();
        let capture = deps
            .microphone
            .is_none()
            .then(|| Arc::new(CaptureMicrophone::default()));
        let microphone: Arc<dyn Microphone> = match (deps.microphone.take(), &capture) {
            (Some(microphone), _) => microphone,
            (None, Some(capture)) => capture.clone(),
            (None, None) => Arc::new(CaptureMicrophone::default()),
        };
        let shared = Arc::new(Shared {
            deps,
            capture,
            microphone,
            updates: tx,
            gate: Arc::new(CloudGate::default()),
            speech: Mutex::new(None),
            busy: Mutex::new(()),
            current: Mutex::new(None),
            loaded: Mutex::new(Vec::new()),
            last_use: Mutex::new(Instant::now()),
            cancel_download: Arc::new(AtomicBool::new(false)),
            run_id: AtomicU64::new(0),
        });
        spawn_idle_watch(Arc::downgrade(&shared));
        Dictation { shared, updates }
    }

    /// The channel of updates.
    pub fn updates(&self) -> Receiver<Update> {
        self.updates.clone()
    }

    /// True from a start until the end of that dictation.
    pub fn is_active(&self) -> bool {
        lock(&self.shared.current).is_some()
    }

    /// Starts a dictation. It does nothing while one runs.
    pub fn start(&self, request: Request) {
        let run = Arc::new(Run::default());
        {
            let mut current = lock(&self.shared.current);
            if current.is_some() {
                return;
            }
            *current = Some(run.clone());
        }
        self.shared.run_id.fetch_add(1, Ordering::SeqCst);
        let _ = self.shared.updates.send(Update::Phase(Phase::Starting));
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new()
            .name("dictation".into())
            .spawn(move || shared.run(&run, &request));
        if let Err(error) = spawned {
            *lock(&self.shared.current) = None;
            let failure = Failure::Engine(format!("Cannot start dictation: {error}"));
            let _ = self.shared.updates.send(Update::Failed(failure));
            let _ = self.shared.updates.send(Update::Phase(Phase::Idle));
        }
    }

    /// Ends the audio. The last words and the cleanup still come.
    pub fn stop(&self) {
        if let Some(run) = lock(&self.shared.current).as_ref() {
            run.stop.store(true, Ordering::SeqCst);
        }
    }

    /// Forgets this dictation. The app gets [`Update::Cancelled`] now, and no
    /// other update of this dictation after it.
    pub fn cancel(&self) {
        let Some(run) = lock(&self.shared.current).take() else {
            return;
        };
        run.cancel.store(true, Ordering::SeqCst);
        self.shared.gate.answer(CloudChoice::Cancel);
        if let Some(speech) = lock(&self.shared.speech).as_ref() {
            speech.cancel_session();
        }
        let _ = self.shared.updates.send(Update::Cancelled);
        let _ = self.shared.updates.send(Update::Phase(Phase::Idle));
    }

    /// The answer to [`Update::CloudLost`].
    pub fn cloud(&self, choice: CloudChoice) {
        self.shared.gate.answer(choice);
    }

    /// The catalog of the engine. Blocks. It starts the engine.
    pub fn catalog(&self) -> Result<Vec<ModelInfo>, String> {
        let speech = self.shared.speech().map_err(|e| e.to_string())?;
        speech.models().map_err(|e| e.to_string())
    }

    /// The models for these settings. Blocks.
    pub fn needs(&self, settings: &Settings) -> Result<Vec<ModelNeed>, String> {
        let catalog = self.catalog()?;
        Ok(plan_models(settings, &catalog, &self.shared.deps.cleanup_dir).needs)
    }

    /// Downloads models, one after the other, on its own thread.
    pub fn download(&self, needs: Vec<ModelNeed>) {
        self.shared.cancel_download.store(false, Ordering::SeqCst);
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new()
            .name("dictation-download".into())
            .spawn(move || shared.download(&needs));
        if let Err(error) = spawned {
            let _ = self
                .shared
                .updates
                .send(Update::DownloadFailed(error.to_string()));
        }
    }

    /// Stops the download between models. A cleanup file stops at once and
    /// keeps its partial file. An engine finishes the file that it fetches,
    /// because its protocol has no cancel.
    pub fn cancel_download(&self) {
        self.shared.cancel_download.store(true, Ordering::SeqCst);
    }

    /// Deletes the models of a plan from this computer. Blocks.
    pub fn remove(&self, needs: &[ModelNeed]) -> Result<(), String> {
        let speech = self.shared.speech().map_err(|e| e.to_string())?;
        for need in needs {
            if need.purpose == Purpose::Cleanup {
                if let Some(model) = cleanup::find(&need.id) {
                    cleanup::download::delete(&self.shared.deps.cleanup_dir, model)
                        .map_err(|e| e.to_string())?;
                }
            } else {
                speech.delete(&need.id).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// Asks Deepgram whether it takes a key. Blocks.
    pub fn check_key(&self, key: &str) -> Result<(), String> {
        let client = match &self.shared.deps.deepgram_url {
            Some(url) => Deepgram::with_base_url(key, url),
            None => Deepgram::new(key),
        };
        client.check_key().map_err(|e| e.message)
    }
}

impl Shared {
    fn speech(self: &Arc<Self>) -> Result<Arc<Speech>, sayso_sdk::embedded::Error> {
        let mut slot = lock(&self.speech);
        if let Some(speech) = slot.as_ref() {
            return Ok(speech.clone());
        }
        let options = Options {
            mode: Mode::Local,
            app: App::new("Consort"),
            engine: self.deps.engine.clone(),
            temp_dir: self.deps.temp_dir.clone(),
            microphone: Some(self.microphone.clone()),
        };
        let gate = self.gate.clone();
        let (lost, back) = (self.clone(), self.clone());
        let layer = move |local: Arc<dyn SpeechEngine>| -> Arc<dyn SpeechEngine> {
            let weak_lost = Arc::downgrade(&lost);
            let weak_back = Arc::downgrade(&back);
            Arc::new(CloudLayer::new(
                local,
                gate,
                Arc::new(move |loss| {
                    if let Some(shared) = weak_lost.upgrade() {
                        shared.on_cloud_loss(loss);
                    }
                }),
                Arc::new(move || {
                    if let Some(shared) = weak_back.upgrade() {
                        let _ = shared.updates.send(Update::CloudBack);
                    }
                }),
            ))
        };
        let speech = Arc::new(Speech::connect_with(options, layer)?);
        *slot = Some(speech.clone());
        Ok(speech)
    }

    fn on_cloud_loss(&self, loss: CloudLoss) {
        let can_finish_on_device = self.gate.has_local_final();
        let _ = self.updates.send(Update::CloudLost {
            loss,
            can_finish_on_device,
        });
    }

    /// Sends an update of `run`, unless the app forgot that run.
    fn emit(&self, run: &Run, update: Update) {
        if !run.cancel.load(Ordering::SeqCst) {
            let _ = self.updates.send(update);
        }
    }

    /// The end of a run that the app did not cancel.
    fn finish(&self, run: &Run, update: Update) {
        let mut current = lock(&self.current);
        if run.cancel.load(Ordering::SeqCst) {
            return;
        }
        *current = None;
        let _ = self.updates.send(update);
        let _ = self.updates.send(Update::Phase(Phase::Idle));
    }

    fn run(self: &Arc<Self>, run: &Arc<Run>, request: &Request) {
        let _busy = lock(&self.busy);
        *lock(&self.last_use) = Instant::now();
        let result = self.listen(run, request);
        if let Some(capture) = &self.capture {
            capture.close_early();
            capture.set_level(None);
        }
        *lock(&self.last_use) = Instant::now();
        match result {
            Ok(Some(done)) => self.finish(run, done),
            Ok(None) => {}
            Err(failure) => self.finish(run, Update::Failed(failure)),
        }
    }

    /// Runs the session and the cleanup. None: the app cancelled it.
    fn listen(
        self: &Arc<Self>,
        run: &Arc<Run>,
        request: &Request,
    ) -> Result<Option<Update>, Failure> {
        let settings = &request.settings;
        let deepgram_on = settings.engine == Engine::Deepgram;
        let key = request.key.clone().filter(|_| deepgram_on);
        if deepgram_on && key.is_none() {
            return Err(Failure::NoKey);
        }
        self.deps.preflight.check(settings.microphone.as_deref())?;
        let speech = self.speech().map_err(engine_failure)?;
        let catalog = speech.models().map_err(engine_failure)?;
        let plan = plan_models(settings, &catalog, &self.deps.cleanup_dir);
        let missing = plan.missing();
        if !missing.is_empty() {
            return Err(Failure::ModelsMissing(missing));
        }
        self.open_microphone(run, settings)?;
        self.gate
            .begin(self.client_key(key), plan.local_final.clone());
        if settings.cleanup_on
            && let Some(preload) = &self.deps.preload
        {
            preload(&settings.cleanup_model);
        }
        let session = self.start_session(&speech, &plan, settings)?;
        self.note_loaded(&plan);
        if run.cancel.load(Ordering::SeqCst) {
            speech.cancel_session();
            return Ok(None);
        }
        self.emit(run, Update::Phase(Phase::Listening));

        let events = speech.events();
        let mut words = LiveWords::default();
        let mut silence = None;
        // Listen until a stop.
        while !run.stop.load(Ordering::SeqCst) {
            if run.cancel.load(Ordering::SeqCst) {
                return Ok(None);
            }
            self.next_event(run, &events, session, &mut words, &mut silence);
            if silence.is_some() {
                break;
            }
        }
        self.emit(run, Update::Phase(Phase::Finishing));

        // The stop waits for the last parts, and their words still show.
        let (stopped_tx, stopped) = unbounded();
        let stopper = speech.clone();
        std::thread::spawn(move || {
            let _ = stopped_tx.send(stopper.stop_session(session));
        });
        let result = loop {
            if run.cancel.load(Ordering::SeqCst) {
                return Ok(None);
            }
            match stopped.try_recv() {
                Ok(result) => break result,
                Err(_) => self.next_event(run, &events, session, &mut words, &mut silence),
            }
        };
        while let Ok(event) = events.try_recv() {
            apply_event(event, session, &mut words, &mut silence);
        }
        let heard = match result {
            // A part with no final text keeps its words from the preview.
            Ok(text) if !text.trim().is_empty() => text.trim().to_owned(),
            Ok(_) => words.text(),
            Err(error) => {
                tracing::warn!("the end of a dictation failed: {error}");
                words.text()
            }
        };
        let (text, cleanup_failed) = self.clean(run, settings, heard)?;
        let silence = silence.filter(|_| !run.cancel.load(Ordering::SeqCst));
        Ok(Some(Update::Done {
            text,
            cleanup_failed,
            silence,
        }))
    }

    fn client_key(&self, key: Option<String>) -> Option<(String, Option<String>)> {
        key.map(|key| (key, self.deps.deepgram_url.clone()))
    }

    fn open_microphone(&self, run: &Arc<Run>, settings: &Settings) -> Result<(), Failure> {
        let Some(capture) = &self.capture else {
            return Ok(());
        };
        capture.set_device(settings.microphone.clone());
        let updates = self.updates.clone();
        let run = run.clone();
        let last = Mutex::new(Instant::now());
        capture.set_level(Some(Arc::new(move |level| {
            let mut last = lock(&last);
            if last.elapsed() >= LEVEL_EVERY && !run.cancel.load(Ordering::SeqCst) {
                *last = Instant::now();
                let _ = updates.send(Update::Level(level));
            }
        })));
        // The audio starts before the models load, so the first words are kept.
        capture.open_early().map_err(|error| {
            tracing::warn!("cannot open the microphone: {error}");
            Failure::NoMicrophone
        })
    }

    fn start_session(
        &self,
        speech: &Speech,
        plan: &ModelPlan,
        settings: &Settings,
    ) -> Result<u64, Failure> {
        let options = SessionOptions {
            sources: vec![Source::Mic],
            final_model: ModelId::new(&plan.final_model),
            preview_model: plan.preview.as_deref().map(ModelId::new),
            vad: plan.vad.clone(),
            language: settings.language.clone(),
            silence_limit: settings.stop_after.limit(),
        };
        // A cancelled session frees its place soon after the cancel.
        let until = Instant::now() + BUSY_WAIT;
        loop {
            match speech.start_session(options.clone()) {
                Err(error)
                    if Instant::now() < until
                        && error.code().is_some_and(|c| c.as_str() == "busy") =>
                {
                    std::thread::sleep(POLL);
                }
                result => return result.map_err(engine_failure),
            }
        }
    }

    fn note_loaded(&self, plan: &ModelPlan) {
        let mut loaded = lock(&self.loaded);
        for need in plan.needs.iter().filter(|n| n.purpose != Purpose::Cleanup) {
            if !loaded.contains(&need.id) {
                loaded.push(need.id.clone());
            }
        }
    }

    fn next_event(
        &self,
        run: &Run,
        events: &Receiver<Event>,
        session: u64,
        words: &mut LiveWords,
        silence: &mut Option<Duration>,
    ) {
        match events.recv_timeout(POLL) {
            Ok(event) => {
                let before = words.text();
                apply_event(event, session, words, silence);
                // Take all that waits, so a slow app does not lag behind.
                while let Ok(event) = events.try_recv() {
                    apply_event(event, session, words, silence);
                }
                let after = words.text();
                if after != before {
                    self.emit(run, Update::Words(after));
                }
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
        }
    }

    /// Decision 6: a failed cleanup never loses text.
    fn clean(
        &self,
        run: &Run,
        settings: &Settings,
        text: String,
    ) -> Result<(String, bool), Failure> {
        if !settings.cleanup_on || text.trim().is_empty() {
            return Ok((text, false));
        }
        let Some(model) = cleanup::find(&settings.cleanup_model) else {
            return Ok((text, true));
        };
        let languages: Vec<&str> = model.languages.to_vec();
        let language = cleanup::dictation_language(
            &settings.language,
            &languages,
            &text,
            cleanup::detect_language,
        );
        if !language.is_some_and(|code| model.supports_language(&code)) {
            return Ok((text, true));
        }
        let cleaner = self.deps.cleaner.clone();
        let cleanup = CleanupRequest {
            model: settings.cleanup_model.clone(),
            text: text.clone(),
            settings: cleanup::Settings::default(),
            app: Some("Consort".to_owned()),
            site: None,
            timeout: CLEANUP_TIMEOUT,
        };
        let (done_tx, done) = unbounded();
        std::thread::spawn(move || {
            let _ = done_tx.send(cleaner.clean(&cleanup));
        });
        loop {
            if run.cancel.load(Ordering::SeqCst) {
                return Ok((text, false));
            }
            match done.recv_timeout(POLL) {
                Ok(Ok(answer)) if !answer.text.trim().is_empty() => {
                    return Ok((answer.text, false));
                }
                Ok(Ok(_)) => return Ok((text, true)),
                Ok(Err(error)) => {
                    tracing::warn!("cleanup failed, the text stays as heard: {error}");
                    return Ok((text, true));
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Ok((text, true)),
            }
        }
    }

    fn download(self: &Arc<Self>, needs: &[ModelNeed]) {
        let speech = match self.speech() {
            Ok(speech) => speech,
            Err(error) => {
                let _ = self.updates.send(Update::DownloadFailed(error.to_string()));
                return;
            }
        };
        for need in needs {
            if self.cancel_download.load(Ordering::SeqCst) {
                return;
            }
            let id = need.id.clone();
            let result = if need.purpose == Purpose::Cleanup {
                self.download_cleanup(need)
            } else {
                speech
                    .download(&need.id, Some(need.size_bytes), |progress| {
                        let _ = self.updates.send(Update::DownloadProgress {
                            model: progress.model.clone(),
                            done: progress.bytes_done,
                            total: progress.bytes_total,
                        });
                    })
                    .map_err(|e| e.to_string())
            };
            if self.cancel_download.load(Ordering::SeqCst) {
                return;
            }
            match result {
                Ok(()) => {
                    let _ = self.updates.send(Update::DownloadModelDone { model: id });
                }
                Err(error) => {
                    let _ = self.updates.send(Update::DownloadFailed(error));
                    return;
                }
            }
        }
        let _ = self.updates.send(Update::DownloadsDone);
    }

    fn download_cleanup(&self, need: &ModelNeed) -> Result<(), String> {
        let model = cleanup::find(&need.id)
            .ok_or_else(|| format!("There is no cleanup model {}.", need.id))?;
        let mut progress = |p: cleanup::download::Progress| {
            let _ = self.updates.send(Update::DownloadProgress {
                model: need.id.clone(),
                done: p.bytes_done,
                total: p.bytes_total,
            });
        };
        cleanup::download::download(
            &self.deps.cleanup_dir,
            model,
            CLEANUP_ENDPOINT,
            &self.cancel_download,
            &mut progress,
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    fn unload_if_idle(&self) {
        if lock(&self.current).is_some() || lock(&self.last_use).elapsed() < self.deps.idle_unload {
            return;
        }
        let models: Vec<String> = std::mem::take(&mut *lock(&self.loaded));
        if models.is_empty() {
            return;
        }
        if let Some(speech) = lock(&self.speech).as_ref() {
            for model in &models {
                if let Err(error) = speech.unload(model) {
                    tracing::debug!("cannot unload {model}: {error}");
                }
            }
        }
        if let Some(unload) = &self.deps.unload_cleanup {
            unload();
        }
    }
}

fn apply_event(event: Event, session: u64, words: &mut LiveWords, silence: &mut Option<Duration>) {
    match event {
        Event::Partial(partial) if partial.session == session => {
            words.on_partial(&partial.committed, &partial.tentative);
        }
        Event::Segment(segment) if segment.session == Some(session) => {
            words.on_segment(&segment.segment.text);
        }
        Event::SilenceLimit {
            session: from,
            silence_ms,
        } if from == session => *silence = Some(Duration::from_millis(silence_ms)),
        _ => {}
    }
}

fn engine_failure(error: sayso_sdk::embedded::Error) -> Failure {
    tracing::warn!("dictation failed: {error}");
    Failure::Engine(error.to_string())
}

/// Takes the models out of the memory after a time with no dictation.
fn spawn_idle_watch(shared: std::sync::Weak<Shared>) {
    let tick = shared
        .upgrade()
        .map_or(POLL, |s| (s.deps.idle_unload / 4).max(POLL));
    let _ = std::thread::Builder::new()
        .name("dictation-idle".into())
        .spawn(move || {
            loop {
                std::thread::sleep(tick);
                let Some(shared) = shared.upgrade() else {
                    return;
                };
                shared.unload_if_idle();
            }
        });
}
