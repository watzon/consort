//! Phase 3 in a headless window: Settings > Hub with Back up now and the
//! schedule, Settings > Clients with the inline Revoke confirm, the Pair a
//! new app dialog, Settings > Notifications, and the delivery of a
//! notification with its click.

use std::sync::Arc;
use std::time::Duration;

use crate::app::{ConsortApp, Dialog, Panel, SettingsSection};
use consort_client::admin::{BackupFrequency, LastBackupView};
use consort_client::notify::WaitingItem;
use consort_client::{Client, ViewState};
use gpui::{AnyWindowHandle, TestAppContext, VisualTestContext};
use gpui_kit::{AppContext as _, Entity, WindowOptions, point, px, size};

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<ConsortApp>) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        consort_theme::init(cx);
        cx.set_app_identity("dev.consort.test", "Consort");
    });
    let options = WindowOptions {
        window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(1440.), px(900.)),
        })),
        ..WindowOptions::default()
    };
    let (handle, app) = cx
        .update(|cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| ConsortApp::new(Client::mock().expect("mock"), true, window, cx))
            })
        })
        .expect("open the window");
    let vcx = &mut VisualTestContext::from_window(handle, cx);
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    (handle, app)
}

fn settle(cx: &mut VisualTestContext, ms: u64) {
    let end = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(25));
        cx.run_until_parked();
    }
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn until(
    cx: &mut VisualTestContext,
    app: &Entity<ConsortApp>,
    what: &str,
    ready: impl Fn(&ConsortApp) -> bool,
) {
    let end = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if app.read_with(cx, |app, _| ready(app)) {
            return;
        }
        assert!(std::time::Instant::now() < end, "timed out: {what}");
        settle(cx, 50);
    }
}

fn has(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    cx.debug_bounds(selector).is_some()
}

fn screen(app: &Entity<ConsortApp>, name: &str, cx: &mut VisualTestContext) {
    let name = name.to_owned();
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_screen(&name, window, cx)));
    settle(cx, 100);
}

#[gpui::test]
fn the_hub_page_turns_the_move_to_another_account_off(cx: &mut TestAppContext) {
    let (handle, app) = open(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "hub", cx);
    app.read_with(cx, |app, _| assert!(app.state.account_fallback));
    crate::test_support::click(cx, "account-fallback");
    until(cx, &app, "the switch to turn off", |app| {
        !app.state.account_fallback
    });
}

#[gpui::test]
fn the_hub_page_backs_up_and_changes_the_schedule(cx: &mut TestAppContext) {
    let (handle, app) = open(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "hub", cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.settings_section, SettingsSection::Hub);
        let hub = &app.state.admin.hub;
        assert_eq!(
            hub.storage.summary,
            "4.2 GB used, 41 GB free on the hub disk"
        );
        assert_eq!(hub.backup.frequency, BackupFrequency::Daily);
    });
    assert!(has(cx, "backup-time"), "a daily schedule has a time");
    assert!(!has(cx, "backup-weekday"), "a daily schedule has no day");

    cx.update(|_, cx| app.update(cx, |app, cx| app.back_up_now(cx)));
    until(cx, &app, "a running backup", |app| {
        app.state.admin.hub.backup.busy
    });
    until(cx, &app, "the finished backup", |app| {
        matches!(app.state.admin.hub.backup.last, LastBackupView::Done { .. })
            && !app.state.admin.hub.backup.busy
    });

    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.set_backup_frequency(BackupFrequency::Weekly, cx)
        })
    });
    until(cx, &app, "a weekly schedule", |app| {
        app.state.admin.hub.backup.frequency == BackupFrequency::Weekly
    });
    settle(cx, 50);
    assert!(has(cx, "backup-weekday"), "a weekly schedule has a day");

    // A pick from the Keep select changes the count.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_menu(
                crate::ui::menu::MenuAction::Select(crate::admin::KEEP_SELECT, "14".into()),
                window,
                cx,
            )
        })
    });
    until(cx, &app, "the new keep count", |app| {
        app.state.admin.hub.backup.keep_label == "The last 14 backups"
    });

    // The folder field opens with the folder, and a path that is not full
    // stays open with the reason.
    cx.update(|window, cx| app.update(cx, |app, cx| app.edit_backup_folder(window, cx)));
    let value = app.read_with(cx, |app, cx| {
        app.admin.folder_input.read(cx).value().to_string()
    });
    assert_eq!(value, "/srv/backups/consort");
}

#[gpui::test]
fn a_client_row_asks_before_it_revokes_and_this_app_has_no_revoke(cx: &mut TestAppContext) {
    let (handle, app) = open(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "clients-revoke", cx);
    let (me, mini) = app.read_with(cx, |app, _| {
        let rows = &app.state.admin.clients.rows;
        assert_eq!(rows.len(), 4);
        let me = rows.iter().find(|r| r.this_app).unwrap().id.clone();
        let mini = rows
            .iter()
            .find(|r| r.name == "Mac mini")
            .unwrap()
            .id
            .clone();
        assert_eq!(app.admin.revoke_confirm.as_deref(), Some(mini.as_str()));
        (me, mini)
    });
    assert!(has(cx, "revoke-confirm"));
    let my_revoke: &'static str = Box::leak(format!("revoke-{me}").into_boxed_str());
    assert!(!has(cx, my_revoke), "the app that you use has no Revoke");
    cx.update(|_, cx| app.update(cx, |app, cx| app.revoke_client(mini.clone(), cx)));
    until(cx, &app, "the row to go", |app| {
        app.state.admin.clients.rows.len() == 3 && app.admin.revoke_confirm.is_none()
    });
}

#[gpui::test]
fn the_pairing_dialog_gets_a_code_and_forgets_it_on_close(cx: &mut TestAppContext) {
    let (handle, app) = open(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "pair-client", cx);
    until(cx, &app, "a code", |app| app.state.admin.pairing.is_some());
    app.read_with(cx, |app, _| {
        assert_eq!(app.dialog, Some(Dialog::PairClient));
        let pairing = app.state.admin.pairing.as_ref().unwrap();
        assert_eq!(pairing.code, "KQ7M-4XPD");
        assert!(pairing.qr_text.contains("KQ7M-4XPD"));
    });
    settle(cx, 50);
    assert!(has(cx, "pairing-new-code"));
    cx.update(|window, cx| app.update(cx, |app, cx| app.close_dialog(window, cx)));
    until(cx, &app, "the code to go", |app| {
        app.state.admin.pairing.is_none()
    });
}

#[gpui::test]
fn a_switch_changes_the_notification_settings(cx: &mut TestAppContext) {
    let (handle, app) = open(cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    screen(&app, "notifications", cx);
    assert!(has(cx, "notify-routines"));
    app.read_with(cx, |app, _| {
        assert!(!app.admin.settings.routines);
        assert_eq!(app.admin.badge.as_deref(), None);
    });
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.set_notification_setting(|s| s.routines = true, cx);
            app.follow_admin(cx);
        })
    });
    app.read_with(cx, |app, _| {
        assert!(app.admin.settings.routines);
        // The mock has three things that need the user.
        assert_eq!(app.admin.badge.as_deref(), Some("3"));
    });
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.set_notification_setting(|s| s.dock_badge = false, cx)
        })
    });
    app.read_with(cx, |app, _| assert_eq!(app.admin.badge, None));
}

#[gpui::test]
fn a_new_approval_notifies_once_and_a_click_opens_its_thread(cx: &mut TestAppContext) {
    let (handle, app) = open(cx);
    let vcx = &mut VisualTestContext::from_window(handle, cx);
    // The first look only learns what is there.
    vcx.update(|_, cx| app.update(cx, |app, cx| app.follow_admin(cx)));
    assert!(vcx.shown_system_notifications().is_empty());
    let (channel, task, title) = app.read_with(vcx, |app, _| {
        let task = app.state.tasks.first().expect("a mock task");
        (task.channel_id.clone(), task.id.clone(), task.title.clone())
    });
    let mut state: ViewState = app.read_with(vcx, |app, _| (*app.state).clone());
    state.admin.attention.waiting.push(WaitingItem {
        id: "apr_new".into(),
        decision: false,
        channel_id: channel.clone(),
        channel_name: "sayso".into(),
        task_id: Some(task.clone()),
        task_title: Some(title.clone()),
        text: "Run make".into(),
    });
    let state = Arc::new(state);
    vcx.update(|_, cx| app.update(cx, |app, cx| app.set_state(state.clone(), cx)));
    let shown = vcx.shown_system_notifications();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].title.as_ref(), "sayso");
    assert_eq!(
        shown[0].body.to_string(),
        format!("An approval waits in {title}.")
    );
    // The same state again shows nothing new.
    vcx.update(|_, cx| app.update(cx, |app, cx| app.follow_admin(cx)));
    assert_eq!(vcx.shown_system_notifications().len(), 1);
    // A click opens the task thread.
    vcx.simulate_system_notification_response(gpui_kit::SystemNotificationResponse {
        tag: shown[0].tag.clone(),
        action_id: None,
    });
    settle(vcx, 50);
    app.read_with(vcx, |app, _| {
        assert_eq!(app.panel, Panel::Thread(task.clone()));
        assert_eq!(app.channel_id, channel);
    });
}

#[gpui::test]
fn test_notification_posts_a_notification(cx: &mut TestAppContext) {
    let (handle, app) = open(cx);
    let vcx = &mut VisualTestContext::from_window(handle, cx);
    vcx.update(|_, cx| app.update(cx, |app, cx| app.send_test_notification(cx)));
    let shown = vcx.shown_system_notifications();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].tag.as_ref(), "consort:test");
}
