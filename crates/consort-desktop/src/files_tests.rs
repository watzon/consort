//! Phase 1 in a headless window: the larger sidebar, the drop state of the
//! composer, the attachment chips and the send that waits for them, the
//! attachments in bubbles, the light box, the Channel panel with its Files
//! tab and its menu, the reply box, and the skill zip upload.

use std::time::Duration;

use crate::app::{ChannelTab, ConsortApp, Dialog, DropOver, Panel};
use crate::ui::menu::MenuAction;
use consort_client::{
    Client, TimelineItem, UploadStatus, UploadTarget, ViewState,
    mock_files::{LIGHTS, ROOF_TASK},
};
use gpui::{AnyWindowHandle, TestAppContext, VisualTestContext};
use gpui_kit::{
    AppContext as _, Entity, ExternalPaths, FileDropEvent, Modifiers, MouseButton, WindowOptions,
    point, px, size,
};

fn open(state: ViewState, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<ConsortApp>) {
    // The mock client runs its own thread.
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        consort_theme::init(cx);
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

/// Lets the mock client's own thread work for `ms`, then draws.
fn settle(cx: &mut VisualTestContext, ms: u64) {
    let end = std::time::Instant::now() + Duration::from_millis(ms);
    while std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(25));
        cx.run_until_parked();
    }
    draw(cx);
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
        if app.read_with(cx, |app, _| ready(app)) {
            return;
        }
        assert!(std::time::Instant::now() < end, "timed out: {what}");
        settle(cx, 50);
    }
}

fn files_state() -> ViewState {
    consort_client::mock::view_state_with_files()
}

fn open_lights(app: &Entity<ConsortApp>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_channel(LIGHTS.into(), window, cx)));
    draw(cx);
}

fn bounds(cx: &mut VisualTestContext, selector: &str) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    let name: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    cx.debug_bounds(name)
        .unwrap_or_else(|| panic!("{selector} draws"))
}

fn has(cx: &mut VisualTestContext, selector: &str) -> bool {
    let name: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    cx.debug_bounds(name).is_some()
}

fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("consort-desktop-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the temp folder");
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("the temp file");
    path
}

/// The sidebar of "Base: Tutti with top tint and larger sidebar": 288 px,
/// 36 px rows, 30 px worker rows, a 28 px avatar. The settings rail uses
/// the same rows.
#[gpui::test]
fn the_sidebar_and_the_settings_rail_use_the_larger_scale(cx: &mut TestAppContext) {
    let (handle, app) = open(consort_client::mock::view_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let (sayso, worker) = app.read_with(cx, |app, _| {
        let sayso = app
            .state
            .channels
            .iter()
            .find(|c| c.name == "sayso")
            .map(|c| c.id.clone())
            .expect("sayso");
        (sayso, app.state.workers[0].id.clone())
    });
    assert_eq!(bounds(cx, "sidebar").size.width, px(288.));
    assert_eq!(bounds(cx, "sidebar-search").size.height, px(36.));
    assert_eq!(
        bounds(cx, &format!("sidebar-channel-{sayso}")).size.height,
        px(36.)
    );
    assert_eq!(
        bounds(cx, &format!("sidebar-worker-{worker}")).size.height,
        px(30.)
    );
    assert_eq!(bounds(cx, "sidebar-avatar").size, size(px(28.), px(28.)));
    // A group label is 30 px: 13 px text on an 18 px line and its padding.
    assert_eq!(bounds(cx, "sidebar-group-label").size.height, px(30.));
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.open_settings(crate::app::SettingsSection::Appearance, cx)
        })
    });
    draw(cx);
    assert_eq!(bounds(cx, "settings-Appearance").size.height, px(36.));
    assert_eq!(bounds(cx, "settings-Skills").size.width, px(267.));
}

/// Files dragged anywhere over the chat make the composer the drop zone: it
/// grows and says how many files attach. A release attaches them.
#[gpui::test]
fn a_drop_over_the_chat_attaches_to_the_composer(cx: &mut TestAppContext) {
    let (handle, app) = open(files_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    let before = bounds(cx, "composer");
    let file = temp_file("porch-plan.pdf", b"a plan");
    // Over the timeline, far from the composer.
    let over = point(px(700.), px(300.));
    cx.simulate_event(FileDropEvent::Entered {
        position: over,
        paths: ExternalPaths(vec![file.clone()].into()),
    });
    cx.simulate_event(FileDropEvent::Pending {
        position: point(px(701.), px(301.)),
    });
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.drop_hover, Some((DropOver::Chat, 1)))
    });
    assert!(has(cx, "drop-zone"), "the composer shows the drop state");
    let during = bounds(cx, "composer");
    assert!(
        during.size.height > before.size.height,
        "the composer grows"
    );
    cx.simulate_event(FileDropEvent::Submit {
        position: point(px(701.), px(301.)),
    });
    draw(cx);
    until(cx, &app, "the dropped file is attached", |app| {
        app.state.uploads.iter().any(|u| {
            u.name == "porch-plan.pdf"
                && u.target
                    == Some(UploadTarget::Composer {
                        channel_id: LIGHTS.into(),
                    })
        })
    });
    app.read_with(cx, |app, _| assert_eq!(app.drop_hover, None));
    assert!(!has(cx, "drop-zone"));
}

/// The chips of "Composer with attachments": pictures as thumbnails, files
/// with their type and size, an upload with its share, a failed upload with
/// Try again. A failed upload stops the send and keeps the text.
#[gpui::test]
fn the_chips_show_each_state_and_a_failed_upload_stops_the_send(cx: &mut TestAppContext) {
    let mut state = files_state();
    state.uploads = consort_client::mock_files::composer_uploads();
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    let image = bounds(cx, "chip-upl_mock_1");
    assert_eq!(image.size, size(px(56.), px(56.)));
    let file = bounds(cx, "chip-upl_mock_3");
    assert_eq!(file.size, size(px(240.), px(56.)));
    assert!(has(cx, "chip-upl_mock_4"), "the upload that runs");
    let failed = bounds(cx, "chip-upl_mock_5");
    assert!(
        failed.size.width > px(240.),
        "the failed chip fits its text"
    );
    let composer = bounds(cx, "composer");
    let chips = bounds(cx, "upload-chips");
    assert!(
        composer.contains(&chips.center()),
        "the chips are in the field"
    );

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.inputs.composer.update(cx, |input, cx| {
                input.set_value("With the photos", window, cx)
            });
            app.send_message(window, cx);
        })
    });
    settle(cx, 300);
    app.read_with(cx, |app, cx| {
        assert_eq!(
            app.inputs.composer.read(cx).value().to_string(),
            "With the photos",
            "the text stays"
        );
        assert_eq!(app.state.uploads.len(), 5, "the chips stay");
        let error = app.notices.current().map(|n| n.message.clone());
        assert!(
            error
                .as_deref()
                .is_some_and(|e| e.contains("gutter-detail.heic")),
            "{error:?}"
        );
    });
}

/// A send waits for an upload that still runs, then carries every file. The
/// chips leave the composer.
#[gpui::test]
fn a_send_waits_for_its_uploads_then_carries_them(cx: &mut TestAppContext) {
    let mut state = files_state();
    state.uploads = consort_client::mock_files::composer_uploads()
        .into_iter()
        .filter(|u| u.status == UploadStatus::Ready)
        .collect();
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    // A new file starts to upload.
    let file = temp_file("gutter.pdf", b"gutter");
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.attach_paths(
                UploadTarget::Composer {
                    channel_id: LIGHTS.into(),
                },
                &[file],
                cx,
            )
        })
    });
    until(cx, &app, "the new upload shows", |app| {
        app.state.uploads.len() == 4
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.inputs
                .composer
                .update(cx, |input, cx| input.set_value("Here they are", window, cx));
            app.send_message(window, cx);
        })
    });
    until(cx, &app, "the message with four files", |app| {
        app.state.timeline(LIGHTS).is_some_and(|t| {
            t.items.iter().any(|i| {
                matches!(i, TimelineItem::Message(m)
                    if m.body == "Here they are" && m.attachments.len() == 4)
            })
        })
    });
    settle(cx, 100);
    app.read_with(cx, |app, cx| {
        assert!(app.state.uploads.is_empty());
        assert_eq!(app.inputs.composer.read(cx).value().to_string(), "");
    });
    assert!(!has(cx, "upload-chips"));
}

/// "Bubbles with attachments": your pictures as tiles above your text, your
/// file as a file bubble, and the assistant's artifact names its task.
#[gpui::test]
fn bubbles_show_pictures_files_and_artifacts(cx: &mut TestAppContext) {
    let (handle, app) = open(files_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    let front = bounds(cx, "image-msg_lights_photos-0");
    let side = bounds(cx, "image-msg_lights_photos-1");
    assert_eq!(front.size, size(px(228.), px(168.)));
    assert_eq!(side.origin.y, front.origin.y, "two in a row");
    let quote = bounds(cx, "file-msg_lights_photos-2");
    assert_eq!(quote.size.width, px(280.));
    assert!(
        quote.origin.y > front.bottom(),
        "the file is under the pictures"
    );
    assert_eq!(quote.right(), side.right(), "in your lane, at the right");
    let artifact = bounds(cx, "file-msg_lights_compare-0");
    assert_eq!(artifact.size.width, px(360.));
    assert!(artifact.origin.x < quote.origin.x, "in the assistant lane");
    assert!(
        has(cx, "file-msg_lights_compare-0-task"),
        "it names its task"
    );
    // Right click: Open, Save, and no Save to channel files for an artifact.
    cx.simulate_mouse_down(artifact.center(), MouseButton::Right, Modifiers::default());
    draw(cx);
    app.read_with(cx, |app, _| {
        let labels: Vec<_> = app
            .menu
            .as_ref()
            .expect("the menu")
            .items
            .iter()
            .map(|i| i.label.to_string())
            .collect();
        assert_eq!(labels, ["Open", "Save"]);
    });
}

/// The light box opens on a picture, steps with the arrows and the keys,
/// stops at the ends, and closes with Escape.
#[gpui::test]
fn the_light_box_opens_steps_and_closes(cx: &mut TestAppContext) {
    let (handle, app) = open(files_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    let front = bounds(cx, "image-msg_lights_photos-0");
    cx.simulate_click(front.center(), Modifiers::default());
    draw(cx);
    let lightbox = bounds(cx, "lightbox");
    // The light box covers the app but not the title bar of Windows and
    // Linux, so the window can always move and close.
    let bar = cx.update(|window, cx| {
        if crate::ui::window_controls::has_controls(window) {
            crate::ui::window_controls::bar_height(&ConsortApp::tokens(cx))
        } else {
            px(0.)
        }
    });
    assert_eq!(lightbox.size, size(px(1440.), px(900.) - bar));
    assert!(has(cx, "lightbox-image") && has(cx, "lightbox-count"));
    app.read_with(cx, |app, _| {
        let lb = app.lightbox.as_ref().expect("open");
        assert_eq!((lb.index, lb.images.len()), (0, 2));
    });
    let next = bounds(cx, "lightbox-next");
    cx.simulate_click(next.center(), Modifiers::default());
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(app.lightbox.as_ref().map(|l| l.index), Some(1))
    });
    cx.simulate_keystrokes("right");
    app.read_with(cx, |app, _| {
        assert_eq!(app.lightbox.as_ref().map(|l| l.index), Some(1), "the last")
    });
    cx.simulate_keystrokes("left");
    app.read_with(cx, |app, _| {
        assert_eq!(app.lightbox.as_ref().map(|l| l.index), Some(0))
    });
    cx.simulate_keystrokes("escape");
    draw(cx);
    app.read_with(cx, |app, _| assert!(app.lightbox.is_none()));
    assert!(!has(cx, "lightbox"));
}

/// The Channel panel: it opens from `#`, takes the place of the Tasks
/// panel, lists folders, files and the artifacts of each task, opens a
/// folder, and its row menu deletes a file after a confirm step. Routines
/// and Memory wait for phase 2.
#[gpui::test]
fn the_channel_panel_lists_files_and_deletes_from_its_menu(cx: &mut TestAppContext) {
    let (handle, app) = open(files_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    let button = bounds(cx, "channel-button");
    cx.simulate_click(button.center(), Modifiers::default());
    draw(cx);
    app.read_with(cx, |app, _| assert_eq!(app.panel, Panel::Channel));
    assert_eq!(bounds(cx, "channel-panel").size.width, px(400.));
    assert!(has(cx, "files-summary") && has(cx, "files-upload"));
    assert!(has(cx, "folder-photos") && has(cx, "folder-quotes"));
    assert!(has(cx, "file-row-wiring-plan.pdf"));
    assert!(has(cx, &format!("artifact-task-{ROOF_TASK}")));
    let photos = bounds(cx, "folder-photos");
    let wiring = bounds(cx, "file-row-wiring-plan.pdf");
    let artifacts = bounds(cx, &format!("artifact-task-{ROOF_TASK}"));
    assert!(photos.origin.y < wiring.origin.y && wiring.origin.y < artifacts.origin.y);

    // Routines waits for phase 2: its tab takes no click.
    let routines = bounds(cx, "channel-tab-Routines");
    cx.simulate_click(routines.center(), Modifiers::default());
    draw(cx);
    app.read_with(cx, |app, _| assert_eq!(app.channel_tab, ChannelTab::Files));

    // A folder opens in place, with a way back.
    cx.simulate_click(photos.center(), Modifiers::default());
    draw(cx);
    app.read_with(cx, |app, _| assert_eq!(app.files_folder, "photos"));
    assert!(has(cx, "file-row-photos/roofline-front.jpg"));
    assert!(has(cx, "files-folder"));
    cx.update(|_, cx| {
        app.update(cx, |app, _| app.files_folder.clear());
    });
    draw(cx);

    // The row menu: Open, Download, Rename, Move, Delete.
    let wiring = bounds(cx, "file-row-wiring-plan.pdf");
    cx.simulate_mouse_down(wiring.center(), MouseButton::Right, Modifiers::default());
    draw(cx);
    let delete = app.read_with(cx, |app, _| {
        let menu = app.menu.as_ref().expect("the row menu");
        let labels: Vec<_> = menu.items.iter().map(|i| i.label.to_string()).collect();
        assert_eq!(labels, ["Open", "Download", "Rename", "Move", "Delete"]);
        menu.items[4].action.clone()
    });
    cx.update(|window, cx| app.update(cx, |app, cx| app.run_menu(delete, window, cx)));
    draw(cx);
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.dialog,
            Some(Dialog::ConfirmDeleteFile(
                LIGHTS.into(),
                "wiring-plan.pdf".into()
            ))
        )
    });
    let confirm = bounds(cx, "dialog-confirm");
    cx.simulate_click(confirm.center(), Modifiers::default());
    until(cx, &app, "the file is gone", |app| {
        !app.state
            .channel_files
            .iter()
            .any(|f| f.path == "wiring-plan.pdf")
            && app.dialog.is_none()
    });

    // Tasks takes the right side; the Channel panel closes.
    let tasks = bounds(cx, "tasks-button");
    cx.simulate_click(tasks.center(), Modifiers::default());
    draw(cx);
    app.read_with(cx, |app, _| assert_eq!(app.panel, Panel::Tasks));
    assert!(!has(cx, "channel-panel"));
}

/// Rename and Move in the row menu change the path of the file.
#[gpui::test]
fn rename_and_move_change_the_path(cx: &mut TestAppContext) {
    let (handle, app) = open(files_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_menu(
                MenuAction::RenameFile(LIGHTS.into(), "wiring-plan.pdf".into()),
                window,
                cx,
            );
            app.set_field(crate::app::Field::Name, "wiring.pdf", window, cx);
        })
    });
    draw(cx);
    let confirm = bounds(cx, "dialog-confirm");
    cx.simulate_click(confirm.center(), Modifiers::default());
    until(cx, &app, "the renamed file", |app| {
        app.state
            .channel_files
            .iter()
            .any(|f| f.path == "wiring.pdf")
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.run_menu(
                MenuAction::MoveFile(LIGHTS.into(), "wiring.pdf".into()),
                window,
                cx,
            );
        })
    });
    draw(cx);
    let quotes = bounds(cx, "move-to-quotes");
    cx.simulate_click(quotes.center(), Modifiers::default());
    draw(cx);
    let confirm = bounds(cx, "dialog-confirm");
    cx.simulate_click(confirm.center(), Modifiers::default());
    until(cx, &app, "the moved file", |app| {
        app.state
            .channel_files
            .iter()
            .any(|f| f.path == "quotes/wiring.pdf")
    });
}

/// The reply box of a task thread has the add button, the chips, and a
/// reply that carries the file.
#[gpui::test]
fn a_reply_in_a_task_thread_carries_its_file(cx: &mut TestAppContext) {
    let mut state = files_state();
    state.uploads = vec![consort_client::mock_files::reply_upload()];
    let (handle, app) = open(state, cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    open_lights(&app, cx);
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_thread(ROOF_TASK.into(), window, cx)));
    draw(cx);
    let reply = bounds(cx, "reply-box");
    let chip = bounds(cx, "chip-upl_mock_reply");
    assert!(
        reply.contains(&chip.center()),
        "the chip is in the reply box"
    );
    assert!(has(cx, "reply-attach"));
    // The add menu opens with its two items.
    let add = bounds(cx, "reply-attach");
    cx.simulate_click(add.center(), Modifiers::default());
    draw(cx);
    app.read_with(cx, |app, _| {
        let labels: Vec<_> = app
            .menu
            .as_ref()
            .expect("the add menu")
            .items
            .iter()
            .map(|i| i.label.to_string())
            .collect();
        assert_eq!(
            labels,
            ["Upload from this computer", "Choose from channel files"]
        );
    });
    cx.update(|_, cx| app.update(cx, |app, cx| app.close_menu(cx)));
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.inputs.reply.update(cx, |input, cx| {
                input.set_value("Add the garage side too.", window, cx)
            });
            app.send_reply(window, cx);
        })
    });
    until(cx, &app, "the reply with its picture", |app| {
        app.state.thread(ROOF_TASK).is_some_and(|t| {
            t.posts.last().is_some_and(|p| {
                p.bodies.iter().any(|b| {
                    matches!(b, consort_client::PostBody::Attachments(list)
                        if list.iter().any(|a| a.name == "garage-side.jpg"))
                })
            })
        })
    });
}

/// Add skill > Upload checks the zip, and a file that is not a zip says
/// what to do.
#[gpui::test]
fn the_add_skill_dialog_checks_an_uploaded_zip(cx: &mut TestAppContext) {
    let (handle, app) = open(files_state(), cx);
    let cx = &mut VisualTestContext::from_window(handle, cx);
    let zip = consort_client::mock_files::skill_zip_path().expect("the mock zip");
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_dialog(Dialog::AddSkill, window, cx);
            app.ext.add_skill.tab = crate::extensions::AddSkillTab::Upload;
            app.ext.add_skill.upload_path = Some(zip.clone());
            app.preview_skill(
                consort_client::SkillInstallInput::Upload { file_path: zip },
                "upload".into(),
                cx,
            );
        })
    });
    until(cx, &app, "the checked zip", |app| {
        app.ext.add_skill.preview.is_some()
    });
    app.read_with(cx, |app, _| {
        let (_, preview) = app.ext.add_skill.preview.as_ref().expect("checked");
        assert_eq!(preview.name, "porch-lights-skill");
    });
    let text = temp_file("notes.txt", b"not a zip");
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.ext.add_skill.preview = None;
            app.preview_skill(
                consort_client::SkillInstallInput::Upload {
                    file_path: text.display().to_string(),
                },
                "upload".into(),
                cx,
            );
        })
    });
    until(cx, &app, "the error", |app| app.dialog_error.is_some());
    app.read_with(cx, |app, _| {
        let error = app.dialog_error.clone().unwrap_or_default();
        assert!(error.contains("not a zip file"), "{error}");
    });
}
