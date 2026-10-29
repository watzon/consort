//! The Consort desktop app.
//!
//! `consort-desktop` connects to the paired hub, or shows onboarding when no
//! hub is paired. `consort-desktop --mock` shows the designs' content with no
//! hub.

mod app;
mod assets;
mod fonts;
mod ui;

use consort_client::Client;
use gpui_kit::{
    AppContext as _, Bounds, Menu, MenuItem, TitlebarOptions, WindowBounds, WindowOptions, actions,
    point, px, size,
};

actions!(consort, [Quit]);

/// Command line options. `--screen` opens one of the designed screens, for
/// screenshots and reviews: everything, sayso, tasks, thread, settings,
/// workers, new-channel, add-worker, onboarding. `--light` and `--dark` set
/// the mode, and `--theme <name>` the theme.
#[derive(Clone, Debug, Default)]
pub struct Args {
    pub mock: bool,
    pub screen: Option<String>,
    pub appearance: Option<consort_theme::Appearance>,
    pub theme: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--mock" => args.mock = true,
            "--screen" => args.screen = it.next(),
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
    let client = if mock {
        Client::mock()
    } else {
        Client::start()
    };
    let client = match client {
        Ok(client) => client,
        Err(err) => {
            eprintln!("consort: {err}");
            std::process::exit(1);
        }
    };
    gpui_kit::application()
        .with_assets(assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            fonts::register(cx);
            consort_theme::init(cx);
            consort_theme::watch_user_themes(cx);
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
            let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
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
            let result = gpui_kit::open_window(options, cx, move |window, cx| {
                if args.appearance.is_some() || args.theme.is_some() {
                    consort_theme::choose(
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
            });
            if let Err(err) = result {
                eprintln!("consort: cannot open the window: {err}");
                cx.quit();
            }
        });
}
