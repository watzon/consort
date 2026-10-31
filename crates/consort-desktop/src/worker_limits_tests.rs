//! The limits of a worker in Settings > Workers: each control applies at once.

use consort_client::{ApprovalMode, mock};
use gpui::{TestAppContext, VisualTestContext};

use crate::app::{ConsortApp, Dialog, SettingsSection};
use crate::test_support::{click, has, open_sized, settle, until};

const WORKER: &str = "wrk_bigboi";

fn open_workers(cx: &mut TestAppContext) -> (gpui::AnyWindowHandle, gpui_kit::Entity<ConsortApp>) {
    let (handle, app) = open_sized(mock::view_state(), 1440., 1600., cx);
    let vcx = &mut VisualTestContext::from_window(handle, cx);
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(SettingsSection::Workers, cx)
        })
    });
    settle(vcx, 100);
    (handle, app)
}

fn folders(app: &gpui_kit::Entity<ConsortApp>, cx: &VisualTestContext) -> Vec<String> {
    cx.read(|cx| {
        app.read(cx)
            .state
            .worker(WORKER)
            .map(|w| w.policy.allowed_roots.clone())
            .unwrap_or_default()
    })
}

#[gpui::test]
fn the_dialog_adds_and_removes_a_folder(cx: &mut TestAppContext) {
    let (handle, app) = open_workers(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let before = folders(&app, cx);
    assert_eq!(before.len(), 2, "{before:?}");
    click(cx, "edit-folders");
    assert!(cx.read(|cx| matches!(app.read(cx).dialog, Some(Dialog::WorkerFolders(_)))));

    // An empty field adds nothing, and says why.
    click(cx, "folder-add");
    assert!(cx.read(|cx| app.read(cx).dialog_error.is_some()));
    assert_eq!(folders(&app, cx), before);

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let input = app.inputs.folder.clone();
            input.update(cx, |input, cx| input.set_value("E:\\Work", window, cx));
        });
    });
    click(cx, "folder-add");
    until(cx, &app, "the folder to show", |app| {
        app.state
            .worker(WORKER)
            .is_some_and(|w| w.policy.allowed_roots.len() == 3)
    });
    assert_eq!(
        folders(&app, cx).last().map(String::as_str),
        Some("E:\\Work")
    );
    settle(cx, 100);
    assert!(has(cx, "folder-row-2"));
    let typed = cx.read(|cx| app.read(cx).inputs.folder.read(cx).value().to_string());
    assert_eq!(typed, "", "the field is empty after the add");

    click(cx, "folder-remove-2");
    until(cx, &app, "the folder to go", |app| {
        app.state
            .worker(WORKER)
            .is_some_and(|w| w.policy.allowed_roots.len() == 2)
    });
    assert_eq!(folders(&app, cx), before);

    // With no folder left, the dialog says that there is no limit.
    click(cx, "folder-remove-0");
    until(cx, &app, "one folder to go", |app| {
        app.state
            .worker(WORKER)
            .is_some_and(|w| w.policy.allowed_roots.len() == 1)
    });
    settle(cx, 100);
    click(cx, "folder-remove-0");
    until(cx, &app, "the last folder to go", |app| {
        app.state
            .worker(WORKER)
            .is_some_and(|w| w.policy.allowed_roots.is_empty())
    });
    settle(cx, 100);
    assert!(has(cx, "folders-no-limit"));
}

#[gpui::test]
fn the_mode_and_the_shell_switch_apply_at_once(cx: &mut TestAppContext) {
    let (handle, app) = open_workers(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let shell = cx.read(|cx| {
        app.read(cx)
            .state
            .worker(WORKER)
            .unwrap()
            .policy
            .allow_shell
    });
    click(cx, "shell-access");
    until(cx, &app, "shell access to change", |app| {
        app.state.worker(WORKER).unwrap().policy.allow_shell != shell
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.pick_select("approval-mode", "full_access", window, cx);
        });
    });
    until(cx, &app, "the mode to change", |app| {
        app.state.worker(WORKER).unwrap().policy.max_approval_mode == ApprovalMode::FullAccess
    });
}
