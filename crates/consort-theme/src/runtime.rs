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
use serde::{Deserialize, Serialize};

use crate::{
    file::{Mode, ThemeError, ThemeFile, load_theme},
    mapping::{kit_theme_json, kit_theme_name},
    tokens::{TextSize, Tokens},
};

/// The Mode setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// The Reduce motion setting. `None` until the user sets it: the app
    /// then starts from the system setting.
    pub reduce_motion: Option<bool>,
    user_dir: Option<PathBuf>,
    user_stamp: Vec<(PathBuf, Option<SystemTime>)>,
}

impl Global for ThemeLibrary {}

pub use crate::builtin::builtin_theme_sources;

/// The user themes folder: `<config dir>/consort/themes`.
pub fn user_themes_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.config_dir().join("consort").join("themes"))
}

/// What the user chose in Settings, as it is saved between runs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedChoice {
    /// The theme name. `None` keeps the default theme.
    pub theme: Option<String>,
    pub appearance: Appearance,
    pub text_size: TextSize,
    /// The Reduce motion setting. Not in the file until the user sets it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reduce_motion: Option<bool>,
}

/// The Consort config folder: `$CONSORT_CONFIG_DIR` when it is set, else
/// `<config dir>/consort`. The client keeps its files in the same folder
/// (`Store::default_location` in `consort-client`), so a run with its own
/// `CONSORT_CONFIG_DIR` never touches the files of the real app.
pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(std::env::var_os("CONSORT_CONFIG_DIR"), || {
        directories::BaseDirs::new().map(|dirs| dirs.config_dir().to_path_buf())
    })
}

fn config_dir_from(
    env: Option<std::ffi::OsString>,
    base: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    match env {
        Some(dir) => Some(PathBuf::from(dir)),
        None => base().map(|dir| dir.join("consort")),
    }
}

/// The file that holds the choice: `appearance.json` in the config folder
/// (see [`config_dir`]).
pub fn choice_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("appearance.json"))
}

/// Reads the saved choice. A missing or damaged file gives the defaults, so a
/// bad file never keeps the app from starting.
pub fn read_choice(path: &Path) -> SavedChoice {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Saves the choice through a temporary file and a rename.
pub fn write_choice(path: &Path, choice: &SavedChoice) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(choice).map_err(std::io::Error::other)?;
    // Each write has its own temporary file, so two quick writes never
    // write into the same file or rename it from under each other.
    let temp = temp_path(path);
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// A temporary file name next to `path` that no other write uses: the process
/// ID, a counter and the time.
fn temp_path(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{name}.{}.{n}.{nanos}.tmp", std::process::id()))
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
            reduce_motion: None,
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

    /// The choice to save.
    pub fn choice(&self) -> SavedChoice {
        SavedChoice {
            theme: Some(self.theme.to_string()),
            appearance: self.appearance,
            text_size: self.text_size,
            reduce_motion: self.reduce_motion,
        }
    }

    /// Applies a saved choice. A theme that is not here any more (the user
    /// deleted its file) leaves the current theme as it is.
    pub fn apply_choice(&mut self, choice: &SavedChoice) {
        if let Some(name) = &choice.theme
            && self.themes.iter().any(|t| &t.file.name == name)
        {
            self.theme = name.clone().into();
        }
        self.appearance = choice.appearance;
        self.text_size = choice.text_size;
        self.reduce_motion = choice.reduce_motion;
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
    let mut library = ThemeLibrary::load(user_themes_dir());
    if let Some(path) = choice_path() {
        library.apply_choice(&read_choice(&path));
    }
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

/// Changes the theme, the mode setting or the text size, applies it, and
/// saves the choice for the next run.
pub fn choose(
    theme: Option<SharedString>,
    appearance: Option<Appearance>,
    text_size: Option<TextSize>,
    window: &Window,
    cx: &mut App,
) {
    choose_without_saving(theme, appearance, text_size, window, cx);
    let choice = ThemeLibrary::global(cx).choice();
    if let Some(path) = choice_path() {
        // The write goes off the UI thread.
        cx.background_executor()
            .spawn(async move {
                if let Err(err) = write_choice(&path, &choice) {
                    eprintln!(
                        "consort-theme: cannot save your theme choice in {}: {err}. Check that you may write to that folder",
                        path.display()
                    );
                }
            })
            .detach();
    }
}

/// The Reduce motion setting at start: the saved choice when the user made
/// one, else `system` (what the operating system asks for, when the app can
/// read it).
pub fn start_reduce_motion(saved: Option<bool>, system: Option<bool>) -> bool {
    saved.or(system).unwrap_or(false)
}

/// Turns Reduce motion on or off for every window, and saves the choice.
/// GPUI's own `reduce_motion` flag carries it, so GPUI's animations follow
/// it too.
pub fn choose_reduce_motion(on: bool, cx: &mut App) {
    cx.global_mut::<ThemeLibrary>().reduce_motion = Some(on);
    cx.set_reduce_motion(on);
    let choice = ThemeLibrary::global(cx).choice();
    if let Some(path) = choice_path() {
        cx.background_executor()
            .spawn(async move {
                if let Err(err) = write_choice(&path, &choice) {
                    eprintln!(
                        "consort-theme: cannot save your motion choice in {}: {err}. Check that you may write to that folder",
                        path.display()
                    );
                }
            })
            .detach();
    }
}

/// Like [`choose`], but the choice lasts for this run only. For command line
/// options such as `--dark`, which must not change the user's saved choice.
pub fn choose_without_saving(
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
/// when it reloaded. This reads the folder on the calling thread, so the
/// poll in [`watch_user_themes`] does not call it.
pub fn reload_if_changed(system: WindowAppearance, cx: &mut App) -> bool {
    let library = ThemeLibrary::global(cx);
    let Some(dir) = library.user_dir.clone() else {
        return false;
    };
    if dir_stamp(&dir) == library.user_stamp {
        return false;
    }
    let fresh = ThemeLibrary::load(Some(dir));
    adopt(fresh, system, cx);
    true
}

/// Takes a library that was just loaded from disk, keeps the user's choice
/// where it still holds, and applies it.
fn adopt(mut fresh: ThemeLibrary, system: WindowAppearance, cx: &mut App) {
    let previous = ThemeLibrary::global(cx).clone();
    fresh.appearance = previous.appearance;
    fresh.text_size = previous.text_size;
    fresh.reduce_motion = previous.reduce_motion;
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
}

/// Polls the user themes folder every two seconds and applies a changed
/// theme at once. The folder is read and the themes are parsed on a
/// background thread, so the UI thread only swaps in the result. Polling
/// keeps the crate free of a file watcher dependency.
pub fn watch_user_themes(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let Some((dir, stamp)) = cx.update(|cx| {
                let library = ThemeLibrary::global(cx);
                library
                    .user_dir
                    .clone()
                    .map(|dir| (dir, library.user_stamp.clone()))
            }) else {
                continue;
            };
            let fresh =
                cx.background_executor()
                    .spawn(async move {
                        (dir_stamp(&dir) != stamp).then(|| ThemeLibrary::load(Some(dir)))
                    })
                    .await;
            if let Some(fresh) = fresh {
                cx.update(|cx| {
                    let system = cx.window_appearance();
                    adopt(fresh, system, cx);
                });
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> ThemeLibrary {
        ThemeLibrary::load(None)
    }

    #[test]
    fn a_choice_round_trips_through_a_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("appearance.json");
        let choice = SavedChoice {
            theme: Some("Velvet".into()),
            appearance: Appearance::Dark,
            text_size: TextSize::Larger,
            reduce_motion: None,
        };
        write_choice(&path, &choice).expect("writes");
        assert_eq!(read_choice(&path), choice);
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("reads the folder")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "no temp file: {leftovers:?}");
    }

    /// Finding 15c of review cycle 2: every write used `appearance.json.tmp`,
    /// so two quick writes raced on one file.
    #[test]
    fn quick_writes_use_their_own_temporary_files_and_end_with_a_whole_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("appearance.json");
        let names: std::collections::HashSet<_> = (0..50).map(|_| temp_path(&path)).collect();
        assert_eq!(names.len(), 50, "no two writes share a temp name");
        assert!(names.iter().all(|n| n.parent() == path.parent()));
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for j in 0..20 {
                        let choice = SavedChoice {
                            theme: Some(format!("Theme {i}-{j}")),
                            appearance: Appearance::Dark,
                            text_size: TextSize::Default,
                            reduce_motion: None,
                        };
                        write_choice(&path, &choice).expect("writes");
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().expect("a writer thread");
        }
        // The file reads as one whole choice, not a mix of two.
        let saved = read_choice(&path);
        assert!(saved.theme.is_some_and(|t| t.starts_with("Theme ")));
        let leftovers = std::fs::read_dir(dir.path())
            .expect("reads the folder")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    /// Finding 15b of review cycle 2: a test run wrote the real
    /// `appearance.json`, because the path ignored `CONSORT_CONFIG_DIR`.
    #[test]
    fn the_config_folder_follows_consort_config_dir_like_the_client() {
        let base = || Some(PathBuf::from("/home/me/.config"));
        assert_eq!(
            config_dir_from(Some("/tmp/e2e".into()), base),
            Some(PathBuf::from("/tmp/e2e"))
        );
        assert_eq!(
            config_dir_from(None, base),
            Some(PathBuf::from("/home/me/.config/consort"))
        );
        assert_eq!(config_dir_from(None, || None), None);
    }

    #[test]
    fn a_missing_or_damaged_choice_file_gives_the_defaults() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("appearance.json");
        assert_eq!(read_choice(&path), SavedChoice::default());
        std::fs::write(&path, "{ not json").expect("writes");
        assert_eq!(read_choice(&path), SavedChoice::default());
        // A file from a newer version with unknown fields still reads.
        std::fs::write(&path, r#"{"appearance":"light","future":1}"#).expect("writes");
        assert_eq!(read_choice(&path).appearance, Appearance::Light);
    }

    #[test]
    fn the_saved_choice_sets_theme_mode_and_text_size() {
        let mut lib = library();
        assert_eq!(lib.appearance, Appearance::System);
        let velvet = lib
            .themes
            .iter()
            .find(|t| t.file.name == "Velvet")
            .map(|t| t.file.name.clone())
            .expect("velvet");
        lib.apply_choice(&SavedChoice {
            theme: Some(velvet.clone()),
            appearance: Appearance::Light,
            text_size: TextSize::Smaller,
            reduce_motion: None,
        });
        assert_eq!(lib.theme.as_ref(), velvet);
        assert_eq!(lib.appearance, Appearance::Light);
        assert_eq!(lib.text_size, TextSize::Smaller);
        assert_eq!(lib.choice().theme.as_deref(), Some("Velvet"));
    }

    #[test]
    fn reduce_motion_starts_from_the_saved_choice_then_the_system() {
        assert!(start_reduce_motion(Some(true), Some(false)));
        assert!(!start_reduce_motion(Some(false), Some(true)));
        assert!(start_reduce_motion(None, Some(true)));
        assert!(!start_reduce_motion(None, None));
        // An old file with no reduce_motion reads as "not set".
        let old: SavedChoice = serde_json::from_str(r#"{"appearance":"dark"}"#).expect("parses");
        assert_eq!(old.reduce_motion, None);
        let mut lib = library();
        lib.apply_choice(&SavedChoice {
            reduce_motion: Some(true),
            ..SavedChoice::default()
        });
        assert_eq!(lib.choice().reduce_motion, Some(true));
    }

    #[test]
    fn a_saved_theme_that_is_gone_keeps_the_current_theme() {
        let mut lib = library();
        let before = lib.theme.clone();
        lib.apply_choice(&SavedChoice {
            theme: Some("Deleted".into()),
            appearance: Appearance::Dark,
            text_size: TextSize::Default,
            reduce_motion: None,
        });
        assert_eq!(lib.theme, before);
        assert_eq!(lib.appearance, Appearance::Dark);
    }
}
