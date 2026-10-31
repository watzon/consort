//! Live dictation in a message box: the view of `consort-dictation`.
//!
//! The crate reports phases and words. This module writes the words into the
//! message box, gray until the dictation ends. The undo history stays clean: a
//! new partial undoes the last one and then inserts the new text, so the whole
//! dictation is one undo step (docs/DICTATION-PLAN.md, step 3).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use consort_dictation::{
    CloudChoice, CloudLoss, Deps, Dictation, Draft, Edit, Engine, Failure, KeyStore, ModelNeed,
    Phase, Request, Settings, SettingsStore, Update, format_size,
};
use gpui_kit::component::input::{EditorState, Undo};
use gpui_kit::{
    AsyncWindowContext, Context, Entity, Focusable as _, HighlightStyle, KeyDownEvent, KeyUpEvent,
    WeakEntity, Window,
};

use crate::app::{ConsortApp, Dialog, SettingsSection};
use crate::notice::INFO_TIMEOUT;
use crate::ui::menu::TextTarget;

/// A press of the shortcut that lasts longer than this is push to talk.
const HOLD: Duration = Duration::from_millis(300);

/// Tap or hold (plan, decision 11). A tap starts and stops. A hold of more than
/// about 300 ms is push to talk, so its release stops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShortcutPress {
    /// This press started a dictation, at this time.
    started_at: Option<Instant>,
}

/// What a key event of the shortcut asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutAction {
    Start,
    Stop,
    Nothing,
}

impl ShortcutPress {
    /// A press of the shortcut. `repeat`: the key repeats while it is held.
    pub fn key_down(&mut self, active: bool, repeat: bool, now: Instant) -> ShortcutAction {
        if repeat {
            return ShortcutAction::Nothing;
        }
        if active {
            self.started_at = None;
            ShortcutAction::Stop
        } else {
            self.started_at = Some(now);
            ShortcutAction::Start
        }
    }

    pub fn key_up(&mut self, active: bool, now: Instant) -> ShortcutAction {
        match self.started_at.take() {
            Some(at) if active && now.duration_since(at) > HOLD => ShortcutAction::Stop,
            _ => ShortcutAction::Nothing,
        }
    }
}

/// One line of the download notice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadRow {
    pub need: ModelNeed,
    pub done: u64,
    pub total: u64,
    pub finished: bool,
}

/// The notices of the dictation (design V4). They have more than one line and
/// their own buttons, so they are not in `Notices`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DictationNotice {
    NoPermission,
    NoMicrophone,
    NoKey,
    CleanupFailed,
    ModelsMissing(Vec<ModelNeed>),
    Downloading(Vec<DownloadRow>),
    CloudLost {
        loss: CloudLoss,
        can_finish_on_device: bool,
    },
    SendHeld,
}

/// A button of a dictation notice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoticeButton {
    OpenSystemSettings,
    OpenDictationSettings,
    Download,
    CancelDownload,
    TryAgain,
    FinishOnDevice,
    SendNow,
}

impl DictationNotice {
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            Self::NoPermission | Self::NoMicrophone | Self::NoKey | Self::CloudLost { .. }
        )
    }

    pub fn title(&self) -> String {
        match self {
            Self::NoPermission => "Microphone access is off".into(),
            Self::NoMicrophone => "Consort found no microphone".into(),
            Self::NoKey => "Deepgram needs a key".into(),
            Self::CleanupFailed => "Cleanup did not run".into(),
            Self::ModelsMissing(needs) => {
                format!(
                    "Dictation needs {} speech {}",
                    count_word(shown_rows(needs)),
                    if shown_rows(needs) == 1 {
                        "model"
                    } else {
                        "models"
                    }
                )
            }
            Self::Downloading(_) => "Downloading the speech models".into(),
            Self::CloudLost {
                loss: CloudLoss::Connection,
                ..
            } => "Lost the connection to Deepgram".into(),
            Self::CloudLost {
                loss: CloudLoss::KeyRefused,
                ..
            } => "Deepgram refused the key".into(),
            Self::SendHeld => "Dictation is still finishing".into(),
        }
    }

    pub fn text(&self) -> String {
        match self {
            Self::NoPermission => "Consort cannot hear you until you allow it. Turn on the microphone for Consort in System Settings, then press the mic again.".into(),
            Self::NoMicrophone => "Connect a microphone, or choose another one in Settings > Dictation. Then press the mic again.".into(),
            Self::NoKey => "Add your key in Settings > Dictation, or choose On this device. Then press the mic again.".into(),
            Self::CleanupFailed => "Consort kept your words as heard. You do not need to do anything.".into(),
            Self::ModelsMissing(needs) => {
                let total: u64 = needs.iter().map(|n| n.size_bytes).sum();
                format!(
                    "They run on this device, so your voice stays here. You download them once. Total {}.",
                    format_size(total)
                )
            }
            Self::Downloading(rows) => {
                let shown = rows.iter().filter(|r| r.need.line().is_some()).count();
                if shown == 1 {
                    "Dictation starts when it is done.".into()
                } else if shown == 2 {
                    "Dictation starts when both are done.".into()
                } else {
                    format!("Dictation starts when all {} are done.", count_word(shown))
                }
            }
            Self::CloudLost { loss: CloudLoss::Connection, can_finish_on_device: true } => "Deepgram writes the final text at the end of each phrase. Your words stay as this computer heard them. Try again, or finish on this device.".into(),
            Self::CloudLost { loss: CloudLoss::Connection, can_finish_on_device: false } => "Deepgram writes the final text at the end of each phrase. Your words stay as this computer heard them. Try again.".into(),
            Self::CloudLost { loss: CloudLoss::KeyRefused, can_finish_on_device: true } => "Your words stay as this computer heard them. Check the key in Settings > Dictation and try again, or finish on this device.".into(),
            Self::CloudLost { loss: CloudLoss::KeyRefused, can_finish_on_device: false } => "Your words stay as this computer heard them. Check the key in Settings > Dictation, then try again.".into(),
            Self::SendHeld => "Consort did not send your message. You can send again once it is done, or send now without the words still to come.".into(),
        }
    }

    pub fn buttons(&self) -> Vec<(NoticeButton, String)> {
        match self {
            Self::NoPermission => vec![(
                NoticeButton::OpenSystemSettings,
                "Open System Settings".into(),
            )],
            Self::NoMicrophone | Self::NoKey => vec![(
                NoticeButton::OpenDictationSettings,
                "Open Dictation settings".into(),
            )],
            Self::CleanupFailed => Vec::new(),
            Self::ModelsMissing(needs) => {
                let total: u64 = needs.iter().map(|n| n.size_bytes).sum();
                vec![
                    (
                        NoticeButton::Download,
                        format!("Download {}", format_size(total)),
                    ),
                    (
                        NoticeButton::OpenDictationSettings,
                        "Change engine".to_owned(),
                    ),
                ]
            }
            Self::Downloading(_) => vec![(NoticeButton::CancelDownload, "Cancel download".into())],
            Self::CloudLost {
                can_finish_on_device,
                ..
            } => {
                let mut buttons = vec![(NoticeButton::TryAgain, "Try again".to_owned())];
                if *can_finish_on_device {
                    buttons.push((
                        NoticeButton::FinishOnDevice,
                        "Finish on this device".to_owned(),
                    ));
                }
                buttons
            }
            Self::SendHeld => vec![(NoticeButton::SendNow, "Send now".into())],
        }
    }
}

fn shown_rows(needs: &[ModelNeed]) -> usize {
    needs.iter().filter(|n| n.line().is_some()).count()
}

fn count_word(count: usize) -> String {
    match count {
        1 => "one".into(),
        2 => "two".into(),
        3 => "three".into(),
        4 => "four".into(),
        other => other.to_string(),
    }
}

/// The dictation state of the app.
#[derive(Default)]
pub struct DictationUi {
    engine: Option<Dictation>,
    /// Tests run with no engine and send updates themselves.
    pub engine_off: bool,
    pub settings: Settings,
    store: Option<SettingsStore>,
    keys: Option<KeyStore>,
    pub has_key: bool,
    pub phase: Phase,
    pub target: Option<TextTarget>,
    pub level: f32,
    draft: Draft,
    /// What the box should hold. Any other text is an edit of the user.
    expected: String,
    /// The box's undo stack has the last partial on top.
    live_step: bool,
    decoration: Option<gpui_kit::base::input::TextDecorationCollection>,
    /// The mic of this box has the failed dot.
    pub failed_dot: Option<TextTarget>,
    pub notice: Option<DictationNotice>,
    press: ShortcutPress,
    /// The next key press becomes the shortcut.
    pub capturing_shortcut: bool,
    /// Start after the downloads, in this box.
    resume: Option<TextTarget>,
    /// The speech models of these settings, for the settings page.
    pub needs: Vec<ModelNeed>,
    pub devices: Vec<consort_dictation::Device>,
    pub languages: Vec<String>,
    page_wants_models: bool,
    /// Ended dictations whose last report did not come yet. Until it comes,
    /// reports in the queue belong to the old dictation, not to a new one.
    pub(crate) reports_to_skip: u32,
}

impl DictationUi {
    /// `engine_off`: no engine, and no file of this computer is read.
    pub fn new(engine_off: bool) -> DictationUi {
        let mut this = DictationUi {
            engine_off,
            ..DictationUi::default()
        };
        if engine_off {
            return this;
        }
        // The settings and the key of this computer. They never go to the hub.
        if let Some(store) = consort_client::store::Store::default_location() {
            let settings = SettingsStore::new(store.dir());
            let keys = KeyStore::new(store.dir());
            this.settings = settings.load();
            this.has_key = keys.load().is_some();
            this.store = Some(settings);
            this.keys = Some(keys);
        }
        this
    }
}

impl ConsortApp {
    /// Saves a change of Settings > Dictation.
    pub(crate) fn change_dictation_settings(
        &mut self,
        change: impl FnOnce(&mut Settings),
        cx: &mut Context<Self>,
    ) {
        change(&mut self.dictation.settings);
        if let Some(store) = &self.dictation.store
            && let Err(error) = store.save(&self.dictation.settings)
        {
            self.show_error(
                format!("Consort could not save the dictation settings. {error}"),
                cx,
            );
        }
        self.refresh_dictation_models(cx);
        cx.notify();
    }

    fn dictation_input(&self, target: TextTarget) -> Entity<EditorState> {
        match target {
            TextTarget::Composer => self.inputs.composer.clone(),
            TextTarget::Reply => self.inputs.reply.clone(),
        }
    }

    /// The page asks the engine what is on disk. It needs a window for the
    /// reports, so the first render of the page starts the engine.
    pub(crate) fn dictation_page_rendered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.dictation.page_wants_models) {
            self.dictation_engine(window, cx);
            self.refresh_dictation_models(cx);
        }
    }

    /// The engine of this app. It starts with the first dictation.
    fn dictation_engine(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Dictation> {
        if self.dictation.engine_off {
            return None;
        }
        if self.dictation.engine.is_none() {
            let deps = Deps::real(engine_program(), data_dir());
            let engine = Dictation::new(deps);
            spawn_update_pump(engine.updates(), window, cx);
            self.dictation.engine = Some(engine);
        }
        self.dictation.engine.clone()
    }

    /// A click on the mic, or a tap of the shortcut.
    pub fn toggle_dictation(
        &mut self,
        target: TextTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.dictation.phase {
            Phase::Idle => self.start_dictation(target, window, cx),
            Phase::Starting | Phase::Listening => self.stop_dictation(),
            Phase::Finishing => {}
        }
    }

    pub fn start_dictation(
        &mut self,
        target: TextTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dictation.phase.is_active() {
            return;
        }
        // The mic is dim until the models are ready.
        if matches!(self.dictation.notice, Some(DictationNotice::Downloading(_))) {
            return;
        }
        let input = self.dictation_input(target);
        input.update(cx, |input, cx| input.focus(window, cx));
        let (value, cursor) = input.read_with(cx, |input, _| {
            (input.value().to_string(), input.selected_range().end)
        });
        let mut draft = Draft::new(&value[..cursor], &value[cursor..]);
        draft.set_words("");
        let d = &mut self.dictation;
        d.draft = draft;
        d.expected = value;
        d.live_step = false;
        d.target = Some(target);
        d.failed_dot = None;
        d.notice = None;
        d.level = 0.0;
        d.phase = Phase::Starting;
        input.update(cx, |input, cx| {
            input.set_placeholder("Listening", window, cx);
        });
        let request = Request {
            settings: self.dictation.settings.clone(),
            key: self.dictation.keys.as_ref().and_then(KeyStore::load),
        };
        if let Some(engine) = self.dictation_engine(window, cx) {
            engine.start(request);
        }
        cx.notify();
    }

    pub fn stop_dictation(&mut self) {
        if let Some(engine) = &self.dictation.engine {
            engine.stop();
        }
    }

    /// Esc: only the dictated words leave, and the caret goes back.
    pub fn cancel_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.remove_dictated_words(window, cx);
        self.end_dictation_session(window, cx);
    }

    /// Ends the engine's dictation with the text as it is now.
    fn end_dictation_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(engine) = &self.dictation.engine
            && engine.is_active()
        {
            engine.cancel();
            self.dictation.reports_to_skip += 1;
        }
        self.dictation_is_idle(window, cx);
    }

    fn dictation_is_idle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_dictation_color(cx);
        if let Some(target) = self.dictation.target.take() {
            let placeholder = match target {
                TextTarget::Composer => self.composer_placeholder(),
                TextTarget::Reply => "Reply to the task".to_owned(),
            };
            self.dictation_input(target).update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx);
            });
        }
        let d = &mut self.dictation;
        d.phase = Phase::Idle;
        d.level = 0.0;
        d.live_step = false;
        if d.notice == Some(DictationNotice::SendHeld)
            || matches!(d.notice, Some(DictationNotice::CloudLost { .. }))
        {
            d.notice = None;
        }
        cx.notify();
    }

    fn remove_dictated_words(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_user_edit(window, cx) {
            self.write_words("", window, cx);
        }
    }

    /// Writes the words into the box. The box holds the draft on both sides.
    fn write_words(&mut self, words: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.dictation.target else {
            return;
        };
        let input = self.dictation_input(target);
        let at = self.dictation.draft.insert_at();
        let old_len = self.dictation.draft.insert().len();
        let whole_before = self.dictation.draft.whole();
        let words_end_before = self.dictation.draft.words_range().end;
        self.dictation.draft.set_words(words);
        let insert = self.dictation.draft.insert();
        let mut base = whole_before.clone();
        base.replace_range(at..at + old_len, "");

        // The caret stays where the user left it, if the user moved it.
        let caret = input.read_with(cx, |input, _| input.selected_range());
        let words_end = words_end_before;
        let user_moved = caret.start != caret.end || caret.end != words_end;

        // Undo the last partial, so all partials are one undo step. Undo needs
        // the box on screen. When it is not, the text does not change.
        let mut undone = false;
        if self.dictation.live_step && old_len > 0 {
            let handle = input.read(cx).focus_handle(cx);
            handle.dispatch_action(&Undo, window, cx);
            undone = input.read_with(cx, |input, _| input.value() == base.as_str());
        }
        input.update(cx, |input, cx| {
            if undone || old_len == 0 {
                input.set_selected_range(at..at, cx);
                if !insert.is_empty() {
                    input.insert(insert.clone(), window, cx);
                }
            } else {
                input.set_selected_range(at..at + old_len, cx);
                input.replace(insert.clone(), window, cx);
            }
        });
        self.dictation.live_step = !insert.is_empty();
        self.dictation.expected = self.dictation.draft.whole();
        let range = self.dictation.draft.words_range();
        let shift = insert.len() as isize - old_len as isize;
        input.update(cx, |input, cx| {
            let place = if user_moved {
                let moved = |at_: usize| {
                    if at_ >= words_end {
                        at_.saturating_add_signed(shift)
                    } else {
                        at_
                    }
                };
                moved(caret.start)..moved(caret.end)
            } else {
                range.end..range.end
            };
            input.set_selected_range(place, cx);
        });
    }

    /// Grays the dictated words. A decoration does not grow at its edges, so
    /// it is set again after each change.
    fn color_dictated_words(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.dictation.target else {
            return;
        };
        let color = Self::tokens(cx).colors.text_tertiary;
        let range = self.dictation.draft.words_range();
        let decorations = if range.is_empty() {
            Vec::new()
        } else {
            vec![gpui_kit::base::input::TextDecoration::new(
                range,
                HighlightStyle {
                    color: Some(color),
                    ..HighlightStyle::default()
                },
            )]
        };
        match &self.dictation.decoration {
            Some(collection) => collection.set(decorations, cx),
            None => {
                let collection = self.dictation_input(target).update(cx, |input, cx| {
                    input.create_decorations_collection(decorations, cx)
                });
                self.dictation.decoration = Some(collection);
            }
        }
    }

    fn clear_dictation_color(&mut self, cx: &mut Context<Self>) {
        if let Some(collection) = self.dictation.decoration.take() {
            collection.set(Vec::new(), cx);
        }
    }

    /// One report of the dictation.
    pub fn on_dictation_update(
        &mut self,
        update: Update,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dictation.reports_to_skip > 0
            && matches!(
                update,
                Update::Phase(_)
                    | Update::Words(_)
                    | Update::Level(_)
                    | Update::CloudLost { .. }
                    | Update::CloudBack
                    | Update::Done { .. }
                    | Update::Cancelled
                    | Update::Failed(_)
            )
        {
            if update == Update::Phase(Phase::Idle) {
                self.dictation.reports_to_skip -= 1;
            }
            return;
        }
        if matches!(update, Update::Done { .. } | Update::Failed(_))
            && self.dictation.engine.is_some()
        {
            self.dictation.reports_to_skip += 1;
        }
        match update {
            Update::Phase(Phase::Idle) => {
                if self.dictation.phase.is_active() {
                    self.dictation_is_idle(window, cx);
                }
            }
            Update::Phase(phase) => {
                if self.dictation.target.is_some() {
                    self.dictation.phase = phase;
                }
            }
            Update::Words(words) => {
                if self.dictation.target.is_some() && self.read_user_edit(window, cx) {
                    self.write_words(&words, window, cx);
                    self.color_dictated_words(cx);
                }
            }
            Update::Level(level) => self.dictation.level = level,
            Update::CloudLost {
                loss,
                can_finish_on_device,
            } => {
                self.dictation.failed_dot = self.dictation.target;
                self.dictation.notice = Some(DictationNotice::CloudLost {
                    loss,
                    can_finish_on_device,
                });
            }
            Update::CloudBack => {
                self.dictation.failed_dot = None;
                self.dictation.notice = None;
            }
            Update::Done {
                text,
                cleanup_failed,
                silence,
            } => self.finish_dictation(text, cleanup_failed, silence, window, cx),
            Update::Cancelled => {}
            Update::Failed(failure) => self.on_dictation_failure(failure, window, cx),
            Update::DownloadProgress { model, done, total } => {
                if let Some(DictationNotice::Downloading(rows)) = &mut self.dictation.notice
                    && let Some(row) = rows.iter_mut().find(|r| r.need.id == model)
                {
                    row.done = done;
                    row.total = total;
                }
            }
            Update::DownloadModelDone { model } => {
                if let Some(DictationNotice::Downloading(rows)) = &mut self.dictation.notice
                    && let Some(row) = rows.iter_mut().find(|r| r.need.id == model)
                {
                    row.finished = true;
                }
            }
            Update::DownloadsDone => {
                self.dictation.notice = None;
                self.refresh_dictation_models(cx);
                if let Some(target) = self.dictation.resume.take() {
                    self.start_dictation(target, window, cx);
                }
            }
            Update::DownloadFailed(error) => {
                self.dictation.notice = None;
                self.dictation.resume = None;
                self.show_error(
                    format!("Consort could not download the speech models. {error}"),
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// Reads an edit of the user since the last write. False: the user changed
    /// the dictated words, so they are the user's now, and the dictation ends.
    fn read_user_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(target) = self.dictation.target else {
            return false;
        };
        let now = self
            .dictation_input(target)
            .read_with(cx, |input, _| input.value().to_string());
        if now == self.dictation.expected {
            return true;
        }
        self.dictation.live_step = false;
        match self.dictation.draft.on_edit(&now) {
            Edit::InDraft => {
                self.dictation.expected = now;
                true
            }
            Edit::TookOver => {
                self.end_dictation_session(window, cx);
                false
            }
        }
    }

    fn finish_dictation(
        &mut self,
        text: String,
        cleanup_failed: bool,
        silence: Option<Duration>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dictation.target.is_none() {
            return;
        };
        if self.read_user_edit(window, cx) {
            self.write_words(&text, window, cx);
        }
        self.dictation_is_idle(window, cx);
        if silence.is_some()
            && let Some(limit) = self.dictation.settings.stop_after.limit()
        {
            self.show_notice(
                format!(
                    "Dictation stopped after {} seconds of silence.",
                    limit.as_secs()
                ),
                cx,
            );
        }
        if cleanup_failed {
            self.show_timed_dictation_notice(DictationNotice::CleanupFailed, cx);
        }
    }

    fn show_timed_dictation_notice(&mut self, notice: DictationNotice, cx: &mut Context<Self>) {
        self.dictation.notice = Some(notice.clone());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(INFO_TIMEOUT).await;
            let _ = this.update(cx, |this, cx| {
                if this.dictation.notice.as_ref() == Some(&notice) {
                    this.dictation.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn on_dictation_failure(
        &mut self,
        failure: Failure,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.dictation.target;
        match failure {
            Failure::NoPermission => {
                self.dictation.failed_dot = target;
                self.dictation.notice = Some(DictationNotice::NoPermission);
            }
            Failure::NoMicrophone => {
                self.dictation.failed_dot = target;
                self.dictation.notice = Some(DictationNotice::NoMicrophone);
            }
            Failure::NoKey => {
                self.dictation.failed_dot = target;
                self.dictation.notice = Some(DictationNotice::NoKey);
            }
            Failure::ModelsMissing(needs) => {
                self.dictation.resume = target;
                self.dictation.notice = Some(DictationNotice::ModelsMissing(needs));
            }
            Failure::Engine(error) => {
                self.show_error(
                    format!("Dictation could not run. Press the mic to try again. {error}"),
                    cx,
                );
            }
        }
        self.dictation_is_idle(window, cx);
    }

    /// Send with an active dictation. It stops the dictation and holds the
    /// message. True: the send stops here.
    pub(crate) fn hold_send_for_dictation(
        &mut self,
        target: TextTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.dictation.phase.is_active() || self.dictation.target != Some(target) {
            return false;
        }
        self.stop_dictation();
        self.dictation.notice = Some(DictationNotice::SendHeld);
        cx.notify();
        true
    }

    /// A button of a dictation notice.
    pub fn run_dictation_notice(
        &mut self,
        button: NoticeButton,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match button {
            NoticeButton::OpenSystemSettings => {
                if let Some(url) = consort_dictation::permission::settings_url() {
                    cx.open_url(url);
                }
            }
            NoticeButton::OpenDictationSettings => {
                self.open_settings(SettingsSection::Dictation, cx);
            }
            NoticeButton::Download => {
                if let Some(DictationNotice::ModelsMissing(needs)) = self.dictation.notice.clone() {
                    self.download_dictation_models(needs, window, cx);
                }
            }
            NoticeButton::CancelDownload => {
                self.dictation.resume = None;
                self.dictation.notice = None;
                if let Some(engine) = &self.dictation.engine {
                    engine.cancel_download();
                }
            }
            NoticeButton::TryAgain => {
                self.dictation.failed_dot = None;
                self.dictation.notice = None;
                if let Some(engine) = &self.dictation.engine {
                    engine.cloud(CloudChoice::TryAgain);
                }
            }
            NoticeButton::FinishOnDevice => {
                self.dictation.failed_dot = None;
                self.dictation.notice = None;
                if let Some(engine) = &self.dictation.engine {
                    engine.cloud(CloudChoice::FinishOnDevice);
                }
            }
            NoticeButton::SendNow => {
                // The words on screen go. The words still to come do not.
                let target = self.dictation.target;
                self.end_dictation_session(window, cx);
                self.dictation.notice = None;
                match target {
                    Some(TextTarget::Composer) => self.send_message(window, cx),
                    Some(TextTarget::Reply) => self.send_reply(window, cx),
                    None => {}
                }
            }
        }
        cx.notify();
    }

    pub fn close_dictation_notice(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.dictation.notice,
            Some(DictationNotice::ModelsMissing(_))
        ) {
            self.dictation.resume = None;
        }
        self.dictation.notice = None;
        self.dictation.failed_dot = None;
        cx.notify();
    }

    pub(crate) fn download_dictation_models(
        &mut self,
        needs: Vec<ModelNeed>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = needs
            .iter()
            .cloned()
            .map(|need| DownloadRow {
                total: need.size_bytes,
                need,
                done: 0,
                finished: false,
            })
            .collect();
        self.dictation.notice = Some(DictationNotice::Downloading(rows));
        if let Some(engine) = self.dictation_engine(window, cx) {
            engine.download(needs);
        }
        cx.notify();
    }

    /// Loads the state of the models for Settings > Dictation.
    pub(crate) fn refresh_dictation_models(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.dictation.engine.clone() else {
            return;
        };
        let settings = self.dictation.settings.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { (engine.needs(&settings), engine.catalog()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(needs) = result.0 {
                    this.dictation.needs = needs;
                }
                if let Ok(catalog) = result.1 {
                    this.dictation.languages = consort_dictation::speech_languages(&catalog);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens Settings > Dictation: the engine answers about its models, and
    /// the system about its microphones.
    pub(crate) fn open_dictation_page(&mut self, cx: &mut Context<Self>) {
        if self.dictation.engine_off {
            return;
        }
        self.dictation.page_wants_models = true;
        cx.spawn(async move |this, cx| {
            let devices = cx
                .background_executor()
                .spawn(async move { consort_dictation::devices() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.dictation.devices = devices;
                cx.notify();
            });
        })
        .detach();
    }

    /// Deletes the local models of Settings > Dictation.
    pub(crate) fn remove_dictation_models(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.dictation.engine.clone() else {
            return;
        };
        let needs: Vec<ModelNeed> = self
            .dictation
            .needs
            .iter()
            .filter(|n| n.on_disk && n.purpose != consort_dictation::Purpose::Cleanup)
            .cloned()
            .collect();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { engine.remove(&needs) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.show_error(
                        format!("Consort could not remove the speech models. {error}"),
                        cx,
                    );
                }
                this.refresh_dictation_models(cx);
            });
        })
        .detach();
    }

    /// Checks a Deepgram key with Deepgram, then keeps it on this computer.
    pub(crate) fn save_deepgram_key(
        &mut self,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = key.trim().to_owned();
        if key.is_empty() {
            self.dialog_error = Some("Enter your Deepgram key.".into());
            cx.notify();
            return;
        }
        let Some(engine) = self.dictation_engine(window, cx) else {
            return;
        };
        let checked = key.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { engine.check_key(&checked) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        if let Some(Err(error)) =
                            this.dictation.keys.as_ref().map(|keys| keys.save(&key))
                        {
                            this.dialog_error =
                                Some(format!("Consort could not save the key. {error}"));
                        } else {
                            this.dictation.has_key = true;
                            this.change_dictation_settings(|s| s.engine = Engine::Deepgram, cx);
                            this.close_dialog(window, cx);
                        }
                    }
                    Err(error) => {
                        this.dialog_error =
                            Some(format!("Deepgram did not take this key. {error}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn remove_deepgram_key(&mut self, cx: &mut Context<Self>) {
        if let Some(Err(error)) = self.dictation.keys.as_ref().map(KeyStore::remove) {
            self.show_error(format!("Consort could not remove the key. {error}"), cx);
            return;
        }
        self.dictation.has_key = false;
        self.change_dictation_settings(|s| s.engine = Engine::Local, cx);
    }

    /// Esc and the shortcut. True: the key press was for dictation.
    pub(crate) fn dictation_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let keystroke = &event.keystroke;
        if self.dictation.capturing_shortcut {
            return self.capture_shortcut(event, cx);
        }
        if keystroke.key == "escape" && self.dictation.phase.is_active() {
            self.cancel_dictation(window, cx);
            return true;
        }
        if !shortcut_matches(&self.dictation.settings.shortcut, keystroke) {
            return false;
        }
        let active = self.dictation.phase.is_active();
        let repeat = event.is_held;
        match self
            .dictation
            .press
            .key_down(active, repeat, Instant::now())
        {
            ShortcutAction::Start => {
                let target = self.focused_text_target(window, cx);
                self.start_dictation(target, window, cx);
            }
            ShortcutAction::Stop => self.stop_dictation(),
            ShortcutAction::Nothing => {}
        }
        true
    }

    /// The release of the shortcut.
    pub(crate) fn dictation_key_up(&mut self, event: &KeyUpEvent, cx: &mut Context<Self>) {
        let (key, _) = shortcut_parts(&self.dictation.settings.shortcut);
        if event.keystroke.key != key {
            return;
        }
        let active = self.dictation.phase.is_active();
        if self.dictation.press.key_up(active, Instant::now()) == ShortcutAction::Stop {
            self.stop_dictation();
            cx.notify();
        }
    }

    fn capture_shortcut(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let keystroke = &event.keystroke;
        if keystroke.key == "escape" {
            self.dictation.capturing_shortcut = false;
            cx.notify();
            return true;
        }
        let m = keystroke.modifiers;
        // A shortcut with no modifier would type a letter.
        if !(m.control || m.alt || m.platform) {
            return true;
        }
        let mut parts = Vec::new();
        if m.control {
            parts.push("ctrl");
        }
        if m.alt {
            parts.push("alt");
        }
        if m.shift {
            parts.push("shift");
        }
        if m.platform {
            parts.push("cmd");
        }
        parts.push(&keystroke.key);
        let shortcut = parts.join("-");
        self.dictation.capturing_shortcut = false;
        self.change_dictation_settings(|s| s.shortcut = shortcut, cx);
        true
    }

    /// The box with the focus, or the composer.
    fn focused_text_target(&self, window: &Window, cx: &gpui_kit::App) -> TextTarget {
        if self
            .inputs
            .reply
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
        {
            TextTarget::Reply
        } else {
            TextTarget::Composer
        }
    }

    /// Opens the dialog of the Deepgram key.
    pub(crate) fn open_deepgram_key_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_dialog(Dialog::DeepgramKey, window, cx);
    }
}

/// The designed states of dictation, for `--mock --screen dictation-<name>`.
pub(crate) fn open_dictation_screen(
    app: &mut ConsortApp,
    screen: &str,
    window: &mut Window,
    cx: &mut Context<ConsortApp>,
) {
    let need = |name: &str, size_bytes, purpose| ModelNeed {
        id: name.into(),
        name: name.into(),
        size_bytes,
        purpose,
        on_disk: false,
    };
    let needs = vec![
        need(
            "Parakeet TDT v3",
            483_000_000,
            consort_dictation::Purpose::Final,
        ),
        need(
            "Parakeet EOU 120M",
            222_000_000,
            consort_dictation::Purpose::Preview,
        ),
        need(
            "Cleanup 1 by Spokenly",
            639_000_000,
            consort_dictation::Purpose::Cleanup,
        ),
    ];
    let draft = "Please build it on bigboi.  Thanks.";
    app.inputs.composer.update(cx, |input, cx| {
        input.set_value(draft, window, cx);
        input.set_selected_range(26..26, cx);
    });
    let words = "then push the fix to pr 62 and tell me it is green";
    let screen = screen.to_owned();
    // The box must be on screen for the first write.
    cx.on_next_frame(window, move |app, window, cx| {
        let live = |app: &mut ConsortApp, window: &mut Window, cx: &mut Context<ConsortApp>| {
            app.start_dictation(TextTarget::Composer, window, cx);
            app.on_dictation_update(Update::Phase(Phase::Listening), window, cx);
            app.on_dictation_update(Update::Level(0.6), window, cx);
        };
        match screen.as_str() {
            "dictation-live" => {
                live(app, window, cx);
                app.on_dictation_update(Update::Words(words.into()), window, cx);
            }
            "dictation-finishing" => {
                live(app, window, cx);
                app.on_dictation_update(Update::Words(words.into()), window, cx);
                app.on_dictation_update(Update::Phase(Phase::Finishing), window, cx);
            }
            "dictation-send" => {
                live(app, window, cx);
                app.on_dictation_update(Update::Words(words.into()), window, cx);
                app.dictation.notice = Some(DictationNotice::SendHeld);
            }
            "dictation-permission" => {
                app.dictation.target = Some(TextTarget::Composer);
                app.on_dictation_update(Update::Failed(Failure::NoPermission), window, cx);
            }
            "dictation-models" => {
                app.dictation.notice = Some(DictationNotice::ModelsMissing(needs));
            }
            "dictation-download" => {
                let mut rows: Vec<DownloadRow> = needs
                    .into_iter()
                    .map(|need| DownloadRow {
                        total: need.size_bytes,
                        need,
                        done: 0,
                        finished: false,
                    })
                    .collect();
                rows[0].finished = true;
                rows[1].done = 130_000_000;
                app.dictation.notice = Some(DictationNotice::Downloading(rows));
            }
            "dictation-cloud" => {
                live(app, window, cx);
                app.on_dictation_update(Update::Words(words.into()), window, cx);
                app.on_dictation_update(
                    Update::CloudLost {
                        loss: CloudLoss::Connection,
                        can_finish_on_device: true,
                    },
                    window,
                    cx,
                );
            }
            _ => {}
        }
        cx.notify();
    });
}

/// `ctrl-alt-d` as the key and the modifiers.
fn shortcut_parts(shortcut: &str) -> (&str, Vec<&str>) {
    let mut parts: Vec<&str> = shortcut.split('-').collect();
    let key = parts.pop().unwrap_or_default();
    (key, parts)
}

fn shortcut_matches(shortcut: &str, keystroke: &gpui_kit::Keystroke) -> bool {
    let (key, modifiers) = shortcut_parts(shortcut);
    let m = keystroke.modifiers;
    keystroke.key == key
        && m.control == modifiers.contains(&"ctrl")
        && m.alt == modifiers.contains(&"alt")
        && m.shift == modifiers.contains(&"shift")
        && m.platform == modifiers.contains(&"cmd")
}

/// The key caps of the shortcut: Control, Option, D.
pub fn shortcut_caps(shortcut: &str) -> Vec<String> {
    let (key, modifiers) = shortcut_parts(shortcut);
    let mac = cfg!(target_os = "macos");
    let mut caps: Vec<String> = modifiers
        .iter()
        .map(|m| {
            match *m {
                "ctrl" => "Control",
                "alt" if mac => "Option",
                "alt" => "Alt",
                "shift" => "Shift",
                "cmd" if mac => "Command",
                "cmd" => "Super",
                other => other,
            }
            .to_owned()
        })
        .collect();
    caps.push(key.to_uppercase());
    caps
}

/// Brings the reports of the dictation to the app.
fn spawn_update_pump(
    updates: crossbeam_channel::Receiver<Update>,
    window: &mut Window,
    cx: &mut Context<ConsortApp>,
) {
    cx.spawn_in(
        window,
        async move |this: WeakEntity<ConsortApp>, cx: &mut AsyncWindowContext| {
            loop {
                let next = updates.clone();
                let Ok(update) = cx
                    .background_executor()
                    .spawn(async move { next.recv() })
                    .await
                else {
                    return;
                };
                let alive = this.update_in(cx, |this, window, cx| {
                    this.on_dictation_update(update, window, cx);
                });
                if alive.is_err() {
                    return;
                }
            }
        },
    )
    .detach();
}

/// The engine program: `CONSORT_SPEECH_ENGINE`, the engine in the helpers of
/// the macOS app, or the engine beside the app.
fn engine_program() -> PathBuf {
    if let Some(path) = std::env::var_os("CONSORT_SPEECH_ENGINE").filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let beside = exe.parent().map(PathBuf::from).unwrap_or_default();
    let helper = beside.join("../Helpers/sayso-engine/sayso-engine");
    if cfg!(target_os = "macos") && helper.exists() {
        return helper;
    }
    beside.join(if cfg!(windows) {
        "sayso-engine.exe"
    } else {
        "sayso-engine"
    })
}

/// The folder that Consort owns for the models and the recordings.
fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CONSORT_DICTATION_DATA").filter(|p| !p.is_empty()) {
        return PathBuf::from(dir);
    }
    directories::BaseDirs::new()
        .map(|dirs| dirs.data_dir().join("consort").join("dictation"))
        .unwrap_or_else(|| std::env::temp_dir().join("consort-dictation"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tap_starts_and_the_next_tap_stops() {
        let t = Instant::now();
        let mut press = ShortcutPress::default();
        assert_eq!(press.key_down(false, false, t), ShortcutAction::Start);
        assert_eq!(
            press.key_up(true, t + Duration::from_millis(100)),
            ShortcutAction::Nothing
        );
        assert_eq!(
            press.key_down(true, false, t + Duration::from_secs(5)),
            ShortcutAction::Stop
        );
        assert_eq!(
            press.key_up(true, t + Duration::from_secs(5)),
            ShortcutAction::Nothing
        );
    }

    #[test]
    fn a_hold_is_push_to_talk() {
        let t = Instant::now();
        let mut press = ShortcutPress::default();
        assert_eq!(press.key_down(false, false, t), ShortcutAction::Start);
        // The key repeats while it is held.
        assert_eq!(
            press.key_down(true, true, t + Duration::from_millis(50)),
            ShortcutAction::Nothing
        );
        assert_eq!(
            press.key_up(true, t + Duration::from_secs(2)),
            ShortcutAction::Stop
        );
    }

    #[test]
    fn a_dictation_that_ended_during_a_hold_has_nothing_to_stop() {
        let t = Instant::now();
        let mut press = ShortcutPress::default();
        press.key_down(false, false, t);
        assert_eq!(
            press.key_up(false, t + Duration::from_secs(2)),
            ShortcutAction::Nothing
        );
    }

    #[test]
    fn the_keycaps_read_as_in_the_design() {
        let caps = shortcut_caps(consort_dictation::DEFAULT_SHORTCUT);
        assert_eq!(caps.last().map(String::as_str), Some("D"));
        assert_eq!(caps[0], "Control");
    }

    #[test]
    fn the_shortcut_matches_only_with_its_own_modifiers() {
        let mut stroke = gpui_kit::Keystroke::parse("ctrl-alt-d").unwrap();
        assert!(shortcut_matches("ctrl-alt-d", &stroke));
        stroke.modifiers.shift = true;
        assert!(!shortcut_matches("ctrl-alt-d", &stroke));
    }

    #[test]
    fn the_notices_have_the_copy_and_the_buttons_of_the_design() {
        use DictationNotice as N;
        let need = |line: &str, size| ModelNeed {
            id: line.into(),
            name: line.into(),
            size_bytes: size,
            purpose: consort_dictation::Purpose::Final,
            on_disk: false,
        };
        let models = N::ModelsMissing(vec![
            need("a", 483_000_000),
            need("b", 222_000_000),
            need("c", 639_000_000),
        ]);
        assert_eq!(models.title(), "Dictation needs three speech models");
        assert_eq!(models.buttons()[0].1, "Download 1.3 GB");
        assert!(models.text().ends_with("Total 1.3 GB."));
        assert!(!models.is_error());

        let lost = N::CloudLost {
            loss: CloudLoss::Connection,
            can_finish_on_device: true,
        };
        assert!(lost.is_error());
        assert_eq!(lost.title(), "Lost the connection to Deepgram");
        let labels: Vec<_> = lost.buttons().into_iter().map(|b| b.1).collect();
        assert_eq!(labels, ["Try again", "Finish on this device"]);
        let no_model = N::CloudLost {
            loss: CloudLoss::Connection,
            can_finish_on_device: false,
        };
        assert_eq!(no_model.buttons().len(), 1);
        assert!(!no_model.text().contains("finish on this device"));

        assert_eq!(N::NoPermission.title(), "Microphone access is off");
        assert_eq!(N::NoPermission.buttons()[0].1, "Open System Settings");
        assert_eq!(N::NoMicrophone.buttons()[0].1, "Open Dictation settings");
        assert!(N::CleanupFailed.buttons().is_empty());
        assert_eq!(N::SendHeld.buttons()[0].1, "Send now");
    }
}
