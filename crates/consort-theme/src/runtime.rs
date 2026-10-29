//! Loads themes into GPUI Kit and keeps the [`Tokens`] global in step with
//! the chosen theme, mode and text size.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use gpui_kit::{
    App, Global, SharedString, Window, WindowAppearance,
    component::{Theme, ThemeConfig, ThemeMode, ThemeRegistry},
};

use crate::{
    file::{Mode, ThemeError, ThemeFile, load_theme},
    mapping::{kit_theme_json, kit_theme_name},
    tokens::{TextSize, Tokens},
};

/// The Mode setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Appearance {
    /// Match system.
    #[default]
    System,
    Light,
    Dark,
}

/// Where a theme came from.
#[derive(Clone, Debug, PartialEq)]
pub enum ThemeSource {
    BuiltIn,
    User(PathBuf),
}

/// A theme that passed every check.
#[derive(Clone, Debug)]
pub struct LoadedTheme {
    pub file: ThemeFile,
    pub source: ThemeSource,
}

/// A user theme file that did not load, and why.
#[derive(Clone, Debug)]
pub struct RejectedTheme {
    pub file_name: String,
    pub reason: String,
}

/// The themes the app knows, the user's choice, and the files that failed.
#[derive(Clone, Debug)]
pub struct ThemeLibrary {
    pub themes: Vec<LoadedTheme>,
    pub rejected: Vec<RejectedTheme>,
    pub theme: SharedString,
    pub appearance: Appearance,
    pub text_size: TextSize,
    user_dir: Option<PathBuf>,
    user_stamp: Vec<(PathBuf, Option<SystemTime>)>,
}

impl Global for ThemeLibrary {}

/// The built-in themes: Spruce (the default) and Velvet.
pub fn builtin_theme_sources() -> [&'static str; 2] {
    [
        include_str!("../../../design/themes/spruce.json"),
        include_str!("../../../design/themes/velvet.json"),
    ]
}

/// The user themes folder: `<config dir>/consort/themes`.
pub fn user_themes_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.config_dir().join("consort").join("themes"))
}

/// Reads every `.json` file in `dir`. Files that fail a check are returned
/// with the reason, so the app can show them.
pub fn read_user_themes(dir: &Path) -> (Vec<LoadedTheme>, Vec<RejectedTheme>) {
    let mut loaded = Vec::new();
    let mut rejected = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (loaded, rejected);
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    paths.sort();
    for path in paths {
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let result = std::fs::read_to_string(&path)
            .map_err(|err| ThemeError::Io {
                path: path.clone(),
                message: err.to_string(),
            })
            .and_then(|text| load_theme(&text));
        match result {
            Ok(file) => loaded.push(LoadedTheme {
                file,
                source: ThemeSource::User(path),
            }),
            Err(err) => rejected.push(RejectedTheme {
                file_name,
                reason: err.to_string(),
            }),
        }
    }
    (loaded, rejected)
}

fn dir_stamp(dir: &Path) -> Vec<(PathBuf, Option<SystemTime>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut stamp: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| {
            let modified = e.metadata().ok().and_then(|m| m.modified().ok());
            (e.path(), modified)
        })
        .collect();
    stamp.sort();
    stamp
}

impl ThemeLibrary {
    fn load(user_dir: Option<PathBuf>) -> ThemeLibrary {
        let mut themes: Vec<LoadedTheme> = crate::builtin_themes()
            .into_iter()
            .map(|file| LoadedTheme {
                file,
                source: ThemeSource::BuiltIn,
            })
            .collect();
        let mut rejected = Vec::new();
        let mut user_stamp = Vec::new();
        if let Some(dir) = &user_dir {
            let (user, bad) = read_user_themes(dir);
            for theme in user {
                if themes.iter().any(|t| t.file.name == theme.file.name) {
                    rejected.push(RejectedTheme {
                        file_name: match &theme.source {
                            ThemeSource::User(p) => p
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            ThemeSource::BuiltIn => String::new(),
                        },
                        reason: format!(
                            "A theme named {} is already loaded. Give this theme another name",
                            theme.file.name
                        ),
                    });
                } else {
                    themes.push(theme);
                }
            }
            rejected.extend(bad);
            user_stamp = dir_stamp(dir);
        }
        let default = themes
            .iter()
            .find(|t| t.file.default)
            .or(themes.first())
            .map(|t| t.file.name.clone())
            .unwrap_or_default();
        ThemeLibrary {
            themes,
            rejected,
            theme: default.into(),
            appearance: Appearance::System,
            text_size: TextSize::Default,
            user_dir,
            user_stamp,
        }
    }

    pub fn global(cx: &App) -> &ThemeLibrary {
        cx.global::<ThemeLibrary>()
    }

    pub fn active(&self) -> Option<&LoadedTheme> {
        self.themes
            .iter()
            .find(|t| t.file.name == self.theme.as_ref())
            .or(self.themes.first())
    }

    pub fn user_dir(&self) -> Option<&Path> {
        self.user_dir.as_deref()
    }
}

fn register(library: &ThemeLibrary, cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    let mut set = Vec::new();
    for theme in &library.themes {
        for mode in [Mode::Dark, Mode::Light] {
            match kit_theme_json(&theme.file, mode)
                .ok()
                .and_then(|value| serde_json::from_value::<ThemeConfig>(value).ok())
            {
                Some(config) => set.push(config),
                None => tracing_error(&theme.file.name),
            }
        }
    }
    let content = serde_json::json!({ "name": "Consort", "themes": set });
    if let Err(err) = registry.load_themes_from_str(&content.to_string()) {
        eprintln!("consort-theme: GPUI Kit did not accept the themes: {err}");
    }
}

fn tracing_error(name: &str) {
    eprintln!("consort-theme: the theme {name} could not be mapped to GPUI Kit");
}

/// Loads the built-in and user themes, registers them with GPUI Kit, and
/// sets the [`Tokens`] global. Call it once, after `gpui_kit::init`.
pub fn init(cx: &mut App) {
    let library = ThemeLibrary::load(user_themes_dir());
    register(&library, cx);
    cx.set_global(library);
    apply(cx.window_appearance(), cx);
}

/// The mode that the Mode setting gives under the system appearance.
pub fn resolve_mode(appearance: Appearance, system: WindowAppearance) -> Mode {
    match appearance {
        Appearance::Light => Mode::Light,
        Appearance::Dark => Mode::Dark,
        Appearance::System => match system {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Mode::Dark,
            WindowAppearance::Light | WindowAppearance::VibrantLight => Mode::Light,
        },
    }
}

/// Applies the library's choice under the given system appearance: the GPUI
/// Kit theme and the [`Tokens`] global.
pub fn apply(system: WindowAppearance, cx: &mut App) {
    let library = ThemeLibrary::global(cx).clone();
    let Some(active) = library.active() else {
        return;
    };
    let mode = resolve_mode(library.appearance, system);
    let name = &active.file.name;
    let registry = ThemeRegistry::global(cx);
    let light = registry
        .themes()
        .get(&SharedString::from(kit_theme_name(name, Mode::Light)))
        .cloned();
    let dark = registry
        .themes()
        .get(&SharedString::from(kit_theme_name(name, Mode::Dark)))
        .cloned();
    if let (Some(light), Some(dark)) = (light, dark) {
        let theme = Theme::global_mut(cx);
        theme.light_theme = light;
        theme.dark_theme = dark;
    }
    let kit_mode = if mode.is_dark() {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    Theme::change(kit_mode, None, cx);
    cx.set_global(Tokens::new(&active.file, mode, library.text_size));
    cx.refresh_windows();
}

/// Changes the theme, the mode setting or the text size, and applies it.
pub fn choose(
    theme: Option<SharedString>,
    appearance: Option<Appearance>,
    text_size: Option<TextSize>,
    window: &Window,
    cx: &mut App,
) {
    let library = cx.global_mut::<ThemeLibrary>();
    if let Some(theme) = theme {
        library.theme = theme;
    }
    if let Some(appearance) = appearance {
        library.appearance = appearance;
    }
    if let Some(text_size) = text_size {
        library.text_size = text_size;
    }
    apply(window.appearance(), cx);
}

/// Reloads the user themes when a file in the folder changed. Returns true
/// when it reloaded.
pub fn reload_if_changed(system: WindowAppearance, cx: &mut App) -> bool {
    let library = ThemeLibrary::global(cx);
    let Some(dir) = library.user_dir.clone() else {
        return false;
    };
    if dir_stamp(&dir) == library.user_stamp {
        return false;
    }
    let previous = library.clone();
    let mut fresh = ThemeLibrary::load(Some(dir));
    fresh.appearance = previous.appearance;
    fresh.text_size = previous.text_size;
    if fresh
        .themes
        .iter()
        .any(|t| t.file.name == previous.theme.as_ref())
    {
        fresh.theme = previous.theme;
    }
    register(&fresh, cx);
    cx.set_global(fresh);
    apply(system, cx);
    true
}

/// Polls the user themes folder every two seconds and applies a changed
/// theme at once. Polling keeps the crate free of a file watcher dependency.
pub fn watch_user_themes(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            cx.update(|cx| {
                let system = cx.window_appearance();
                reload_if_changed(system, cx);
            });
        }
    })
    .detach();
}
