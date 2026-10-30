//! The Consort desktop app.
//!
//! `consort-desktop` connects to the paired hub, or shows onboarding when no
//! hub is paired. `consort-desktop --mock` shows the designs' content with no
//! hub.

// A release build on Windows is a window app, with no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod admin;
#[cfg(test)]
mod admin_tests;
mod app;
mod assets;
#[cfg(test)]
mod chat_tests;
mod countdown;
mod drafts;
mod edit_keys;
#[cfg(test)]
mod ext_tests;
mod extensions;
#[cfg(test)]
mod files_tests;
mod fonts;
#[cfg(test)]
mod keys_tests;
mod knowledge;
#[cfg(test)]
mod knowledge_tests;
mod notice;
mod notify_platform;
#[cfg(test)]
mod offline_tests;
#[cfg(test)]
mod phase5_tests;
mod read_marker;
mod ui;
mod waiting;

use consort_client::Client;
use gpui_kit::{
    AppContext as _, Bounds, Menu, MenuItem, TitlebarOptions, WindowBounds, WindowOptions, actions,
    point, size,
};

actions!(consort, [Quit]);

/// Command line options. `--screen` opens one of the designed screens, for
/// screenshots and reviews: tutti, sayso, tasks, thread, settings,
/// workers, add-account, add-account-name, sign-in, channel-tags,
/// new-channel, add-worker, worker-added, review, failed, error, new-folder,
/// onboarding, connectors, add-connector, connect, connectors-in-channel,
/// channel-connectors, skills, add-skill, add-skill-unsupported,
/// account-settings, hub-cards, tutti-manages, instructions, mark-idle,
/// mark-working, mark-needs-you, mark-done. Phase 1: top-tint,
/// attachments, drop, bubbles, lightbox, files, files-menu, files-drop,
/// files-empty, thread-attach and add-skill-upload. Phase 3: hub, clients,
/// clients-revoke, pair-client and notifications. Phase 5: add-worker,
/// add-worker-install, worker-added, update-ready and hub-offline. The old names `everything` and
/// files-empty, thread-attach and add-skill-upload. Phase 2: routines,
/// routine-runs, routine-new, routine-edit, schedule-picker,
/// settings-routines, memory, memory-conflict, settings-memory,
/// memory-history, memory-deleted, routines-empty, memory-empty,
/// schedule-interval, schedule-once, schedule-once-past and
/// schedule-cron. The old names `everything` and
/// `everything-manages` still work. `--light` and `--dark` set
/// the mode, and `--theme <name>` the theme.
#[derive(Clone, Debug, Default)]
pub struct Args {
    pub mock: bool,
    pub screen: Option<String>,
    pub appearance: Option<consort_theme::Appearance>,
    pub theme: Option<String>,
}

/// The current name of a screen. The main channel was called Everything, so
/// `everything` and `everything-manages` still open Tutti's screens.
fn canonical_screen(name: &str) -> String {
    match name {
        "everything" => "tutti".to_owned(),
        "everything-manages" => "tutti-manages".to_owned(),
        other => other.to_owned(),
    }
}

fn parse_args() -> Args {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--mock" => args.mock = true,
            "--screen" => args.screen = it.next().map(|name| canonical_screen(&name)),
            "--theme" => args.theme = it.next(),
            "--light" => args.appearance = Some(consort_theme::Appearance::Light),
            "--dark" => args.appearance = Some(consort_theme::Appearance::Dark),
            other => {
                eprintln!(
                    "consort: unknown option {other}. Use --mock, --screen <name>, --light, --dark or --theme <name>."
                );
                std::process::exit(2);
            }
        }
    }
    args
}

fn main() {
    let args = parse_args();
    let mock = args.mock;
    let client = if mock
        && matches!(
            args.screen.as_deref(),
            Some("channel-requests" | "new-folder")
        ) {
        Client::mock_with(consort_client::mock::view_state_with_requests())
    } else if mock && args.screen.as_deref() == Some("worker-added") {
        Client::mock_with(consort_client::mock::view_state_with_long_detail())
    } else if mock && args.screen.as_deref() == Some("tutti-manages") {
        Client::mock_with(consort_client::mock::view_state_with_connect_card())
    } else if mock && args.screen.as_deref() == Some("add-skill-unsupported") {
        // The hub answers `unsupported` for the skill directories (step 3).
        let mut state = consort_client::mock::view_state();
        state.skill_directory.load = consort_client::LoadState::Unsupported(
            "This hub cannot search skill directories yet. Update the hub, then try again.".into(),
        );
        Client::mock_with(state)
    } else if mock && args.screen.as_deref() == Some("model-picker-no-account") {
        // No account is signed in on any worker.
        let mut state = consort_client::mock::view_state();
        state.model_groups.clear();
        Client::mock_with(state)
    } else if mock
        && let Some(state) = args
            .screen
            .as_deref()
            .and_then(consort_client::mock_files::screen_state)
    {
        // The phase 1 screens: files and attachments in house-lights.
        Client::mock_with(state)
    } else if mock
        && let Some(state) = args
            .screen
            .as_deref()
            .and_then(consort_client::mock_routines::screen_state)
    {
        // The phase 2 screens: routines and memory.
        Client::mock_with(state)
    } else if mock
        && let Some(state) = args
            .screen
            .as_deref()
            .and_then(consort_client::mock_offline::screen_state)
    {
        // The hub is offline at start: saved data, a queued message.
        Client::mock_with(state)
    } else if mock {
        Client::mock()
    } else {
        Client::start()
    };
    // A client that cannot start still gets a window, which says what
    // happened and what to do next. The error also goes to stderr.
    if let Err(err) = &client {
        eprintln!("consort: {err}");
    }
    gpui_kit::application()
        .with_assets(assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            fonts::register(cx);
            consort_theme::init(cx);
            consort_theme::watch_user_themes(cx);
            // Reduce motion starts from the user's choice, else from the
            // system setting where the app can read it, else off.
            let saved = consort_theme::ThemeLibrary::global(cx).reduce_motion;
            cx.set_reduce_motion(consort_theme::start_reduce_motion(
                saved,
                ui::motion::system_reduce_motion(),
            ));
            // The identity of the app's notifications (the AppUserModelID
            // on Windows).
            cx.set_app_identity("dev.consort.desktop", "Consort");
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.bind_keys([gpui_kit::KeyBinding::new("cmd-q", Quit, None)]);
            cx.set_menus(vec![
                Menu::new("Consort").items([MenuItem::action("Quit Consort", Quit)]),
            ]);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
            let tokens = consort_theme::ActiveTokens::tokens(cx).clone();
            let (min_w, min_h) = tokens.layout.min_window;
            let (width, height) = tokens.layout.default_window;
            let bounds = Bounds::centered(None, size(width, height), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(min_w, min_h)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Consort".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(tokens.space.lg, tokens.space.lg)),
                }),
                ..WindowOptions::default()
            };
            let result = match client {
                Ok(client) => gpui_kit::open_window(options, cx, move |window, cx| {
                    if args.appearance.is_some() || args.theme.is_some() {
                        consort_theme::choose_without_saving(
                            args.theme.clone().map(|t| {
                                let mut c = t.chars();
                                c.next()
                                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                                    .unwrap_or_default()
                                    .into()
                            }),
                            args.appearance,
                            None,
                            window,
                            cx,
                        );
                    }
                    cx.new(|cx| {
                        let mut app = app::ConsortApp::new(client, mock, window, cx);
                        if let Some(screen) = &args.screen {
                            app.open_screen(screen, window, cx);
                        }
                        app
                    })
                })
                .map(|_| ()),
                Err(err) => gpui_kit::open_window(options, cx, move |_, cx| {
                    cx.new(|_| ui::startup_error::StartupError::new(err.to_string()))
                })
                .map(|_| ()),
            };
            if let Err(err) = result {
                eprintln!("consort: cannot open the window: {err}");
                cx.quit();
            }
        });
}

#[cfg(test)]
mod screen_name_tests {
    use super::canonical_screen;

    #[test]
    fn the_old_screen_names_open_the_tutti_screens() {
        assert_eq!(canonical_screen("everything"), "tutti");
        assert_eq!(canonical_screen("everything-manages"), "tutti-manages");
        assert_eq!(canonical_screen("tutti"), "tutti");
        assert_eq!(canonical_screen("skills"), "skills");
    }
}
