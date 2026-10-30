//! Phase 4 in a headless window: reactions and the message menu, the thread
//! of a message, the search palette, New task, the task item view, the
//! channel settings, the simple New channel dialog, groups, the task thread
//! in bubbles, About, and the voice tool that is gone.

use std::time::Duration;

use crate::app::{ConsortApp, Dialog, Field, Panel, TasksTab};
use crate::convo::ConvoAction;
use crate::ui::menu::MenuAction;
use consort_client::{
    Client, TimelineItem, ViewState,
    convo::{ItemDetailState, SearchLoad, SearchScope},
    mock_convo,
};
use gpui::{AnyWindowHandle, TestAppContext, VisualTestContext};
use gpui_kit::{AppContext as _, Entity, WindowOptions, point, px, size};

fn open(state: ViewState, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<ConsortApp>) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        consort_theme::init(cx);
        crate::convo::bind_keys(cx);
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
                cx.new(|cx| {
                    ConsortApp::new(Client::mock_with(state).expect("mock"), true, window, cx)
                })
            })
        })
        .expect("open the window");
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    (handle, app)
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

/// Waits until `ready` holds for the app, for at most 5 seconds.
fn until(
    cx: &mut VisualTestContext,
    app: &Entity<ConsortApp>,
    what: &str,
    ready: impl Fn(&ConsortApp) -> bool,
) {
    let end = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        if app.read_with(cx, |app, _| ready(app)) {
            draw(cx);
            return;
        }
        assert!(
            std::time::Instant::now() < end,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn mock() -> ViewState {
    consort_client::mock::view_state()
}

fn tutti(app: &Entity<ConsortApp>, cx: &VisualTestContext) -> String {
    app.read_with(cx, |app, _| app.state.tutti().unwrap().id.clone())
}

fn message(app: &ConsortApp, id: &str) -> consort_client::MessageView {
    app.find_message(id).expect("the message")
}

#[gpui::test]
fn the_message_menu_has_the_quick_emoji_and_a_second_click_removes_one(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    // The chips under the Tutti answer: yours in the accent, a count when
    // both of you chose the same emoji.
    app.read_with(cx, |app, _| {
        let m = message(app, "msg_tutti_reply");
        assert_eq!(m.reactions.len(), 2);
        assert_eq!(m.reactions[1].count_label().as_deref(), Some("2"));
    });
    assert!(cx.debug_bounds("bubble-msg_tutti_reply.p0").is_some());
    // A right click on the bubble opens the message menu with the bar.
    let bubble = cx
        .debug_bounds("bubble-msg_tutti_reply.p1")
        .expect("the bubble");
    cx.simulate_event(gpui_kit::MouseDownEvent {
        button: gpui_kit::MouseButton::Right,
        position: bubble.center(),
        modifiers: gpui_kit::Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    draw(cx);
    app.read_with(cx, |app, _| {
        let menu = app.menu.as_ref().expect("the menu");
        let bar = menu.reactions.as_ref().expect("the quick emoji");
        assert_eq!(bar.message_id, "msg_tutti_reply");
        assert!(bar.chosen.contains(&"🙏".to_owned()));
        let labels: Vec<&str> = menu.items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(
            labels,
            [
                "Copy text",
                "Copy as Markdown",
                "Reply in thread",
                "Quote in reply"
            ]
        );
    });
    // 🙏 is chosen, so the bar removes it.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_menu(
                MenuAction::Convo(ConvoAction::React {
                    message_id: "msg_tutti_reply".into(),
                    emoji: "🙏".into(),
                    remove: true,
                }),
                window,
                cx,
            )
        })
    });
    until(cx, &app, "the reaction removed", |app| {
        message(app, "msg_tutti_reply").reactions.len() == 1
    });
    // More opens the full picker, and a pick adds the reaction.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_menu(
                MenuAction::Convo(ConvoAction::MoreEmoji("msg_tutti_reply".into())),
                window,
                cx,
            )
        })
    });
    draw(cx);
    assert!(app.read_with(cx, |app, _| app.convo.emoji_picker.is_some()));
    assert!(
        cx.debug_bounds("emoji-picker").is_some(),
        "the picker shows"
    );
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_convo(
                ConvoAction::React {
                    message_id: "msg_tutti_reply".into(),
                    emoji: "🚀".into(),
                    remove: false,
                },
                window,
                cx,
            )
        })
    });
    until(cx, &app, "the new reaction", |app| {
        message(app, "msg_tutti_reply")
            .reactions
            .iter()
            .any(|r| r.emoji == "🚀" && r.mine)
    });
    assert!(app.read_with(cx, |app, _| app.convo.emoji_picker.is_none()));
}

#[gpui::test]
fn reply_in_thread_opens_the_thread_panel_with_the_ring_and_a_reply_goes_there(
    cx: &mut TestAppContext,
) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let tutti = tutti(&app, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_channel(tutti.clone(), window, cx);
            app.run_menu(
                MenuAction::ReplyInThread("msg_tutti_reply".into()),
                window,
                cx,
            );
        })
    });
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.panel, Panel::MessageThread("msg_tutti_reply".into()));
        let thread = app.state.convo.thread("msg_tutti_reply").expect("thread");
        assert_eq!(thread.subtitle, "In Tutti, 4 replies");
        assert_eq!(thread.replies.len(), 4);
    });
    assert!(cx.debug_bounds("message-thread-panel").is_some());
    assert!(cx.debug_bounds("thread-reply-box").is_some());
    // The reply line under the message says how many and when.
    let footer = app.read_with(cx, |app, _| {
        message(app, "msg_tutti_reply").last_reply_label
    });
    assert_eq!(footer.as_deref(), Some("Last reply 9:31 AM"));

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.convo_inputs
                .thread_reply
                .update(cx, |input, cx| input.set_value("Thanks.", window, cx));
            app.send_thread_reply(window, cx);
        })
    });
    until(cx, &app, "the reply in the thread", |app| {
        app.state
            .convo
            .thread("msg_tutti_reply")
            .is_some_and(|t| t.replies.iter().any(|r| r.body == "Thanks."))
    });
    app.read_with(cx, |app, _| {
        assert_eq!(message(app, "msg_tutti_reply").reply_count, 5);
    });
}

#[gpui::test]
fn search_shows_local_hits_at_once_then_the_hub_and_tab_changes_the_scope(cx: &mut TestAppContext) {
    mock_convo::hold_search(false);
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_switcher(window, cx)));
    draw(cx);
    cx.simulate_input("windows engine");
    draw(cx);
    // Go to shows at once, before the hub answers.
    app.read_with(cx, |app, cx| {
        assert!(app.switcher_hits(cx).iter().any(|h| matches!(
            h,
            crate::ui::switcher::Hit::Channel { name, .. } if name == "sayso"
        )));
    });
    assert!(cx.debug_bounds("search-palette").is_some());
    // The hub search waits for a pause in the typing.
    cx.executor().advance_clock(Duration::from_millis(300));

    until(cx, &app, "the hub results", |app| {
        app.state.convo.search.load == SearchLoad::Done && !app.state.convo.search.hits.is_empty()
    });
    app.read_with(cx, |app, _| {
        assert!(app.state.convo.search.hits.iter().any(|h| h.tutti));
    });
    cx.simulate_keystrokes("tab");
    draw(cx);
    until(cx, &app, "the Messages scope", |app| {
        app.state.convo.search.scope == SearchScope::Messages
            && app.state.convo.search.load == SearchLoad::Done
    });
    app.read_with(cx, |app, _| {
        assert_eq!(app.convo.search_scope, SearchScope::Messages);
        assert!(
            app.state
                .convo
                .search
                .hits
                .iter()
                .all(|h| h.section == consort_client::convo::SearchSection::Messages)
        );
    });
    // Enter opens the first result in place: the Tutti message.
    cx.simulate_keystrokes("enter");
    draw(cx);
    app.read_with(cx, |app, _| {
        assert!(app.switcher.is_none());
        assert_eq!(app.channel_id, app.state.tutti().unwrap().id);
    });
}

#[gpui::test]
fn new_task_makes_a_scheduled_task_with_the_dialog_fields(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_new_task(window, cx);
            app.convo_inputs.task_title.update(cx, |input, cx| {
                input.set_value("Fix the tray icon", window, cx)
            });
        })
    });
    draw(cx);
    // No brief: the dialog says so and stays.
    cx.update(|window, cx| app.update(cx, |app, cx| app.create_task(window, cx)));
    app.read_with(cx, |app, _| {
        assert_eq!(app.dialog, Some(Dialog::NewTask));
        assert!(app.dialog_error.as_deref().unwrap().contains("brief"));
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.convo_inputs.task_brief.update(cx, |input, cx| {
                input.set_value("Find the cause and fix it.", window, cx)
            });
            app.convo.new_task.later = true;
            app.run_convo(ConvoAction::TaskTime(7 * 60), window, cx);
            app.create_task(window, cx);
        })
    });
    until(cx, &app, "the new task", |app| {
        app.state
            .tasks
            .iter()
            .any(|t| t.title == "Fix the tray icon")
    });
    app.read_with(cx, |app, _| {
        let task = app
            .state
            .tasks
            .iter()
            .find(|t| t.title == "Fix the tray icon")
            .unwrap();
        assert_eq!(task.status, consort_client::TaskStatus::Scheduled);
        assert!(app.dialog.is_none());
    });
}

#[gpui::test]
fn an_activity_line_opens_the_item_in_the_wide_panel_and_back_closes_it(cx: &mut TestAppContext) {
    mock_convo::hold_item(false);
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let task = app.read_with(cx, |app, _| {
        app.state
            .tasks
            .iter()
            .find(|t| t.channel_name == "sayso" && t.status.is_your_turn())
            .unwrap()
            .id
            .clone()
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_thread(task.clone(), window, cx);
            app.open_task_item(
                task.clone(),
                mock_convo::SESSION.into(),
                mock_convo::ITEM_BUILD.into(),
                cx,
            );
        })
    });
    until(cx, &app, "the loaded output", |app| {
        app.state
            .convo
            .item
            .as_ref()
            .is_some_and(|i| matches!(i.state, ItemDetailState::Loaded { .. }))
    });
    let view = cx.debug_bounds("item-view").expect("the item view");
    let t = app.read_with(cx, |_, cx| ConsortApp::tokens(cx));
    assert_eq!(view.size.width, t.component.convo.panel_wide);
    app.read_with(cx, |app, _| {
        let item = app.state.convo.item.as_ref().unwrap();
        assert_eq!(item.position_label().as_deref(), Some("Item 9 of 20"));
    });
    cx.update(|_, cx| app.update(cx, |app, cx| app.close_task_item(cx)));
    until(cx, &app, "the thread again", |app| {
        app.state.convo.item.is_none()
    });
    assert!(cx.debug_bounds("item-view").is_none());
}

#[gpui::test]
fn an_item_on_an_offline_worker_says_so(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let task = app.read_with(cx, |app, _| {
        app.state
            .tasks
            .iter()
            .find(|t| t.channel_name == "homelab")
            .unwrap()
            .id
            .clone()
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_thread(task.clone(), window, cx);
            app.open_task_item(
                task,
                mock_convo::SESSION.into(),
                mock_convo::ITEM_OFFLINE.into(),
                cx,
            );
        })
    });
    until(cx, &app, "the offline state", |app| {
        app.state
            .convo
            .item
            .as_ref()
            .is_some_and(|i| i.state == ItemDetailState::Offline)
    });
    assert!(cx.debug_bounds("item-view").is_some());
}

#[gpui::test]
fn channel_settings_save_only_what_changed(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let sayso = app.read_with(cx, |app, _| {
        app.state
            .channels
            .iter()
            .find(|c| c.name == "sayso")
            .unwrap()
            .id
            .clone()
    });
    // The channel menu items of the old dialogs open the settings.
    for action in [
        MenuAction::RenameChannel(sayso.clone()),
        MenuAction::ChannelInstructions(sayso.clone()),
        MenuAction::ChannelModel(sayso.clone()),
        MenuAction::ChannelTags(sayso.clone()),
    ] {
        cx.update(|window, cx| app.update(cx, |app, cx| app.run_menu(action, window, cx)));
        draw(cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.dialog, Some(Dialog::ChannelSettings(sayso.clone())))
        });
    }
    // Nothing changed: nothing to send.
    app.read_with(cx, |app, cx| {
        let (_, changes) = app.settings_changes(cx).unwrap();
        assert_eq!(changes, consort_client::ChannelChanges::default());
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.convo_inputs
                .settings_purpose
                .update(cx, |input, cx| input.set_value("Ship Sayso", window, cx));
            app.convo.settings.memory = false;
            app.convo.settings.servers.clear();
            app.run_convo(
                ConvoAction::SettingsMode(consort_client::ApprovalMode::Ask),
                window,
                cx,
            );
            app.save_channel_settings(window, cx);
        })
    });
    until(cx, &app, "the saved channel", |app| {
        app.state
            .channel(&sayso)
            .is_some_and(|c| c.purpose == "Ship Sayso" && !c.memory_enabled)
    });
    app.read_with(cx, |app, _| {
        let c = app.state.channel(&sayso).unwrap();
        assert!(c.allowed_local_mcp_servers.is_empty());
        assert_eq!(c.default_approval_mode, consort_client::ApprovalMode::Ask);
        assert_eq!(c.name, "sayso", "the name did not change");
        assert!(app.dialog.is_none());
    });
}

#[gpui::test]
fn new_channel_in_a_group_has_an_icon_and_opens(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let home = app.read_with(cx, |app, _| {
        app.state
            .groups
            .iter()
            .find(|g| g.name == "Home")
            .unwrap()
            .id
            .clone()
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_menu(
                MenuAction::Convo(ConvoAction::NewChannel(Some(home.clone()))),
                window,
                cx,
            );
            app.run_convo(ConvoAction::PickIcon("memory".into()), window, cx);
            app.set_field(Field::Name, "reading", window, cx);
        })
    });
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.dialog, Some(Dialog::NewChannel));
        assert_eq!(app.convo.new_channel_group.as_deref(), Some(home.as_str()));
    });
    let confirm = cx.debug_bounds("dialog-confirm").expect("Create channel");
    cx.simulate_click(confirm.center(), gpui_kit::Modifiers::default());
    until(cx, &app, "the new channel", |app| {
        app.state.channels.iter().any(|c| c.name == "reading")
    });
    app.read_with(cx, |app, _| {
        let c = app
            .state
            .channels
            .iter()
            .find(|c| c.name == "reading")
            .unwrap();
        assert_eq!(c.icon, "memory");
        assert_eq!(c.group_id.as_deref(), Some(home.as_str()));
        assert_eq!(app.channel_id, c.id, "the new channel opens");
    });
}

#[gpui::test]
fn delete_group_asks_first_ignores_enter_and_keeps_the_channels(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let home = app.read_with(cx, |app, _| {
        app.state
            .groups
            .iter()
            .find(|g| g.name == "Home")
            .unwrap()
            .id
            .clone()
    });
    let menu = app.read_with(cx, |app, _| app.group_menu_items(&home));
    let labels: Vec<&str> = menu.iter().map(|i| i.label.as_ref()).collect();
    assert_eq!(labels, ["New channel", "Rename", "Delete group"]);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_menu(
                MenuAction::Convo(ConvoAction::DeleteGroup(home.clone())),
                window,
                cx,
            )
        })
    });
    draw(cx);
    // Enter does not press a destructive button.
    assert!(app.read_with(cx, |app, _| app.dialog_confirm.borrow().is_none()));
    let channels_before = app.read_with(cx, |app, _| app.state.channels.len());
    let confirm = cx.debug_bounds("dialog-confirm").expect("Delete group");
    cx.simulate_click(confirm.center(), gpui_kit::Modifiers::default());
    until(cx, &app, "the group gone", |app| {
        !app.state.groups.iter().any(|g| g.id == home)
    });
    app.read_with(cx, |app, _| {
        assert_eq!(app.state.channels.len(), channels_before);
        assert!(
            app.state
                .channels
                .iter()
                .filter(|c| c.name == "homelab" || c.name == "house-lights")
                .all(|c| c.group_id.is_none())
        );
    });
}

#[gpui::test]
fn groups_reorder_by_drop_and_an_empty_group_offers_add_a_channel(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let ids = app.read_with(cx, |app, _| {
        app.state
            .groups
            .iter()
            .map(|g| g.id.clone())
            .collect::<Vec<_>>()
    });
    cx.update(|_, cx| app.update(cx, |app, cx| app.drop_group(&ids[1], Some(&ids[0]), cx)));
    until(cx, &app, "the new order", |app| {
        app.state.groups.first().map(|g| g.id.as_str()) == Some(ids[1].as_str())
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_dialog(Dialog::NewGroup(None), window, cx);
            app.set_field(Field::Name, "Learning", window, cx);
        })
    });
    let confirm = cx.debug_bounds("dialog-confirm").expect("Create group");
    cx.simulate_click(confirm.center(), gpui_kit::Modifiers::default());
    until(cx, &app, "the new group", |app| {
        app.state.groups.iter().any(|g| g.name == "Learning")
    });
    let learning = app.read_with(cx, |app, _| {
        app.state
            .groups
            .iter()
            .find(|g| g.name == "Learning")
            .unwrap()
            .id
            .clone()
    });
    assert!(
        cx.debug_bounds("add-channel").is_some(),
        "the empty group offers Add a channel"
    );
    app.read_with(cx, |app, _| {
        let group = app
            .state
            .sidebar
            .groups
            .iter()
            .find(|g| g.id == learning)
            .unwrap();
        assert!(group.channels.is_empty());
    });
}

#[gpui::test]
fn the_task_menu_offers_delete_and_retry_after_the_session_ended(cx: &mut TestAppContext) {
    let mut state = mock();
    let review = state
        .tasks
        .iter_mut()
        .find(|t| t.status == consort_client::TaskStatus::Review)
        .unwrap();
    review.session_ended = true;
    let review_id = review.id.clone();
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let items = app.read_with(cx, |app, _| {
        let task = app.state.task(&review_id).unwrap();
        crate::ui::timeline::task_menu(&task.id, task.status, task.session_ended, true)
    });
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
    assert!(labels.contains(&"Retry"));
    assert_eq!(labels.last(), Some(&"Delete"));
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_menu(
                MenuAction::Convo(ConvoAction::DeleteTask(review_id.clone())),
                window,
                cx,
            )
        })
    });
    draw(cx);
    let confirm = cx.debug_bounds("dialog-confirm").expect("Delete task");
    cx.simulate_click(confirm.center(), gpui_kit::Modifiers::default());
    until(cx, &app, "the task gone", |app| {
        app.state.task(&review_id).is_none()
    });
}

#[gpui::test]
fn the_resolved_tab_lists_archived_tasks_with_restore(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.panel = Panel::Tasks;
            app.tasks_tab = TasksTab::Resolved;
            cx.notify();
        })
    });
    draw(cx);
    let toggle = cx
        .debug_bounds("archived-toggle")
        .expect("Show archived tasks");
    cx.simulate_click(toggle.center(), gpui_kit::Modifiers::default());
    until(cx, &app, "the archived list", |app| {
        app.convo.archived_open && !app.state.convo.archived.tasks.is_empty()
    });
    assert!(cx.debug_bounds("archived-restore").is_some());
    let first = app.read_with(cx, |app, _| app.state.convo.archived.tasks[0].id.clone());
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_convo(ConvoAction::RestoreTask(first.clone()), window, cx)
        })
    });
    until(cx, &app, "the restored task", |app| {
        app.state.task(&first).is_some()
    });
    app.read_with(cx, |app, _| {
        assert!(app.state.convo.archived.tasks.iter().all(|t| t.id != first));
    });
}

#[gpui::test]
fn the_task_thread_starts_with_the_brief_as_bubbles(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let task = app.read_with(cx, |app, _| {
        app.state
            .tasks
            .iter()
            .find(|t| t.channel_name == "sayso" && t.status.is_your_turn())
            .unwrap()
            .id
            .clone()
    });
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_thread(task.clone(), window, cx)));
    draw(cx);
    app.read_with(cx, |app, _| {
        assert!(app.thread_keys[0].starts_with("brief-"));
        let posts = app.state.thread(&task).unwrap().posts.len();
        assert_eq!(app.thread_keys.len(), posts + 1);
    });
}

#[gpui::test]
fn about_opens_from_the_settings_rail_and_shows_the_update(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(crate::app::SettingsSection::Appearance, cx);
            app.updates =
                consort_client::update::Updates::fixed(crate::ui::update_notice::ready_view());
        })
    });
    draw(cx);
    let row = cx.debug_bounds("settings-about").expect("About Consort");
    cx.simulate_click(row.center(), gpui_kit::Modifiers::default());
    draw(cx);
    assert_eq!(
        app.read_with(cx, |app, _| app.dialog.clone()),
        Some(Dialog::About)
    );
    assert!(
        cx.debug_bounds("about-restart").is_some(),
        "a ready update offers Restart to install"
    );
}

#[gpui::test]
fn the_composer_has_no_voice_button_and_dictation_has_a_hint(cx: &mut TestAppContext) {
    let (handle, _app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    assert!(cx.debug_bounds("composer").is_some());
    assert!(cx.debug_bounds("voice").is_none());
    if cfg!(target_os = "macos") {
        assert!(
            crate::convo::dictation_hint()
                .unwrap()
                .contains("Dictation")
        );
    }
    let _ = TimelineItem::Day;
}

#[gpui::test]
fn reconnect_of_a_form_service_loads_its_schemes_first(cx: &mut TestAppContext) {
    let mut state = mock();
    state.connector_catalog.services.clear();
    let mut todoist = state.connections[0].clone();
    todoist.id = "con_todoist".into();
    todoist.service = "todoist".into();
    todoist.service_name = "Todoist".into();
    state.connections.push(todoist);
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.reconnect("con_todoist".into(), window, cx)
        })
    });
    until(cx, &app, "the Connect form", |app| {
        app.dialog == Some(Dialog::Connect)
    });
    app.read_with(cx, |app, _| {
        assert_eq!(app.ext.connect.reconnect.as_deref(), Some("con_todoist"));
        assert!(!app.ext.connect.fields.is_empty(), "the API token field");
    });
}

#[gpui::test]
fn review_changes_shows_a_diff_of_the_new_skill_version(cx: &mut TestAppContext) {
    let (handle, app) = open(mock(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let skill = app.read_with(cx, |app, _| {
        app.state
            .skills
            .iter()
            .find(|s| matches!(s.update, consort_client::SkillUpdateView::Available { .. }))
            .unwrap()
            .id
            .clone()
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_dialog(Dialog::SkillDiff(skill.clone()), window, cx)
        })
    });
    until(cx, &app, "both versions", |app| {
        !app.convo.skill_diff.is_empty() && app.convo.skill_diff.iter().all(|(_, d)| d.is_some())
    });
    app.read_with(cx, |app, _| {
        let (_, diff) = app
            .convo
            .skill_diff
            .iter()
            .find(|(p, _)| p == "SKILL.md")
            .unwrap();
        let lines = diff.as_ref().unwrap().as_ref().unwrap();
        assert!(lines.iter().any(|l| matches!(
            l,
            consort_client::convo::DiffLine::Added { text, .. } if text.contains("linter")
        )));
    });
}
