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
#[cfg(test)]
mod approvals_tests;
mod assets;
#[cfg(test)]
mod assistant_status_tests;
#[cfg(test)]
mod chat_tests;
#[cfg(test)]
mod command_check_tests;
#[cfg(test)]
mod composer_gate_tests;
#[cfg(test)]
mod composer_tests;
mod convo;
#[cfg(test)]
mod convo_tests;
mod countdown;
#[cfg(test)]
mod cover_tests;
mod dictation;
#[cfg(test)]
mod dictation_tests;
#[cfg(test)]
mod document_tests;
mod drafts;
#[cfg(test)]
mod drafts_tests;
mod edit_keys;
#[cfg(test)]
mod errors_tests;
#[cfg(test)]
mod ext_tests;
mod extensions;
mod fallback;
#[cfg(test)]
mod fallback_tests;
#[cfg(test)]
mod files_tests;
#[cfg(test)]
mod first_run_tests;
mod fonts;
#[cfg(test)]
mod icon_picker_tests;
#[cfg(test)]
mod keys_tests;
mod knowledge;
#[cfg(test)]
mod knowledge_tests;
mod layout;
#[cfg(test)]
mod layout_tests;
#[cfg(test)]
mod lightbox_tests;
#[cfg(test)]
mod local_hub_tests;
mod logging;
mod monid;
#[cfg(test)]
mod monid_tests;
mod notice;
#[cfg(test)]
mod notice_row_tests;
#[cfg(test)]
mod notices_tests;
mod notify_platform;
#[cfg(test)]
mod offline_tests;
mod permissions;
#[cfg(test)]
mod permissions_tests;
#[cfg(test)]
mod phase5_tests;
mod proactivity;
#[cfg(test)]
mod proactivity_tests;
#[cfg(test)]
mod reach_tests;
mod read_marker;
#[cfg(test)]
mod setup_tests;
#[cfg(test)]
mod tasks_tests;
#[cfg(test)]
mod test_support;
mod ui;
#[cfg(test)]
mod versions_tests;
mod visual;
#[cfg(test)]
mod visual_tests;
mod voice;
#[cfg(test)]
mod voice_tests;
mod waiting;
#[cfg(test)]
mod worker_limits_tests;

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
/// mark-working, mark-needs-you, mark-done. Monid: monid-cards,
/// monid-calls, monid-panel, monid-view, monid-settings, monid-add,
/// monid-offer, monid-routine, monid-full-access, monid-access and
/// monid-task. The composer: composer-grown and
/// composer-full. Phase 1: top-tint,
/// attachments, drop, bubbles, lightbox, files, files-menu, files-drop,
/// files-empty, thread-attach and add-skill-upload. Phase 3: hub, clients,
/// clients-revoke, pair-client and notifications. The icon picker: icon-picker,
/// icon-picker-search and icon-picked. Phase 5: add-worker,
/// add-worker-install, worker-added, update-ready and hub-offline. The old names `everything` and
/// files-empty, thread-attach and add-skill-upload. Phase 2: routines,
/// routine-runs, routine-new, routine-edit, schedule-picker,
/// settings-routines, memory, memory-conflict, settings-memory,
/// memory-history, memory-deleted, routines-empty, memory-empty,
/// schedule-interval, schedule-once, schedule-once-past and
/// schedule-cron. Versions: hub-too-old, app-too-old, update-held,
/// hub-update, pair-too-old and workers-updating. Reach: reach-off, reach-on,
/// reach-states, reach-unreachable, pair-client-loopback and
/// add-worker-loopback. A hub on this computer: local-install, local-windows,
/// local-bubblewrap, local-failed, local-found, local-stopped, hub-local,
/// hub-local-remove, hub-local-move, hub-local-connect and restart-tasks. The
/// setup: every name in `consort_client::mock_local::SETUP_SCREENS`. The
/// old names `everything`
/// and `everything-manages` still work. `--light` and `--dark` set
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
    logging::init();
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
            .and_then(consort_client::mock_monid::screen_state)
    {
        // The Monid screens that start with no key on the hub.
        Client::mock_with(state)
    } else if mock
        && let Some(state) = args
            .screen
            .as_deref()
            .and_then(consort_client::mock_offline::screen_state)
    {
        // The hub is offline at start: saved data, a queued message.
        Client::mock_with(state)
    } else if mock
        && let Some(state) = args
            .screen
            .as_deref()
            .and_then(consort_client::mock_proactivity::screen_state)
    {
        // The phase 4b screens: the assistant on its own.
        Client::mock_with(state)
    } else if mock
        && let Some(state) = args
            .screen
            .as_deref()
            .and_then(consort_client::mock_voice::screen_state)
    {
        // The phase 4c screens: write in my voice.
        Client::mock_with(state)
    } else if mock
        && let Some(state) = args
            .screen
            .as_deref()
            .and_then(consort_client::mock_reach::screen_state)
    {
        // The reach screens: how other devices get to the hub.
        Client::mock_with(state)
    } else if mock
        && let Some(state) = args
            .screen
            .as_deref()
            .and_then(consort_client::mock_first::screen_state)
    {
        // The first conversation in Tutti.
        Client::mock_with(state)
    } else if mock
        && let Some(state) = args
            .screen
            .as_deref()
            .and_then(consort_client::mock_versions::screen_state)
    {
        // The version screens: a hub or an app that is too old.
        Client::mock_with(state)
    } else if mock
        && let Some(state) = args
            .screen
            .as_deref()
            .and_then(consort_client::mock_local::screen_state)
    {
        // A hub on this computer: its install, its settings and its restart.
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
            convo::bind_keys(cx);
            cx.bind_keys([gpui_kit::KeyBinding::new("cmd-q", Quit, None)]);
            cx.set_menus(vec![Menu::new("Consort").items([
                MenuItem::action("About Consort", convo::ShowAbout),
                MenuItem::separator(),
                MenuItem::action("Quit Consort", Quit),
            ])]);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            // For a look at the app in the background. A window that comes
            // forward takes the keyboard from the app that the user types in.
            if std::env::var_os("CONSORT_NO_ACTIVATE").is_none() {
                cx.activate(true);
            }
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
