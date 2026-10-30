//! Every icon of the set, in the categories of the All icons dialog, with
//! the words that find it (`docs/notes/design-icon-picker.md`).
//!
//! A search word matches an icon when it is the start of a word in the
//! icon's name or in one of its keywords. The keywords also hold the old
//! Lucide names of the channel icons ("cloud", "drive", "shield"), so a user
//! who knows the old menu finds the icon that replaced it.

/// One icon: its name in the set and the words that find it.
pub struct IconEntry {
    pub name: &'static str,
    pub keywords: &'static [&'static str],
}

/// One category of the All icons dialog, in the order of the list.
pub struct IconCategory {
    pub title: &'static str,
    pub icons: &'static [IconEntry],
}

const fn e(name: &'static str, keywords: &'static [&'static str]) -> IconEntry {
    IconEntry { name, keywords }
}

pub const CATEGORIES: [IconCategory; 6] = [
    IconCategory {
        title: "Work and code",
        icons: &[
            e("code", &["programming", "dev", "software", "brackets"]),
            e("terminal", &["shell", "command", "cli", "console"]),
            e("branch", &["git", "version", "fork"]),
            e("pull-request", &["review", "merge", "git"]),
            e("file", &["document", "page", "paper"]),
            e("folder", &["directory", "files", "project"]),
            e("tasks", &["list", "checklist", "todo"]),
            e("compose", &["write", "draft", "new"]),
            e("edit", &["pencil", "change", "write"]),
            e("skill", &["ability", "spark", "tool"]),
            e("routine", &["schedule", "metronome", "repeat", "tempo"]),
            e("calendar", &["date", "day", "plan", "schedule"]),
            e("clock", &["time", "hour", "wait", "follow-up"]),
            e("effort", &["level", "bars", "strength", "speed", "signal"]),
            e("archive", &["box", "store", "old"]),
            e(
                "memory",
                &[
                    "notes", "notebook", "reading", "remember", "journal", "book",
                ],
            ),
        ],
    },
    IconCategory {
        title: "Machines",
        icons: &[
            e(
                "server",
                &["rack", "host", "homelab", "nas", "drive", "disk", "storage"],
            ),
            e(
                "worker",
                &[
                    "laptop",
                    "machine",
                    "computer",
                    "build server",
                    "remote",
                    "device",
                ],
            ),
            e("chip", &["cpu", "gpu", "hardware", "processor"]),
            e("bot", &["robot", "assistant", "ai", "agent"]),
            e("keyboard", &["type", "keys", "input"]),
            e("connector", &["service", "integration", "plug", "mcp"]),
            e(
                "globe",
                &["web", "world", "internet", "site", "language", "cloud"],
            ),
        ],
    },
    IconCategory {
        title: "Communication",
        icons: &[
            e("message", &["chat", "talk", "comment", "bubble"]),
            e("mail", &["email", "letter", "inbox"]),
            e("send", &["post", "submit", "arrow"]),
            e("reply", &["answer", "respond", "back"]),
            e("forward", &["share", "pass on"]),
            e(
                "voice",
                &["microphone", "mic", "speak", "audio", "dictation"],
            ),
            e("bell", &["alert", "notification", "ring", "reminder"]),
            e("users", &["people", "team", "group", "family"]),
            e("user", &["person", "profile", "account", "me"]),
            e("channel", &["hash", "tag", "topic"]),
            e("attach", &["paperclip", "clip", "attachment"]),
            e("quote", &["citation", "saying", "text"]),
        ],
    },
    IconCategory {
        title: "Places and things",
        icons: &[
            e(
                "home",
                &["house", "building", "smart home", "lights", "lightbulb"],
            ),
            e(
                "sun",
                &[
                    "day",
                    "light",
                    "morning",
                    "bright",
                    "lightbulb",
                    "bulb",
                    "idea",
                ],
            ),
            e("moon", &["night", "dark", "sleep", "evening"]),
            e("star", &["favorite", "best", "mark"]),
            e("star-filled", &["favorite", "important", "pinned"]),
            e("pin", &["keep", "place", "location", "map"]),
            e("image", &["picture", "photo", "art", "gallery"]),
            e("tag", &["label", "price", "name"]),
            e("key", &["secret", "password", "access", "token"]),
            e("lock", &["private", "secure", "closed", "shield"]),
            e("unlock", &["open", "public", "unsecured"]),
            e("eye", &["watch", "view", "look", "check-in", "monitor"]),
            e("eye-off", &["hide", "hidden", "private", "ignore"]),
            e(
                "appearance",
                &["palette", "theme", "color", "design", "paint"],
            ),
        ],
    },
    IconCategory {
        title: "Status and actions",
        icons: &[
            e("check", &["tick", "yes", "ok", "correct"]),
            e("done", &["finished", "complete", "ready", "ring"]),
            e("failed", &["error", "cross", "wrong", "broken"]),
            e("needs-input", &["fermata", "hold", "waiting", "question"]),
            e("working", &["busy", "progress", "running", "spinner"]),
            e("warning", &["caution", "danger", "alert", "triangle"]),
            e("info", &["information", "about", "details"]),
            e("help", &["question", "support", "faq", "guide"]),
            e("pause", &["hold", "wait", "break"]),
            e("play", &["start", "run", "go", "media"]),
            e("stop", &["halt", "end", "square"]),
            e("retry", &["again", "repeat", "refresh", "redo", "reload"]),
            e("download", &["save", "get", "in", "arrow down"]),
            e("upload", &["publish", "out", "arrow up"]),
            e("search", &["find", "look up", "magnifier", "zoom"]),
            e("trash", &["delete", "bin", "remove", "garbage"]),
            e("copy", &["duplicate", "clone", "clipboard"]),
            e("link", &["chain", "url", "connect", "anchor"]),
        ],
    },
    IconCategory {
        title: "Controls and arrows",
        icons: &[
            e("back", &["previous", "return", "arrow left"]),
            e("chevron-down", &["arrow", "open", "expand", "below"]),
            e("chevron-left", &["arrow", "previous", "back"]),
            e("chevron-right", &["arrow", "next", "forward"]),
            e("chevron-up", &["arrow", "close", "collapse", "above"]),
            e("chevrons-up-down", &["up down", "select", "swap", "arrows"]),
            e("close", &["cross", "x", "cancel", "dismiss"]),
            e("minus", &["subtract", "less", "remove", "dash"]),
            e("plus", &["add", "new", "more", "create"]),
            e(
                "more-horizontal",
                &["dots", "menu", "ellipsis", "three dots"],
            ),
            e("more-vertical", &["dots", "menu", "ellipsis", "kebab"]),
            e("sort", &["order", "arrange", "rank"]),
            e(
                "sliders",
                &["tune", "adjust", "settings", "controls", "mixer"],
            ),
            e("sidebar", &["panel", "layout", "side", "columns"]),
            e("filter", &["funnel", "narrow", "refine"]),
            e("settings", &["gear", "cog", "preferences", "config"]),
            e("log-out", &["sign out", "exit", "leave", "door"]),
            e("external-link", &["open", "new window", "outside"]),
        ],
    },
];

/// One category of a search result: its title and the icons that match, in
/// the order of the list.
#[derive(Clone, Debug, PartialEq)]
pub struct ResultGroup {
    pub title: &'static str,
    pub icons: Vec<&'static str>,
}

/// The words of a name or a keyword: `pull-request` is "pull" and
/// "request".
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c == '-' || c.is_whitespace())
        .filter(|w| !w.is_empty())
}

/// True when every word of `query` starts a word of `text`. "look up"
/// matches "look up", and "serv" matches "server".
fn starts_words(text: &str, query: &[String]) -> bool {
    query
        .iter()
        .all(|q| words(text).any(|w| w.to_lowercase().starts_with(q.as_str())))
}

/// True when every word of `query` starts a word of the icon's name or of
/// any of its keywords: "arrow down" finds `chevron-down`.
fn matches_anywhere(icon: &IconEntry, query: &[String]) -> bool {
    query.iter().all(|q| {
        std::iter::once(icon.name)
            .chain(icon.keywords.iter().copied())
            .flat_map(words)
            .any(|w| w.to_lowercase().starts_with(q.as_str()))
    })
}

/// The icons that match `query`, by category. An empty query gives every
/// icon. In each category, the icons whose name matches come first, then
/// the icons that match only by a keyword. A category with no match is left
/// out.
pub fn search(query: &str) -> Vec<ResultGroup> {
    let query: Vec<String> = words(query.trim()).map(str::to_lowercase).collect();
    CATEGORIES
        .iter()
        .filter_map(|category| {
            let (mut by_name, mut by_keyword) = (Vec::new(), Vec::new());
            for icon in category.icons {
                if query.is_empty() || starts_words(icon.name, &query) {
                    by_name.push(icon.name);
                } else if matches_anywhere(icon, &query) {
                    by_keyword.push(icon.name);
                }
            }
            by_name.append(&mut by_keyword);
            (!by_name.is_empty()).then_some(ResultGroup {
                title: category.title,
                icons: by_name,
            })
        })
        .collect()
}

/// The icons of a result in one list, in the order of the dialog. The arrow
/// keys move through this list.
pub fn flatten(groups: &[ResultGroup]) -> Vec<&'static str> {
    groups
        .iter()
        .flat_map(|g| g.icons.iter().copied())
        .collect()
}

/// The name of an icon in words, for its tooltip: `pull-request` is "Pull
/// request".
pub fn label(name: &str) -> String {
    let words = name.replace('-', " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The count line of a search: "1 icon" or "12 icons".
pub fn count_text(count: usize) -> String {
    if count == 1 {
        "1 icon".to_owned()
    } else {
        format!("{count} icons")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn names(query: &str) -> Vec<&'static str> {
        flatten(&search(query))
    }

    /// Every icon file of the set is in exactly one category, and the
    /// catalog names no icon that does not exist.
    #[test]
    fn the_catalog_holds_every_icon_once() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons");
        let files: BTreeSet<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| {
                let name = e.unwrap().file_name().into_string().unwrap();
                name.strip_suffix(".svg").map(str::to_owned)
            })
            .collect();
        let listed: Vec<&str> = names("");
        let unique: BTreeSet<String> = listed.iter().map(|n| (*n).to_owned()).collect();
        assert_eq!(listed.len(), unique.len(), "an icon is listed twice");
        assert_eq!(unique, files);
        assert_eq!(listed.len(), 85);
    }

    #[test]
    fn a_word_finds_names_first_then_keywords() {
        // "serv" starts the name "server" and the keywords "service" and
        // "build server".
        assert_eq!(names("serv"), ["server", "worker", "connector"]);
        assert_eq!(
            search("serv").iter().map(|g| g.title).collect::<Vec<_>>(),
            ["Machines"]
        );
        // Not case sensitive, and the start of a word only.
        assert_eq!(names("SERV"), names("serv"));
        assert!(names("erver").is_empty());
    }

    #[test]
    fn the_old_menu_names_find_their_icons() {
        assert!(names("cloud").contains(&"globe"));
        assert!(names("drive").contains(&"server"));
        assert!(names("shield").contains(&"lock"));
        assert!(names("hash").contains(&"channel"));
        assert!(names("mic").contains(&"voice"));
        assert!(names("bulb").contains(&"sun"));
        assert!(names("book").contains(&"memory"));
        assert!(names("terminal").contains(&"terminal"));
    }

    #[test]
    fn two_words_must_both_match() {
        assert_eq!(names("look up"), ["search"]);
        assert!(names("arrow").len() > 5);
        assert!(names("arrow down").contains(&"download"));
        assert!(names("arrow down").contains(&"chevron-down"));
        assert!(!names("arrow down").contains(&"upload"));
    }

    #[test]
    fn no_match_gives_no_group() {
        assert!(search("xyz").is_empty());
        assert_eq!(search("  ").len(), CATEGORIES.len());
    }

    #[test]
    fn labels_and_counts_read_as_words() {
        assert_eq!(label("pull-request"), "Pull request");
        assert_eq!(label("eye-off"), "Eye off");
        assert_eq!(count_text(1), "1 icon");
        assert_eq!(count_text(3), "3 icons");
    }
}
