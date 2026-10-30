//! Write in my voice, in the desktop app: the state and the actions of
//! Settings > Voice, the dialogs (Reset, the full guide, Edit as markdown,
//! Add a document), and the draft card (the tick, Edit and "Not how I
//! write").

use std::collections::{BTreeMap, BTreeSet};

use consort_client::Command;
use consort_client::voice::{
    Disclosure, GuideEdit, VoiceChanges, VoiceSource, VoiceSourceKind, VoiceSourceRow,
};
use gpui_kit::component::input::{InputState, TextareaState};
use gpui_kit::{AppContext as _, Context, Entity, Window};

use crate::app::{ConsortApp, Dialog, SettingsSection};

pub use consort_client::mock_voice::SCREENS;

/// A dialog of the voice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VoiceDialog {
    /// "Reset the voice?"
    ConfirmReset,
    /// The style guide that waits for Do it.
    Proposal,
    /// Edit as markdown.
    Markdown,
    /// Add a document as a source.
    AddDocument,
}

/// The text inputs of the voice.
pub struct VoiceInputs {
    /// The request of "Try it".
    pub request: Entity<InputState>,
    /// A new hard rule.
    pub rule: Entity<InputState>,
    /// A document path.
    pub document: Entity<InputState>,
    /// The Subject of a draft card in Edit.
    pub subject: Entity<InputState>,
    /// The Body of a draft card in Edit.
    pub body: Entity<TextareaState>,
    /// The note in Edit as markdown.
    pub markdown: Entity<TextareaState>,
}

impl VoiceInputs {
    pub fn new(window: &mut Window, cx: &mut Context<ConsortApp>) -> VoiceInputs {
        let field = |placeholder: &str, window: &mut Window, cx: &mut Context<ConsortApp>| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        VoiceInputs {
            request: field("Reply to Dana, say yes", window, cx),
            rule: field("Never sign as the team.", window, cx),
            document: field("house-lights/notes/announcement.md", window, cx),
            subject: field("Subject", window, cx),
            body: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(6, 16)
                    .placeholder("Write the text")
            }),
            markdown: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(16, 32)
                    .placeholder("Write the style guide in Markdown")
            }),
        }
    }

    /// The one-line fields, for the editor style.
    pub fn fields(&self) -> [&Entity<InputState>; 4] {
        [&self.request, &self.rule, &self.document, &self.subject]
    }

    /// The multi-line fields, for the editor style.
    pub fn areas(&self) -> [&Entity<TextareaState>; 2] {
        [&self.body, &self.markdown]
    }
}

/// What the desktop keeps for the voice screens.
#[derive(Clone, Debug, Default)]
pub struct VoiceUi {
    /// True once the app asked the hub for the voice.
    pub asked: bool,
    /// The source ticks while the voice is off. They go to the hub with
    /// "Turn on and learn".
    pub draft_sources: Option<Vec<VoiceSourceRow>>,
    /// The documents while the voice is off.
    pub draft_documents: Option<Vec<String>>,
    /// "Change sources" is open on the page with the voice on.
    pub changing_sources: bool,
    /// "Add a rule" is open.
    pub adding_rule: bool,
    /// The rule or example under the pointer: `<section>:<index>` for a
    /// rule, `ex:<section>:<index>` for an example.
    pub hovered: Option<String>,
    /// The draft cards whose "I checked the marked lines" is ticked, with
    /// the review hash of the card that the user saw. A new scan or an
    /// edit from another client makes a new hash, so the tick clears.
    pub checked: BTreeMap<String, String>,
    /// The draft card in Edit: the approval and the key of the card that
    /// shows the fields. The same card in another view stays as it is.
    pub editing: Option<(String, String)>,
    /// The draft cards that the user marked "Not how I write" in this run.
    pub feedback: BTreeSet<String>,
}

/// The key of a rule row: `<section>:<index>`.
pub fn rule_key(section: &str, index: usize) -> String {
    format!("{section}:{index}")
}

/// The key of an example row: `ex:<section>:<index>`.
pub fn example_key(section: &str, index: usize) -> String {
    format!("ex:{section}:{index}")
}

/// The sources as the hub takes them, from source rows and documents.
pub fn sources_of(rows: &[VoiceSourceRow], documents: &[String]) -> Vec<VoiceSource> {
    let mut out: Vec<VoiceSource> = rows
        .iter()
        .filter(|r| r.ticked)
        .filter_map(|r| {
            Some(VoiceSource {
                kind: r.kind?,
                connection_id: r.connection_id.parse().ok(),
                include_private: r.include_private,
                paths: Vec::new(),
            })
        })
        .collect();
    if !documents.is_empty() {
        out.push(VoiceSource {
            kind: VoiceSourceKind::Documents,
            connection_id: None,
            include_private: false,
            paths: documents.to_vec(),
        });
    }
    out
}

impl ConsortApp {
    /// Runs one voice command. A failure shows as a notice.
    pub fn run_voice(&mut self, command: Command, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.spawn_command(cx, async move { client.run(command).await });
    }

    /// Asks the hub for the voice once, when the page opens.
    pub fn load_voice(&mut self, cx: &mut Context<Self>) {
        if self.voice.asked {
            return;
        }
        self.voice.asked = true;
        self.run_voice(Command::LoadVoice, cx);
    }

    /// The source rows that the page shows: the ticks of the hub, or the
    /// user's ticks while the voice is off.
    pub fn voice_source_rows(&self) -> Vec<VoiceSourceRow> {
        if !self.state.voice.enabled
            && let Some(rows) = &self.voice.draft_sources
        {
            return rows.clone();
        }
        self.state.voice.sources.clone()
    }

    /// The documents that the page shows.
    pub fn voice_documents(&self) -> Vec<String> {
        if !self.state.voice.enabled
            && let Some(docs) = &self.voice.draft_documents
        {
            return docs.clone();
        }
        self.state.voice.documents.clone()
    }

    /// Sends the sources when the voice is on; keeps them here when it is
    /// off.
    fn put_sources(
        &mut self,
        rows: Vec<VoiceSourceRow>,
        documents: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if self.state.voice.enabled {
            self.run_voice(
                Command::SetVoice(VoiceChanges {
                    sources: Some(sources_of(&rows, &documents)),
                    ..VoiceChanges::default()
                }),
                cx,
            );
        } else {
            self.voice.draft_sources = Some(rows);
            self.voice.draft_documents = Some(documents);
        }
        cx.notify();
    }

    /// Ticks or unticks a source.
    pub fn toggle_voice_source(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        let mut rows = self.voice_source_rows();
        if let Some(row) = rows.iter_mut().find(|r| r.connection_id == connection_id) {
            row.ticked = !row.ticked;
        }
        let documents = self.voice_documents();
        self.put_sources(rows, documents, cx);
    }

    /// Ticks or unticks the direct messages of a chat source.
    pub fn toggle_voice_private(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        let mut rows = self.voice_source_rows();
        if let Some(row) = rows.iter_mut().find(|r| r.connection_id == connection_id) {
            row.include_private = !row.include_private;
            if row.include_private {
                row.ticked = true;
            }
        }
        let documents = self.voice_documents();
        self.put_sources(rows, documents, cx);
    }

    /// Adds a document path.
    pub fn add_voice_document(&mut self, path: String, cx: &mut Context<Self>) {
        let path = path.trim().to_owned();
        if path.is_empty() {
            return;
        }
        let rows = self.voice_source_rows();
        let mut documents = self.voice_documents();
        if !documents.contains(&path) {
            documents.push(path);
        }
        self.put_sources(rows, documents, cx);
    }

    /// Removes a document path.
    pub fn remove_voice_document(&mut self, path: &str, cx: &mut Context<Self>) {
        let rows = self.voice_source_rows();
        let mut documents = self.voice_documents();
        documents.retain(|d| d != path);
        self.put_sources(rows, documents, cx);
    }

    /// "Your edits to drafts".
    pub fn toggle_voice_edits(&mut self, cx: &mut Context<Self>) {
        let on = !self.state.voice.learn_from_edits;
        self.run_voice(
            Command::SetVoice(VoiceChanges {
                learn_from_edits: Some(on),
                ..VoiceChanges::default()
            }),
            cx,
        );
    }

    /// The switch and "Turn on and learn". The switch only turns on with a
    /// ticked source, and turning it off keeps the style guide.
    pub fn set_voice_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        if on {
            let rows = self.voice_source_rows();
            let documents = self.voice_documents();
            let sources = sources_of(&rows, &documents);
            if sources.is_empty() {
                self.show_error(
                    "Tick at least one source, then turn the voice on.".into(),
                    cx,
                );
                return;
            }
            self.run_voice(
                Command::SetVoice(VoiceChanges {
                    enabled: Some(true),
                    sources: Some(sources),
                    ..VoiceChanges::default()
                }),
                cx,
            );
            self.voice.draft_sources = None;
            self.voice.draft_documents = None;
        } else {
            self.run_voice(
                Command::SetVoice(VoiceChanges {
                    enabled: Some(false),
                    ..VoiceChanges::default()
                }),
                cx,
            );
        }
        cx.notify();
    }

    pub fn learn_voice_again(&mut self, cx: &mut Context<Self>) {
        self.run_voice(Command::LearnVoice, cx);
    }

    pub fn set_voice_disclosure(&mut self, disclosure: Disclosure, cx: &mut Context<Self>) {
        self.run_voice(
            Command::SetVoice(VoiceChanges {
                disclosure: Some(disclosure),
                ..VoiceChanges::default()
            }),
            cx,
        );
    }

    /// "Write a sample".
    pub fn try_voice(&mut self, cx: &mut Context<Self>) {
        let request = self.voice_inputs.request.read(cx).value().trim().to_owned();
        if request.is_empty() {
            self.show_error(
                "Write a request first, such as \"Reply to Dana, say yes\".".into(),
                cx,
            );
            return;
        }
        self.run_voice(Command::TryVoice { request }, cx);
    }

    /// Remove on a rule or an example, and Add a rule.
    pub fn edit_voice_guide(&mut self, edit: GuideEdit, cx: &mut Context<Self>) {
        self.voice.hovered = None;
        self.run_voice(Command::EditVoiceGuide(edit), cx);
        cx.notify();
    }

    /// Saves the rule in the "Add a rule" field.
    pub fn add_voice_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.voice_inputs.rule.read(cx).value().trim().to_owned();
        if text.is_empty() {
            self.voice.adding_rule = false;
            cx.notify();
            return;
        }
        self.voice_inputs
            .rule
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.voice.adding_rule = false;
        self.edit_voice_guide(GuideEdit::AddHardRule { text }, cx);
    }

    /// Opens a voice dialog.
    pub fn open_voice_dialog(
        &mut self,
        dialog: VoiceDialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_dialog(Dialog::Voice(dialog), window, cx);
    }

    /// Fills the fields of a voice dialog when it opens.
    pub fn prepare_voice_dialog(
        &mut self,
        dialog: &VoiceDialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match dialog {
            VoiceDialog::Markdown => {
                let content = self
                    .state
                    .voice
                    .note
                    .as_ref()
                    .map(|(content, _)| content.clone())
                    .unwrap_or_default();
                self.voice_inputs.markdown.update(cx, |area, cx| {
                    area.set_value(content, window, cx);
                    area.focus(window, cx);
                });
            }
            VoiceDialog::AddDocument => {
                self.voice_inputs.document.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    input.focus(window, cx);
                });
            }
            VoiceDialog::Proposal => {
                // The guide that waits comes with voice.get.
                self.run_voice(Command::LoadVoice, cx);
            }
            VoiceDialog::ConfirmReset => {}
        }
    }

    // The draft card ------------------------------------------------------

    /// Whether "I checked the marked lines" is ticked for the card with
    /// this review hash. A new scan, an Edit or an edit from another client
    /// gives a new hash, so the tick clears.
    pub fn draft_checked(&self, approval_id: &str, review_hash: &str) -> bool {
        self.voice
            .checked
            .get(approval_id)
            .is_some_and(|h| h == review_hash)
    }

    pub fn toggle_draft_checked(
        &mut self,
        approval_id: &str,
        review_hash: &str,
        cx: &mut Context<Self>,
    ) {
        if self.draft_checked(approval_id, review_hash) {
            self.voice.checked.remove(approval_id);
        } else {
            self.voice
                .checked
                .insert(approval_id.to_owned(), review_hash.to_owned());
        }
        cx.notify();
    }

    /// Allow or Don't on a draft card: Allow sends the tick with the review
    /// hash of the card that the user checked.
    pub fn answer_draft_card(
        &mut self,
        approval_id: String,
        choice: consort_client::ApprovalChoice,
        checked: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(checked) = checked.filter(|_| choice != consort_client::ApprovalChoice::Deny)
        else {
            self.answer_approval(approval_id, choice, window, cx);
            return;
        };
        if self.blocked_by_hub(cx) {
            return;
        }
        let client = self.client.clone();
        let id = approval_id.clone();
        self.run_guarded(
            format!("approval:{approval_id}"),
            async move {
                client
                    .run(Command::AnswerApproval {
                        approval_id: id,
                        choice,
                        checked: Some(checked),
                    })
                    .await
            },
            crate::drafts::Kept::Request("The request still waits for your answer."),
            |_, _, _| {},
            window,
            cx,
        );
    }

    /// Edit on a draft card: the subject and the text become fields.
    pub fn start_draft_edit(
        &mut self,
        approval_id: String,
        key: String,
        subject: Option<String>,
        body: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.voice_inputs.subject.update(cx, |input, cx| {
            input.set_value(subject.unwrap_or_default(), window, cx);
        });
        self.voice_inputs.body.update(cx, |area, cx| {
            area.set_value(body.unwrap_or_default(), window, cx);
            area.focus(window, cx);
        });
        self.voice.editing = Some((approval_id, key));
        cx.notify();
    }

    pub fn cancel_draft_edit(&mut self, cx: &mut Context<Self>) {
        self.voice.editing = None;
        cx.notify();
    }

    /// "Save changes": the hub checks the new text again, and the tick
    /// clears.
    pub fn save_draft_edit(
        &mut self,
        approval_id: String,
        subject_key: Option<String>,
        body_key: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let mut fields = Vec::new();
        if let Some(key) = subject_key {
            fields.push((key, self.voice_inputs.subject.read(cx).value().to_string()));
        }
        if let Some(key) = body_key {
            fields.push((key, self.voice_inputs.body.read(cx).value().to_string()));
        }
        self.voice.editing = None;
        self.voice.checked.remove(&approval_id);
        if !fields.is_empty() {
            self.run_voice(
                Command::EditApproval {
                    approval_id,
                    fields,
                },
                cx,
            );
        }
        cx.notify();
    }

    /// "Not how I write".
    pub fn not_my_voice(&mut self, approval_id: String, cx: &mut Context<Self>) {
        if !self.voice.feedback.insert(approval_id.clone()) {
            return;
        }
        self.run_voice(Command::VoiceFeedback { approval_id }, cx);
        self.show_notice("Noted. Consort asks in Tutti what is wrong.".into(), cx);
    }

    /// Opens a voice design screen. Returns false for another screen.
    pub fn open_voice_screen(
        &mut self,
        screen: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use consort_client::mock_voice::{DRAFT_APPROVAL, REPLY_TASK, TUTTI};
        match screen {
            "voice-off" | "voice-on" => {
                self.voice.asked = true;
                self.open_settings(SettingsSection::Voice, cx);
                if screen == "voice-on" {
                    // The hover state of the design: Remove on a rule.
                    self.voice.hovered = Some(rule_key("core", 1));
                }
            }
            "voice-offer" => self.open_channel(TUTTI.into(), window, cx),
            "voice-approval" | "voice-approval-checked" | "voice-approval-edit" => {
                self.open_channel(TUTTI.into(), window, cx);
                self.open_thread(REPLY_TASK.into(), window, cx);
                if screen == "voice-approval-checked" {
                    self.voice.checked.insert(
                        DRAFT_APPROVAL.into(),
                        consort_client::mock_voice::MOCK_REVIEW_HASH.into(),
                    );
                }
                if screen == "voice-approval-edit" {
                    // The card in the task thread, as the design shows it.
                    self.start_draft_edit(
                        DRAFT_APPROVAL.into(),
                        "pst_reply_dana_2.0".into(),
                        Some("Venue for the team offsite".into()),
                        Some(consort_client::mock_voice::DRAFT_BODY.into()),
                        window,
                        cx,
                    );
                }
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sources_go_to_the_hub_with_the_ticked_rows_and_the_documents() {
        let rows = consort_client::mock_voice::off_view().sources;
        let list = sources_of(&rows, &["notes/post.md".to_owned()]);
        assert_eq!(list.len(), 3);
        assert_eq!(list[2].kind, VoiceSourceKind::Documents);
        assert_eq!(rule_key("core", 1), "core:1");
        assert_eq!(example_key("chat", 0), "ex:chat:0");
    }
}
