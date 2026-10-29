//! Settings that turn off the self-update of each tool.
//!
//! Consort updates managed tools itself. A tool that updates itself would
//! change files under the tools root while a session uses them. The worker
//! applies these settings in each profile folder.
//!
//! - Claude Code: [`CLAUDE_DISABLE_AUTOUPDATER`] in the environment.
//! - Codex: [`CODEX_CONFIG_LINE`] in the profile `config.toml`.
//! - OpenCode: [`OPENCODE_DISABLE_AUTOUPDATE`] in the environment.
//! - Gemini CLI: `general.enableAutoUpdate: false` in the profile
//!   `settings.json` (see [`gemini_disable_auto_update`]).
//!
//! [`crate::ToolStore::launch`] already adds the environment variables.

/// The environment variable that stops Claude Code from updating itself.
pub const CLAUDE_DISABLE_AUTOUPDATER: (&str, &str) = ("DISABLE_AUTOUPDATER", "1");

/// The environment variable that stops OpenCode from updating itself.
pub const OPENCODE_DISABLE_AUTOUPDATE: (&str, &str) = ("OPENCODE_DISABLE_AUTOUPDATE", "true");

/// The top-level `config.toml` line that stops Codex from checking for
/// updates.
pub const CODEX_CONFIG_LINE: &str = "check_for_update_on_startup = false";

/// Returns the Codex `config.toml` text with [`CODEX_CONFIG_LINE`] set.
///
/// The key is a top-level key, so it must come before the first table
/// header. An existing top-level line for the key is replaced. Other lines
/// do not change.
pub fn codex_disable_update_check(config_toml: &str) -> String {
    const KEY: &str = "check_for_update_on_startup";
    let mut out = Vec::new();
    let mut in_top_level = true;
    let mut done = false;
    for line in config_toml.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            in_top_level = false;
        }
        let is_key = trimmed
            .strip_prefix(KEY)
            .is_some_and(|rest| rest.trim_start().starts_with('='));
        if in_top_level && is_key {
            if !done {
                out.push(CODEX_CONFIG_LINE.to_owned());
                done = true;
            }
            continue;
        }
        out.push(line.to_owned());
    }
    if !done {
        out.insert(0, CODEX_CONFIG_LINE.to_owned());
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Sets `general.enableAutoUpdate` to `false` in a Gemini CLI
/// `settings.json` value, and keeps every other setting.
///
/// A value that is not an object becomes an object.
pub fn gemini_disable_auto_update(settings: &mut serde_json::Value) {
    use serde_json::{Map, Value};
    if !settings.is_object() {
        *settings = Value::Object(Map::new());
    }
    let Value::Object(root) = settings else {
        return;
    };
    let general = root
        .entry("general")
        .or_insert_with(|| Value::Object(Map::new()));
    if !general.is_object() {
        *general = Value::Object(Map::new());
    }
    if let Value::Object(general) = general {
        general.insert("enableAutoUpdate".to_owned(), Value::Bool(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_line_goes_before_the_first_table() {
        let text = "model = \"o3\"\n[profiles.x]\ncheck_for_update_on_startup = true\n";
        let out = codex_disable_update_check(text);
        assert_eq!(
            out,
            "check_for_update_on_startup = false\nmodel = \"o3\"\n[profiles.x]\ncheck_for_update_on_startup = true\n"
        );
    }

    #[test]
    fn codex_line_replaces_a_top_level_value() {
        let text = "check_for_update_on_startup = true\nmodel = \"o3\"\n";
        let out = codex_disable_update_check(text);
        assert_eq!(out, "check_for_update_on_startup = false\nmodel = \"o3\"\n");
        assert_eq!(codex_disable_update_check(&out), out);
        assert_eq!(
            codex_disable_update_check(""),
            "check_for_update_on_startup = false\n"
        );
    }

    #[test]
    fn gemini_setting_keeps_other_keys() {
        let mut value = json!({ "general": { "vimMode": true }, "ui": { "theme": "x" } });
        gemini_disable_auto_update(&mut value);
        assert_eq!(
            value,
            json!({ "general": { "vimMode": true, "enableAutoUpdate": false }, "ui": { "theme": "x" } })
        );
        let mut empty = serde_json::Value::Null;
        gemini_disable_auto_update(&mut empty);
        assert_eq!(empty, json!({ "general": { "enableAutoUpdate": false } }));
    }
}
