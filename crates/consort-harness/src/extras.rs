//! Provider extras: what a harness account brings from the provider's side,
//! and how a session starts without it.
//!
//! Only Claude Code has account-synced extras that Consort can see and turn
//! off (see `docs/notes/step2-worker.md`). For Claude Code the extras are
//! the claude.ai MCP connectors, the synced plugins and the synced skills.
//!
//! The report reads folder names only. It never opens a file, and it never
//! touches a login file.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use consort_proto::model::{ExtrasGroup, PROVIDER_EXTRAS_NAMES_MAX, ProviderExtras};
use serde_json::{Value, json};

use crate::account::{default_home, expand_home};
use crate::{AccountConfig, HarnessKind};

/// The folder of synced plugins, in the Claude config folder. Each folder
/// inside it belongs to one organization and holds one folder per plugin.
pub const CLAUDE_SYNCED_PLUGINS: &str = "plugins/synced";
/// The folder of synced skills, in the Claude config folder. Each folder
/// inside it belongs to one organization and holds one folder per skill.
pub const CLAUDE_SYNCED_SKILLS: &str = "skills/synced";

/// The inline Claude Code settings of a session with provider extras off.
///
/// Claude Code applies inline settings (the same as `--settings`) to one run
/// only. They stop the downloads and hide the synced items. They move no
/// file. The `syncClaudeAi*` keys must never go into a settings file of the
/// profile: there, Claude Code moves the synced folders to `.trash` at the
/// next start.
pub fn claude_extras_off_settings() -> Value {
    json!({
        "disableClaudeAiConnectors": true,
        "syncClaudeAiSkills": false,
        "syncClaudeAiPlugins": false,
    })
}

/// The environment variable that is a second guard against claude.ai
/// connectors, with its value for a session with provider extras off.
pub const CLAUDE_CONNECTORS_ENV: (&str, &str) = ("ENABLE_CLAUDEAI_MCP_SERVERS", "false");

/// The Claude config folder of an account: `CLAUDE_CONFIG_DIR` from the
/// account's runtime or own environment, the shadow home, the home, or the
/// default `~/.claude`.
pub fn claude_config_dir(account: &AccountConfig) -> Option<PathBuf> {
    let from_env = account
        .runtime
        .env
        .get("CLAUDE_CONFIG_DIR")
        .or_else(|| account.env.get("CLAUDE_CONFIG_DIR"));
    if let Some(dir) = from_env {
        return Some(expand_home(dir));
    }
    if let Some(shadow) = &account.shadow_home {
        return Some(expand_home(shadow));
    }
    if let Some(home) = &account.home {
        return Some(expand_home(home));
    }
    default_home(HarnessKind::ClaudeCode)
}

/// What the account brings from the provider's side, when Consort can tell.
///
/// | Harness | Account-synced extras | The switch |
/// |---|---|---|
/// | Claude Code | claude.ai connectors, synced plugins, synced skills | inline settings (see [`claude_extras_off_settings`]) |
/// | Codex | with ChatGPT sign-in: ChatGPT apps, account plugins, cloud skills | `thread/start` config (see `codex::extras_off_config`) |
/// | Gemini CLI | none for a personal Google account. Admin controls of an organization | none for the admin controls |
/// | OpenCode | servers and providers of a connected OpenCode Console | none that spares the hub tools |
/// | ACP registry agents | differs by agent, mostly enterprise settings | none |
///
/// Only Claude Code lets the worker list its extras offline. For Codex the
/// result is `None`: the switch works, but the worker cannot count what
/// ChatGPT syncs. Gemini CLI, OpenCode and the registry agents report the
/// extras that they cannot turn off, in words for the account dialog. Pi
/// has none.
pub fn report(account: &AccountConfig) -> Option<ProviderExtras> {
    let only_words = |text: &str| ProviderExtras {
        cannot_turn_off: vec![text.to_owned()],
        ..ProviderExtras::default()
    };
    match account.harness {
        HarnessKind::ClaudeCode => {
            claude_config_dir(account).map(|dir| claude_extras(&dir, &account_cannot_turn_off()))
        }
        HarnessKind::GeminiCli => Some(only_words(
            "If your Google account belongs to an organization, the admin settings of that organization apply. They cannot be turned off.",
        )),
        HarnessKind::OpenCode => Some(only_words(
            "OpenCode has no switch for the servers and providers that a connected OpenCode Console adds.",
        )),
        HarnessKind::Acp => Some(only_words(
            "Consort has no switch for what this agent syncs from its provider, such as organization settings.",
        )),
        HarnessKind::Codex | HarnessKind::Pi => None,
    }
}

/// The words for the account dialog about what the switch does not reach.
fn account_cannot_turn_off() -> Vec<String> {
    vec!["Plugins and skills that you installed by hand in this profile stay on.".to_owned()]
}

/// The extras of a Claude config folder, from folder names only.
///
/// `mcp_servers` stays empty: the claude.ai connectors come from the
/// network, and a session reports them. The worker cannot list them offline.
pub fn claude_extras(config_dir: &Path, cannot_turn_off: &[String]) -> ProviderExtras {
    ProviderExtras {
        mcp_servers: ExtrasGroup::default(),
        plugins: synced_group(&config_dir.join(CLAUDE_SYNCED_PLUGINS)),
        skills: synced_group(&config_dir.join(CLAUDE_SYNCED_SKILLS)),
        cannot_turn_off: cannot_turn_off.to_vec(),
    }
}

/// The distinct folder names inside the organization folders of `root`.
/// The count is the number of distinct names. The names are sorted and cut
/// at [`PROVIDER_EXTRAS_NAMES_MAX`].
fn synced_group(root: &Path) -> ExtrasGroup {
    let mut names = BTreeSet::new();
    let Ok(orgs) = std::fs::read_dir(root) else {
        return ExtrasGroup::default();
    };
    for org in orgs.flatten() {
        // `file_type` does not follow links, so a link out of the profile is
        // not listed.
        if !org.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(items) = std::fs::read_dir(org.path()) else {
            continue;
        };
        for item in items.flatten() {
            if !item.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let name = item.file_name().to_string_lossy().into_owned();
            if !name.starts_with('.') {
                names.insert(name);
            }
        }
    }
    ExtrasGroup {
        count: u32::try_from(names.len()).unwrap_or(u32::MAX),
        names: names.into_iter().take(PROVIDER_EXTRAS_NAMES_MAX).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(dir: &Path, rel: &str) {
        std::fs::create_dir_all(dir.join(rel)).unwrap();
    }

    #[test]
    fn counts_folder_names_under_the_synced_folders() {
        let dir = tempfile::tempdir().unwrap();
        make(dir.path(), "plugins/synced/org-a_1/engineering");
        make(dir.path(), "plugins/synced/org-a_1/legal");
        make(dir.path(), "plugins/synced/org-b_2/engineering");
        make(dir.path(), "plugins/synced/org-b_2/.cache");
        // Files do not count, and a file among the organizations is skipped.
        std::fs::write(
            dir.path().join("plugins/synced/org-a_1/manifest.json"),
            "{}",
        )
        .unwrap();
        std::fs::write(dir.path().join("plugins/synced/stray.txt"), "x").unwrap();
        make(dir.path(), "skills/synced/org-a_1/pdf");
        make(dir.path(), "skills/synced/org-a_1/docx");
        // A skill that the user made by hand is not synced.
        make(dir.path(), "skills/mine");

        let extras = claude_extras(dir.path(), &["stays".to_owned()]);
        assert_eq!(extras.plugins.count, 2);
        assert_eq!(extras.plugins.names, ["engineering", "legal"]);
        assert_eq!(extras.skills.count, 2);
        assert_eq!(extras.skills.names, ["docx", "pdf"]);
        assert_eq!(extras.mcp_servers, ExtrasGroup::default());
        assert_eq!(extras.cannot_turn_off, ["stays"]);
    }

    #[test]
    fn names_stop_at_the_limit_and_the_count_does_not() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..(PROVIDER_EXTRAS_NAMES_MAX + 5) {
            make(dir.path(), &format!("skills/synced/org_1/skill-{i:03}"));
        }
        let extras = claude_extras(dir.path(), &[]);
        assert_eq!(extras.skills.count as usize, PROVIDER_EXTRAS_NAMES_MAX + 5);
        assert_eq!(extras.skills.names.len(), PROVIDER_EXTRAS_NAMES_MAX);
        assert_eq!(extras.skills.names[0], "skill-000");
    }

    #[test]
    fn a_missing_config_folder_has_no_extras() {
        let dir = tempfile::tempdir().unwrap();
        let extras = claude_extras(&dir.path().join("none"), &[]);
        assert_eq!(extras.plugins, ExtrasGroup::default());
        assert_eq!(extras.skills, ExtrasGroup::default());
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_folder_is_not_listed() {
        let dir = tempfile::tempdir().unwrap();
        make(dir.path(), "outside/secret-name");
        make(dir.path(), "skills/synced/org_1");
        std::os::unix::fs::symlink(
            dir.path().join("outside/secret-name"),
            dir.path().join("skills/synced/org_1/linked"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            dir.path().join("outside"),
            dir.path().join("skills/synced/org_2"),
        )
        .unwrap();
        let extras = claude_extras(dir.path(), &[]);
        assert_eq!(extras.skills, ExtrasGroup::default());
    }

    #[test]
    fn the_report_reads_the_profile_folder_of_the_account() {
        let dir = tempfile::tempdir().unwrap();
        make(dir.path(), "skills/synced/org_1/pdf");
        let mut account = AccountConfig::default_for(HarnessKind::ClaudeCode);
        account.runtime.env.insert(
            "CLAUDE_CONFIG_DIR".into(),
            dir.path().to_string_lossy().into_owned(),
        );
        let extras = report(&account).unwrap();
        assert_eq!(extras.skills.names, ["pdf"]);
        assert!(!extras.cannot_turn_off.is_empty());

        // Codex and Pi have nothing to report. The other harnesses say what
        // they cannot turn off, and list nothing.
        for kind in [HarnessKind::Codex, HarnessKind::Pi] {
            assert_eq!(report(&AccountConfig::default_for(kind)), None);
        }
        for kind in [
            HarnessKind::GeminiCli,
            HarnessKind::OpenCode,
            HarnessKind::Acp,
        ] {
            let extras = report(&AccountConfig::default_for(kind)).unwrap();
            assert_eq!(extras.plugins, ExtrasGroup::default(), "{kind}");
            assert_eq!(extras.skills, ExtrasGroup::default(), "{kind}");
            assert_eq!(extras.cannot_turn_off.len(), 1, "{kind}");
        }
    }

    #[test]
    fn the_off_settings_never_name_a_file() {
        let settings = claude_extras_off_settings();
        assert_eq!(settings["disableClaudeAiConnectors"], true);
        assert_eq!(settings["syncClaudeAiSkills"], false);
        assert_eq!(settings["syncClaudeAiPlugins"], false);
        assert_eq!(settings.as_object().unwrap().len(), 3);
    }
}
