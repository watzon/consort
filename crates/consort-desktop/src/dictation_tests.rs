//! The dictation in the real view: reports come as the crate sends them, and
//! the tests read the message box. `consort-dictation` tests the engine side.

use consort_dictation::{CloudLoss, Failure, ModelNeed, Phase, Purpose, StopAfter, Update};
use gpui::{TestAppContext, VisualTestContext};
use gpui_kit::component::input::{EditorState, Undo};
use gpui_kit::{Entity, Focusable as _};

use crate::app::{ConsortApp, Dialog, SettingsSection};
use crate::convo_tests::{draw, mock, open};
use crate::dictation::DictationNotice;
use crate::ui::menu::TextTarget;

fn set_up(
    text: &str,
    cursor: usize,
    cx: &mut TestAppContext,
) -> (Entity<ConsortApp>, Entity<EditorState>, VisualTestContext) {
    let (handle, app) = open(mock(), cx);
    let mut vcx = VisualTestContext::from_window(handle, cx);
    draw(&mut vcx);
    let input = app.read_with(&vcx, |app, _| app.inputs.composer.clone());
    let text = text.to_owned();
    vcx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            input.set_selected_range(cursor..cursor, cx);
        });
    });
    draw(&mut vcx);
    (app, input, vcx)
}

fn send(app: &Entity<ConsortApp>, update: Update, cx: &mut VisualTestContext) {
    app.update_in(cx, |app, window, cx| {
        app.on_dictation_update(update, window, cx)
    });
    draw(cx);
}

fn start(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) {
    app.update_in(cx, |app, window, cx| {
        app.start_dictation(TextTarget::Composer, window, cx)
    });
    draw(cx);
}

fn value(input: &Entity<EditorState>, cx: &VisualTestContext) -> String {
    input.read_with(cx, |input, _| input.value().to_string())
}

fn caret(input: &Entity<EditorState>, cx: &VisualTestContext) -> usize {
    input.read_with(cx, |input, _| input.selected_range().end)
}

fn undo(input: &Entity<EditorState>, cx: &mut VisualTestContext) {
    let handle = input.read_with(cx, |input, cx| input.focus_handle(cx));
    cx.update(|window, cx| handle.dispatch_action(&Undo, window, cx));
    draw(cx);
}

fn done(text: &str) -> Update {
    Update::Done {
        text: text.into(),
        cleanup_failed: false,
        silence: None,
    }
}

#[gpui::test]
fn words_come_between_the_draft_and_the_final_text_replaces_them(cx: &mut TestAppContext) {
    let (app, input, mut cx) = set_up("Please. Thanks.", 7, cx);
    start(&app, &mut cx);
    send(&app, Update::Phase(Phase::Listening), &mut cx);
    send(&app, Update::Words("then push".into()), &mut cx);
    assert_eq!(value(&input, &cx), "Please. then push Thanks.");
    assert_eq!(caret(&input, &cx), "Please. then push".len());
    send(&app, Update::Words("then push the fix".into()), &mut cx);
    assert_eq!(value(&input, &cx), "Please. then push the fix Thanks.");
    send(&app, done("Then push the fix."), &mut cx);
    send(&app, Update::Phase(Phase::Idle), &mut cx);
    assert_eq!(value(&input, &cx), "Please. Then push the fix. Thanks.");
    app.read_with(&cx, |app, _| {
        assert_eq!(app.dictation.phase, Phase::Idle);
        assert_eq!(app.dictation.target, None);
    });
}

#[gpui::test]
fn the_whole_dictation_is_one_undo_step(cx: &mut TestAppContext) {
    let (app, input, mut cx) = set_up("Draft", 5, cx);
    start(&app, &mut cx);
    for words in ["one", "one two", "one two three"] {
        send(&app, Update::Words(words.into()), &mut cx);
    }
    send(&app, done("One two three."), &mut cx);
    assert_eq!(value(&input, &cx), "Draft One two three.");
    undo(&input, &mut cx);
    assert_eq!(value(&input, &cx), "Draft");
}

#[gpui::test]
fn esc_removes_only_the_dictated_words_and_the_caret_goes_back(cx: &mut TestAppContext) {
    let (app, input, mut cx) = set_up("Before after", 6, cx);
    start(&app, &mut cx);
    send(&app, Update::Words("héllo wörld".into()), &mut cx);
    assert_eq!(value(&input, &cx), "Before héllo wörld after");
    cx.simulate_keystrokes("escape");
    draw(&mut cx);
    assert_eq!(value(&input, &cx), "Before after");
    assert_eq!(caret(&input, &cx), 6);
    app.read_with(&cx, |app, _| {
        assert_eq!(app.dictation.phase, Phase::Idle);
        assert_eq!(app.dictation.notice, None, "a cancel has no notice");
    });
}

#[gpui::test]
fn offsets_are_bytes_with_text_that_is_not_ascii(cx: &mut TestAppContext) {
    let draft = "日本語のテキスト";
    let (app, input, mut cx) = set_up(draft, "日本語の".len(), cx);
    start(&app, &mut cx);
    send(&app, Update::Words("ça va".into()), &mut cx);
    send(&app, Update::Words("ça va très".into()), &mut cx);
    let now = value(&input, &cx);
    assert!(now.starts_with("日本語の"), "{now}");
    assert!(now.ends_with("テキスト"), "{now}");
    assert!(now.contains("ça va très"), "{now}");
    assert!(!now.contains("ça va ça"), "{now}");
}

#[gpui::test]
fn typing_in_the_draft_keeps_the_dictation_alive(cx: &mut TestAppContext) {
    let (app, input, mut cx) = set_up("Draft", 5, cx);
    start(&app, &mut cx);
    send(&app, Update::Words("one".into()), &mut cx);
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.set_selected_range(0..0, cx);
            input.insert("Hi ", window, cx);
        });
    });
    draw(&mut cx);
    send(&app, Update::Words("one two".into()), &mut cx);
    assert_eq!(value(&input, &cx), "Hi Draft one two");
    assert!(app.read_with(&cx, |app, _| app.dictation.phase.is_active()));
    assert_eq!(caret(&input, &cx), 3, "the caret stays with the user");
}

#[gpui::test]
fn an_edit_of_the_dictated_words_gives_them_to_the_user(cx: &mut TestAppContext) {
    let (app, input, mut cx) = set_up("Draft", 5, cx);
    start(&app, &mut cx);
    send(&app, Update::Words("one two".into()), &mut cx);
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            let end = input.value().len();
            input.set_selected_range(end - 2..end - 1, cx);
            input.insert("X", window, cx);
        });
    });
    draw(&mut cx);
    let edited = value(&input, &cx);
    send(&app, Update::Words("one two three".into()), &mut cx);
    assert_eq!(value(&input, &cx), edited, "no word goes over the edit");
    assert_eq!(
        app.read_with(&cx, |app, _| app.dictation.phase),
        Phase::Idle
    );
}

#[gpui::test]
fn the_reply_box_dictates_too(cx: &mut TestAppContext) {
    let (app, _, mut cx) = set_up("", 0, cx);
    let reply = app.read_with(&cx, |app, _| app.inputs.reply.clone());
    app.update_in(&mut cx, |app, window, cx| {
        app.start_dictation(TextTarget::Reply, window, cx)
    });
    send(&app, Update::Words("one".into()), &mut cx);
    assert_eq!(value(&reply, &cx), "one");
}

#[gpui::test]
fn send_holds_the_message_until_the_dictation_is_done(cx: &mut TestAppContext) {
    let (app, input, mut cx) = set_up("", 0, cx);
    start(&app, &mut cx);
    send(&app, Update::Words("one".into()), &mut cx);
    app.update_in(&mut cx, |app, window, cx| app.send_message(window, cx));
    draw(&mut cx);
    assert_eq!(value(&input, &cx), "one", "nothing was sent");
    assert_eq!(
        app.read_with(&cx, |app, _| app.dictation.notice.clone()),
        Some(DictationNotice::SendHeld)
    );
    assert!(cx.debug_bounds("dictation-notice").is_some());
    send(&app, done("One."), &mut cx);
    assert_eq!(value(&input, &cx), "One.", "no send by itself");
    assert_eq!(
        app.read_with(&cx, |app, _| app.dictation.notice.clone()),
        None
    );
}

#[gpui::test]
fn send_now_sends_the_words_on_screen(cx: &mut TestAppContext) {
    let (app, input, mut cx) = set_up("", 0, cx);
    start(&app, &mut cx);
    send(&app, Update::Words("one".into()), &mut cx);
    app.update_in(&mut cx, |app, window, cx| app.send_message(window, cx));
    draw(&mut cx);
    let button = cx
        .debug_bounds("dictation-notice-button-0")
        .expect("Send now");
    cx.simulate_click(button.center(), gpui_kit::Modifiers::default());
    draw(&mut cx);
    assert_eq!(value(&input, &cx), "", "the words went with the message");
    assert_eq!(
        app.read_with(&cx, |app, _| app.dictation.phase),
        Phase::Idle
    );
}

#[gpui::test]
fn auto_stop_says_so_in_a_notice_that_leaves(cx: &mut TestAppContext) {
    let (app, _input, mut cx) = set_up("", 0, cx);
    app.update(&mut cx, |app, _| {
        app.dictation.settings.stop_after = StopAfter::S30
    });
    start(&app, &mut cx);
    send(&app, Update::Words("one".into()), &mut cx);
    send(
        &app,
        Update::Done {
            text: "One.".into(),
            cleanup_failed: false,
            silence: Some(std::time::Duration::from_secs(30)),
        },
        &mut cx,
    );
    // A notice that leaves by itself, not a line that stays in the tool row.
    let notice = app.read_with(&cx, |app, _| {
        app.notices
            .visible()
            .next()
            .map(|n| (n.message.clone(), n.kind))
    });
    assert_eq!(
        notice,
        Some((
            "Dictation stopped after 30 seconds of silence.".to_owned(),
            crate::notice::NoticeKind::Info
        ))
    );
}

#[gpui::test]
fn a_failed_cleanup_keeps_the_words_and_tells_the_user(cx: &mut TestAppContext) {
    let (app, input, mut cx) = set_up("", 0, cx);
    start(&app, &mut cx);
    send(
        &app,
        Update::Done {
            text: "as heard".into(),
            cleanup_failed: true,
            silence: None,
        },
        &mut cx,
    );
    assert_eq!(value(&input, &cx), "as heard");
    assert_eq!(
        app.read_with(&cx, |app, _| app.dictation.notice.clone()),
        Some(DictationNotice::CleanupFailed)
    );
}

#[gpui::test]
fn no_permission_marks_the_mic_and_shows_the_notice(cx: &mut TestAppContext) {
    let (app, input, mut cx) = set_up("Draft", 5, cx);
    start(&app, &mut cx);
    send(&app, Update::Failed(Failure::NoPermission), &mut cx);
    assert_eq!(value(&input, &cx), "Draft");
    assert_eq!(
        app.read_with(&cx, |app, _| app.dictation.phase),
        Phase::Idle
    );
    assert!(
        cx.debug_bounds("voice-failed").is_some(),
        "the dot of the mic"
    );
    assert!(cx.debug_bounds("dictation-notice").is_some());
}

#[gpui::test]
fn a_lost_connection_keeps_the_gray_words(cx: &mut TestAppContext) {
    let (app, input, mut cx) = set_up("", 0, cx);
    start(&app, &mut cx);
    send(&app, Update::Words("one".into()), &mut cx);
    send(
        &app,
        Update::CloudLost {
            loss: CloudLoss::Connection,
            can_finish_on_device: true,
        },
        &mut cx,
    );
    assert_eq!(value(&input, &cx), "one");
    assert!(app.read_with(&cx, |app, _| app.dictation.phase.is_active()));
    assert!(cx.debug_bounds("dictation-notice-button-1").is_some());
    send(&app, Update::CloudBack, &mut cx);
    assert_eq!(
        app.read_with(&cx, |app, _| app.dictation.notice.clone()),
        None
    );
}

#[gpui::test]
fn the_downloads_show_and_dictation_starts_when_they_end(cx: &mut TestAppContext) {
    let need = |id: &str, purpose| ModelNeed {
        id: id.into(),
        name: id.into(),
        size_bytes: 100_000_000,
        purpose,
        on_disk: false,
    };
    let (app, _, mut cx) = set_up("", 0, cx);
    start(&app, &mut cx);
    let needs = vec![need("a", Purpose::Final), need("b", Purpose::Cleanup)];
    send(
        &app,
        Update::Failed(Failure::ModelsMissing(needs.clone())),
        &mut cx,
    );
    assert_eq!(
        app.read_with(&cx, |app, _| app.dictation.phase),
        Phase::Idle
    );
    app.update_in(&mut cx, |app, window, cx| {
        app.download_dictation_models(needs, window, cx)
    });
    send(
        &app,
        Update::DownloadProgress {
            model: "a".into(),
            done: 40_000_000,
            total: 100_000_000,
        },
        &mut cx,
    );
    send(
        &app,
        Update::DownloadModelDone { model: "a".into() },
        &mut cx,
    );
    app.read_with(&cx, |app, _| {
        let Some(DictationNotice::Downloading(rows)) = &app.dictation.notice else {
            panic!("the download notice");
        };
        assert!(rows[0].finished);
        assert!(!rows[1].finished);
    });
    send(&app, Update::DownloadsDone, &mut cx);
    app.read_with(&cx, |app, _| {
        assert_eq!(app.dictation.notice, None);
        assert_eq!(app.dictation.phase, Phase::Starting, "it starts by itself");
    });
}

#[gpui::test]
fn old_reports_do_not_touch_a_new_dictation(cx: &mut TestAppContext) {
    let (app, input, mut cx) = set_up("", 0, cx);
    start(&app, &mut cx);
    app.update(&mut cx, |app, _| app.dictation.reports_to_skip = 1);
    send(&app, Update::Words("old".into()), &mut cx);
    send(&app, Update::Cancelled, &mut cx);
    send(&app, Update::Phase(Phase::Idle), &mut cx);
    assert_eq!(value(&input, &cx), "");
    assert_eq!(
        app.read_with(&cx, |app, _| app.dictation.phase),
        Phase::Starting
    );
    send(&app, Update::Words("new".into()), &mut cx);
    assert_eq!(value(&input, &cx), "new");
}

#[gpui::test]
fn the_mic_keeps_its_size_while_it_listens(cx: &mut TestAppContext) {
    let (app, _, mut cx) = set_up("", 0, cx);
    let rest = cx.debug_bounds("voice").expect("the mic");
    start(&app, &mut cx);
    send(&app, Update::Phase(Phase::Listening), &mut cx);
    let live = cx.debug_bounds("voice").expect("the live mic");
    assert_eq!(rest.size, live.size);
    assert_eq!(rest.origin, live.origin, "the row does not move");
    send(&app, Update::Phase(Phase::Finishing), &mut cx);
    assert_eq!(cx.debug_bounds("voice").map(|b| b.size), Some(rest.size));
}

#[gpui::test]
fn a_click_on_the_live_mic_does_not_start_a_second_dictation(cx: &mut TestAppContext) {
    let (app, _, mut cx) = set_up("Draft", 5, cx);
    let mic = cx.debug_bounds("voice").expect("the mic");
    cx.simulate_click(mic.center(), gpui_kit::Modifiers::default());
    draw(&mut cx);
    assert_eq!(
        app.read_with(&cx, |app, _| app.dictation.phase),
        Phase::Starting
    );
    assert_eq!(
        app.read_with(&cx, |app, _| app.dictation.target),
        Some(TextTarget::Composer)
    );
    cx.simulate_click(mic.center(), gpui_kit::Modifiers::default());
    draw(&mut cx);
    assert_eq!(
        app.read_with(&cx, |app, _| app.dictation.phase),
        Phase::Starting
    );
}

#[gpui::test]
fn the_shortcut_starts_in_the_focused_box(cx: &mut TestAppContext) {
    let (app, input, mut cx) = set_up("", 0, cx);
    cx.update(|window, cx| input.update(cx, |input, cx| input.focus(window, cx)));
    draw(&mut cx);
    cx.simulate_keystrokes("ctrl-alt-d");
    draw(&mut cx);
    app.read_with(&cx, |app, _| {
        assert_eq!(app.dictation.phase, Phase::Starting);
        assert_eq!(app.dictation.target, Some(TextTarget::Composer));
    });
}

#[gpui::test]
fn change_takes_the_next_chord_as_the_shortcut(cx: &mut TestAppContext) {
    let (app, _, mut cx) = set_up("", 0, cx);
    app.update(&mut cx, |app, cx| {
        app.open_settings(SettingsSection::Dictation, cx)
    });
    draw(&mut cx);
    // A click in the page focuses the root of the app. The button lies under
    // the fold of the small test window.
    assert!(cx.debug_bounds("dictation-change-shortcut").is_some());
    let root = app.read_with(&cx, |app, _| app.focus.clone());
    cx.update(|window, cx| root.focus(window, cx));
    app.update(&mut cx, |app, _| app.dictation.capturing_shortcut = true);
    draw(&mut cx);
    cx.simulate_keystrokes("a");
    assert!(
        app.read_with(&cx, |app, _| app.dictation.capturing_shortcut),
        "a plain key would type a letter"
    );
    cx.simulate_keystrokes("ctrl-alt-m");
    app.read_with(&cx, |app, _| {
        assert!(!app.dictation.capturing_shortcut);
        assert_eq!(app.dictation.settings.shortcut, "ctrl-alt-m");
    });
}

#[gpui::test]
fn settings_dictation_shows_its_groups(cx: &mut TestAppContext) {
    let (app, _, mut cx) = set_up("", 0, cx);
    app.update(&mut cx, |app, cx| {
        app.open_settings(SettingsSection::Dictation, cx)
    });
    draw(&mut cx);
    for selector in [
        "dictation-engine-local",
        "dictation-recommended",
        "dictation-engine-deepgram",
        "dictation-add-key",
        "dictation-cleanup",
        "dictation-cleanup-model",
        "dictation-stop-after",
        "dictation-language",
        "dictation-microphone",
        "dictation-change-shortcut",
    ] {
        assert!(cx.debug_bounds(selector).is_some(), "{selector}");
    }
}

#[gpui::test]
fn add_key_opens_its_dialog_and_an_empty_key_stays_there(cx: &mut TestAppContext) {
    let (app, _, mut cx) = set_up("", 0, cx);
    app.update(&mut cx, |app, cx| {
        app.open_settings(SettingsSection::Dictation, cx)
    });
    draw(&mut cx);
    let add = cx.debug_bounds("dictation-add-key").expect("Add key");
    cx.simulate_click(add.center(), gpui_kit::Modifiers::default());
    draw(&mut cx);
    assert_eq!(
        app.read_with(&cx, |app, _| app.dialog.clone()),
        Some(Dialog::DeepgramKey)
    );
    app.update_in(&mut cx, |app, window, cx| {
        app.save_deepgram_key(String::new(), window, cx)
    });
    assert_eq!(
        app.read_with(&cx, |app, _| app.dialog_error.clone())
            .as_deref(),
        Some("Enter your Deepgram key.")
    );
}
