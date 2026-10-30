//! The document in the right panel (Paper F1) in a headless window: a click
//! on a Markdown or text file opens it rendered, the header has its tools,
//! a file that is too large or fails says so, and the card that opened the
//! panel closes it (rule 4).

use crate::app::{ChannelTab, ConsortApp, Panel};
use crate::test_support::{bounds, click, draw, has, open, open_sized, settle, until};
use crate::ui::document::DocumentState;
use consort_client::{
    MessageView, TaskStatus, TimelineItem, ViewState,
    files::{ArtifactTask, AttachmentView},
    mock_files::{LIGHTS, SWITCH_TASK},
};
use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::{Entity, px};

const SHORTLIST: &str = "# Switch shortlist\n\nThree switches, each with a **dimmer**.\n\n- Lutron\n- Leviton\n- Legrand\n";

fn temp_file(name: &str, text: &str) -> String {
    let dir = std::env::temp_dir().join(format!("consort-document-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the temp folder");
    let path = dir.join(name);
    std::fs::write(&path, text).expect("the temp file");
    path.display().to_string()
}

/// house-lights with a Markdown artifact in a bubble and in the Files tab,
/// a text file, a file that is too large and a file with no copy.
fn state() -> ViewState {
    let mut state = consort_client::mock::view_state_with_files();
    let local = temp_file("switch-shortlist.md", SHORTLIST);
    let mut card = AttachmentView {
        local_path: Some(local.clone()),
        artifact: Some(ArtifactTask {
            task_id: SWITCH_TASK.into(),
            task_name: "Compare the porch switches".into(),
            status: TaskStatus::Review,
        }),
        ..consort_client::mock_files::attachment("switch-shortlist.md", None)
    };
    card.size = 6 * 1024;
    card.size_label = "6 KB".into();
    for file in &mut state.channel_files {
        if file.name == "switch-shortlist.md" {
            file.local_path = Some(local.clone());
        }
    }
    let mut huge = state.channel_files[0].clone();
    huge.path = "huge.md".into();
    huge.name = "huge.md".into();
    huge.folder = String::new();
    huge.image = false;
    huge.mime = "text/markdown".into();
    huge.sha256 = "ab".repeat(32);
    huge.size = 5 * 1024 * 1024;
    huge.size_label = "5.0 MB".into();
    huge.local_path = None;
    let mut missing = huge.clone();
    missing.path = "missing.txt".into();
    missing.name = "missing.txt".into();
    missing.sha256 = "cd".repeat(32);
    missing.size = 1024;
    missing.size_label = "1 KB".into();
    let mut notes = missing.clone();
    notes.path = "notes.txt".into();
    notes.name = "notes.txt".into();
    notes.sha256 = "ef".repeat(32);
    notes.local_path = Some(temp_file(
        "notes.txt",
        "first line\nsecond line\nthird line",
    ));
    state.channel_files.extend([huge, missing, notes]);
    let timeline = state
        .timelines
        .iter_mut()
        .find(|t| t.channel_id == LIGHTS)
        .expect("house-lights");
    timeline.items.push(TimelineItem::Message(MessageView {
        id: "msg_lights_doc".into(),
        channel_id: LIGHTS.into(),
        speaker: consort_client::Speaker::Consort,
        time_label: "8:30 PM".into(),
        created_at: consort_client::mock::MOCK_NOW,
        body: "The shortlist is in the document.".into(),
        attachments: vec![card],
        ..MessageView::default()
    }));
    consort_client::derive::refresh(&mut state);
    state
}

fn open_lights(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_channel(LIGHTS.into(), window, cx)));
    draw(cx);
}

fn show_files(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) {
    app.update(cx, |app, cx| {
        app.panel = Panel::Channel;
        app.channel_tab = ChannelTab::Files;
        cx.notify();
    });
    draw(cx);
}

fn document_state(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) -> Option<DocumentState> {
    app.read_with(cx, |app, _| app.document.as_ref().map(|d| d.state.clone()))
}

fn wait_ready(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) {
    until(cx, app, "the document loads", |app| {
        matches!(
            app.document.as_ref().map(|d| &d.state),
            Some(DocumentState::Ready(_))
        )
    });
}

#[gpui::test]
fn a_click_on_the_file_card_opens_the_document_rendered_in_the_panel(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    click(cx, "file-msg_lights_doc-0");
    wait_ready(&app, cx);
    app.read_with(cx, |app, _| assert_eq!(app.panel, Panel::Document));
    let t = app.read_with(cx, |_, cx| ConsortApp::tokens(cx));
    let panel = bounds(cx, "document-panel");
    assert_eq!(panel.size.width, t.layout.panel_width);
    // The header: the name at the left, Copy, Save and Open in another app,
    // and Close at the far right end.
    let name = bounds(cx, "document-name");
    let copy = bounds(cx, "document-copy");
    let save = bounds(cx, "document-save");
    let open_app = bounds(cx, "document-open-app");
    let close = bounds(cx, "document-close");
    assert!(name.left() < panel.left() + px(24.));
    assert!(name.right() <= copy.left());
    assert!(copy.right() <= save.left() && save.right() <= open_app.left());
    assert!(open_app.right() <= close.left());
    assert!(
        panel.right() - close.right() <= t.space.sm,
        "Close is at the far right end"
    );
    for part in [name, copy, save, open_app, close] {
        assert_eq!(part.center().y.floor(), name.center().y.floor(), "one row");
    }
    // The line under the header: the task, then the size and the date.
    let summary = bounds(cx, "document-summary");
    assert!(summary.top() >= name.bottom());
    assert!(has(cx, "document-task"));
    // The text is on the page, rendered, under the line.
    let text = bounds(cx, "document-text");
    assert!(text.top() >= summary.bottom());
    assert!(panel.contains(&text.center()));
    // The card of the open document has the selected ground.
    app.read_with(cx, |app, _| {
        let sha = app.document.as_ref().expect("open").sha256.clone();
        assert!(app.document_is_open(&sha));
    });
}

#[gpui::test]
fn the_card_that_opened_the_panel_closes_it_and_so_does_close(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    click(cx, "file-msg_lights_doc-0");
    wait_ready(&app, cx);
    // Rule 4: the opener closes it.
    click(cx, "file-msg_lights_doc-0");
    app.read_with(cx, |app, _| assert_eq!(app.panel, Panel::None));
    assert!(!has(cx, "document-panel"));
    // Close closes it too.
    click(cx, "file-msg_lights_doc-0");
    wait_ready(&app, cx);
    click(cx, "document-close");
    app.read_with(cx, |app, _| assert_eq!(app.panel, Panel::None));
}

#[gpui::test]
fn a_markdown_row_of_the_files_tab_opens_in_the_panel(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    show_files(&app, cx);
    let row = format!("file-row-artifacts/{SWITCH_TASK}/switch-shortlist.md");
    assert!(
        has(cx, &row) || has(cx, "file-row-switch-shortlist.md"),
        "the row"
    );
    let row = if has(cx, &row) {
        row
    } else {
        "file-row-switch-shortlist.md".into()
    };
    click(cx, &row);
    wait_ready(&app, cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.panel, Panel::Document);
        let open = app.document.as_ref().expect("open");
        assert_eq!(open.name, "switch-shortlist.md");
        // The Files tab names the task from the task list.
        assert_eq!(
            open.task.as_ref().map(|t| t.0.as_str()),
            app.state.task(SWITCH_TASK).map(|t| t.title.as_str())
        );
        let date = open.date_label.clone().expect("the row has a date");
        assert_eq!(open.summary(), format!("6 KB, {}", date.to_lowercase()));
    });
    assert!(has(cx, "document-text"));
}

#[gpui::test]
fn a_file_over_the_limit_says_so_and_offers_open_in_another_app(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    show_files(&app, cx);
    click(cx, "file-row-huge.md");
    // No download starts: the state is final at once.
    assert_eq!(document_state(&app, cx), Some(DocumentState::TooLarge));
    assert!(has(cx, "document-too-large"));
    assert!(has(cx, "document-open-app-text"));
    assert!(!has(cx, "document-text") && !has(cx, "document-retry"));
}

#[gpui::test]
fn a_failed_download_says_what_went_wrong_and_what_to_do(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    show_files(&app, cx);
    click(cx, "file-row-missing.txt");
    until(cx, &app, "the answer", |app| {
        matches!(
            app.document.as_ref().map(|d| &d.state),
            Some(DocumentState::Failed(_))
        )
    });
    draw(cx);
    assert!(has(cx, "document-error"));
    assert!(has(cx, "document-retry"));
    assert!(has(cx, "document-open-app-text"));
    let Some(DocumentState::Failed(message)) = document_state(&app, cx) else {
        panic!("failed");
    };
    assert!(
        message.starts_with("Consort could not get this document."),
        "{message}"
    );
    // Try again asks again, and the panel says so while it waits.
    click(cx, "document-retry");
    settle(cx, 300);
    assert!(matches!(
        document_state(&app, cx),
        Some(DocumentState::Failed(_))
    ));
}

#[gpui::test]
fn the_panel_says_that_it_loads(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    app.update(cx, |app, cx| {
        app.panel = Panel::Document;
        app.document = Some(crate::ui::document::OpenDocument {
            sha256: "00".repeat(32),
            name: "slow.md".into(),
            size: 1024,
            size_label: "1 KB".into(),
            date_label: None,
            task: None,
            state: DocumentState::Loading,
        });
        cx.notify();
    });
    draw(cx);
    assert!(has(cx, "document-loading"));
    assert!(!has(cx, "document-text") && !has(cx, "document-task"));
}

#[gpui::test]
fn copy_puts_the_text_on_the_clipboard_and_says_so(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    click(cx, "file-msg_lights_doc-0");
    wait_ready(&app, cx);
    click(cx, "document-copy");
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some(SHORTLIST));
    assert!(has(cx, "notice-stack"));
}

#[gpui::test]
fn a_text_file_keeps_its_line_breaks(cx: &mut TestAppContext) {
    let (handle, app) = open(state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    show_files(&app, cx);
    click(cx, "file-row-notes.txt");
    wait_ready(&app, cx);
    draw(cx);
    let t = app.read_with(cx, |_, cx| ConsortApp::tokens(cx));
    let text = bounds(cx, "document-text");
    assert!(
        text.size.height > t.text.mono.line * 2.5,
        "three lines: {text:?}"
    );
}

#[gpui::test]
fn in_a_compact_window_the_document_takes_the_window(cx: &mut TestAppContext) {
    let (handle, app) = open_sized(state(), 600., 900., cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    click(cx, "file-msg_lights_doc-0");
    wait_ready(&app, cx);
    let panel = bounds(cx, "document-panel");
    assert_eq!(panel.size.width, px(600.));
    assert!(has(cx, "document-close") && has(cx, "document-text"));
}
