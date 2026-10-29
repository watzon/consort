//! Skill delivery: how a session's skills reach each harness.
//!
//! The worker fetches and checks the skill files (`consort-worker`,
//! `skills.rs`), keeps them in a cache by content hash, and calls
//! [`write_session_skills`] to copy them into a folder of the session. The
//! folder has the layout that the harness expects, and the adapter tells the
//! harness where it is. The worker calls [`finish_session_skills`] when the
//! session ends, which removes the folder, so one session never sees the
//! skills of another.
//!
//! A delivery never writes into the account's own folders (`~/.claude`,
//! `$CODEX_HOME`, `$GEMINI_CLI_HOME`, `~/.config/opencode`) and never into
//! the workspace.
//!
//! | Harness | Layout in the session folder | How the harness finds it |
//! |---|---|---|
//! | Claude Code | `consort-skills/` is a skills-only plugin | `plugins` in `_meta.claudeCode.options` |
//! | Codex | `skills/<name>/` | `skills/extraRoots/set` on the session's own `app-server` |
//! | Gemini CLI | `gemini-home/` is a copy of the account's Gemini home, made of links, with the skills in its `.gemini/skills/` | `GEMINI_CLI_HOME` of the agent process |
//! | OpenCode | `skills/<name>/` | `skills` in `OPENCODE_CONFIG_CONTENT` |
//! | other ACP agents | none | the hub tools `list_skills` and `read_skill` |
//!
//! `docs/notes/step2-worker.md` gives the reasons and the tests behind each
//! row.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::account::{build_shadow_home, expand_home, user_home};
use crate::{AccountConfig, HarnessError, HarnessKind};

/// The plugin folder for Claude Code, inside the session's skill folder.
pub const CLAUDE_PLUGIN_DIR: &str = "consort-skills";
/// The name of the generated Claude Code plugin. Claude Code names each of
/// its skills `consort-skills:<skill>`.
pub const CLAUDE_PLUGIN_NAME: &str = "consort-skills";
/// The folder with one folder per skill, inside the session's skill folder,
/// for harnesses that read a plain folder of skills.
pub const SKILLS_DIR: &str = "skills";
/// The Gemini home of a session, inside the session's skill folder.
pub const GEMINI_HOME_DIR: &str = "gemini-home";

/// One verified skill in the worker's cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSource {
    /// The skill name. It is the name of the skill's folder.
    pub name: String,
    /// The cache folder with the skill's files, with `SKILL.md` at its root.
    pub dir: PathBuf,
}

/// Files that a harness made inside a session folder and that must stay,
/// because the folder is a copy of an account folder. See
/// [`finish_session_skills`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reclaim {
    /// The folder in the session folder.
    pub from: PathBuf,
    /// The account folder that it copies.
    pub to: PathBuf,
    /// Entry names that stay in the session folder.
    pub skip: Vec<String>,
}

/// The skills that the worker wrote for one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSkills {
    /// The session's skill folder. The worker removes it when the session
    /// ends.
    pub root: PathBuf,
    /// The names of the skills, in the order of the session start.
    pub names: Vec<String>,
    /// Environment variables for the harness process, such as
    /// `GEMINI_CLI_HOME`.
    pub env: BTreeMap<String, String>,
    /// What to move back into an account folder when the session ends.
    pub reclaim: Option<Reclaim>,
}

impl SessionSkills {
    /// Skills in `root`, with no environment and nothing to reclaim.
    pub fn new(root: PathBuf, names: Vec<String>) -> Self {
        Self {
            root,
            names,
            env: BTreeMap::new(),
            reclaim: None,
        }
    }
}

/// Whether the harness loads skills from a folder, so that the hub does not
/// need to serve `list_skills` and `read_skill` to it. Pi and the other ACP
/// agents have no native skills. Gemini CLI needs links to the account's
/// files (see [`write_session_skills`]), so it has native delivery on Unix
/// only.
pub const fn native_delivery(kind: HarnessKind) -> bool {
    match kind {
        HarnessKind::ClaudeCode | HarnessKind::Codex | HarnessKind::OpenCode => true,
        HarnessKind::GeminiCli => cfg!(unix),
        HarnessKind::Pi | HarnessKind::Acp => false,
    }
}

/// Copies the skills into `root` in the layout of the account's harness. A
/// harness with no native delivery gets nothing, and the result is `None`.
///
/// `root` must be a folder that only this session uses. Two skills with the
/// same name are a mistake of the caller: the first one wins.
pub fn write_session_skills(
    account: &AccountConfig,
    root: &Path,
    skills: &[SkillSource],
) -> Result<Option<SessionSkills>, HarnessError> {
    let kind = account.harness;
    if skills.is_empty() || !native_delivery(kind) {
        return Ok(None);
    }
    let mut names: Vec<String> = Vec::new();
    let skills: Vec<&SkillSource> = skills
        .iter()
        .filter(|s| {
            let new = !names.contains(&s.name);
            if new {
                names.push(s.name.clone());
            } else {
                tracing::warn!(skill = %s.name, "two skills have the same name; the session gets the first");
            }
            new
        })
        .collect();
    // A new folder, so a link from an earlier run cannot lead anywhere.
    if root.exists() {
        std::fs::remove_dir_all(root)?;
    }
    let mut written = SessionSkills::new(root.to_path_buf(), names);
    let skill_folder = match kind {
        HarnessKind::ClaudeCode => {
            let plugin = root.join(CLAUDE_PLUGIN_DIR);
            let manifest_dir = plugin.join(".claude-plugin");
            std::fs::create_dir_all(&manifest_dir)?;
            std::fs::write(
                manifest_dir.join("plugin.json"),
                serde_json::to_vec_pretty(&claude_plugin_manifest())
                    .map_err(|e| HarnessError::Protocol(e.to_string()))?,
            )?;
            plugin.join(SKILLS_DIR)
        }
        HarnessKind::GeminiCli => gemini_overlay(account, root, &mut written)?,
        _ => root.join(SKILLS_DIR),
    };
    std::fs::create_dir_all(&skill_folder)?;
    for skill in skills {
        let target = skill_folder.join(&skill.name);
        // In the Gemini overlay, an own skill of the account is a link. Only
        // the link goes, never what it points to.
        if target.symlink_metadata().is_ok() {
            tracing::info!(skill = %skill.name, "a Consort skill replaces a skill of the account in this session");
            remove_entry(&target)?;
        }
        copy_tree(&skill.dir, &target)?;
    }
    Ok(Some(written))
}

/// Ends the delivery of a session: moves back what the harness made for
/// the first time inside a copy of an account folder (Gemini CLI makes
/// `tmp`, for example, at its first run), then removes the session folder.
/// A file that the harness wrote with a rename replaces the account's file.
pub fn finish_session_skills(skills: &SessionSkills) -> Result<(), HarnessError> {
    if let Some(reclaim) = &skills.reclaim {
        reclaim_new_entries(reclaim)?;
    }
    match std::fs::remove_dir_all(&skills.root) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// The `plugin.json` of the generated Claude Code plugin.
pub fn claude_plugin_manifest() -> Value {
    json!({
        "name": CLAUDE_PLUGIN_NAME,
        "version": "1.0.0",
        "description": "The skills that Consort gives to this session.",
    })
}

/// The folder of the generated Claude Code plugin.
pub fn claude_plugin_dir(skills: &SessionSkills) -> PathBuf {
    skills.root.join(CLAUDE_PLUGIN_DIR)
}

/// The `plugins` option for `_meta.claudeCode.options`: the Claude Agent SDK
/// takes `{type: "local", path}` and starts Claude Code with one
/// `--plugin-dir` for each.
pub fn claude_plugins_option(skills: &SessionSkills) -> Value {
    json!([{
        "type": "local",
        "path": claude_plugin_dir(skills).to_string_lossy(),
    }])
}

/// The folder for `skills/extraRoots/set` of Codex.
pub fn codex_extra_root(skills: &SessionSkills) -> PathBuf {
    skills.root.join(SKILLS_DIR)
}

/// The `skills` list for the OpenCode config (OpenCode 2 takes a list of
/// folders; the older `skills.paths` form still works).
pub fn opencode_skill_paths(skills: &SessionSkills) -> Vec<String> {
    vec![skills.root.join(SKILLS_DIR).to_string_lossy().into_owned()]
}

/// The Gemini home that the account uses: `GEMINI_CLI_HOME` from its
/// environment, else the user's home folder.
fn gemini_account_home(account: &AccountConfig) -> Option<PathBuf> {
    account
        .runtime
        .env
        .get("GEMINI_CLI_HOME")
        .or_else(|| account.env.get("GEMINI_CLI_HOME"))
        .map(|h| expand_home(h))
        .or_else(user_home)
}

/// Makes the Gemini home of a session: a copy of the account's Gemini home
/// in which each file and folder is a link, except `.gemini/skills`, which
/// is a real folder. Gemini CLI has no setting for more skill folders, and
/// it reads `$GEMINI_CLI_HOME/.gemini/skills` only. So each session gets its
/// own home, and the account's login, settings and history stay where they
/// are. The skills of the account stay too, as links. Returns the folder
/// for the session's skills.
fn gemini_overlay(
    account: &AccountConfig,
    root: &Path,
    written: &mut SessionSkills,
) -> Result<PathBuf, HarnessError> {
    let source = gemini_account_home(account).ok_or(HarnessError::Unsupported(
        "find the Gemini home folder of the account",
    ))?;
    let home = root.join(GEMINI_HOME_DIR);
    let gemini = home.join(".gemini");
    let account_gemini = source.join(".gemini");
    // Everything except `skills` is a link into the account's folder, so a
    // login that Gemini refreshes in the session stays in the account.
    build_shadow_home(&gemini, Some(&account_gemini), &[SKILLS_DIR])?;
    build_shadow_home(
        &gemini.join(SKILLS_DIR),
        Some(&account_gemini.join(SKILLS_DIR)),
        &[],
    )?;
    // The user scope has a second folder, `.agents/skills`.
    build_shadow_home(&home.join(".agents"), Some(&source.join(".agents")), &[])?;
    written.env.insert(
        "GEMINI_CLI_HOME".to_owned(),
        home.to_string_lossy().into_owned(),
    );
    written.reclaim = Some(Reclaim {
        from: gemini.clone(),
        to: account_gemini,
        skip: vec![SKILLS_DIR.to_owned()],
    });
    Ok(gemini.join(SKILLS_DIR))
}

/// Gives back what the harness wrote in `reclaim.from`: a real entry (not a
/// link) moves to `reclaim.to` when `to` has no such entry. A real file
/// replaces the file of `to`: the harness wrote it with a rename, which
/// replaced our link (Gemini CLI does this for `projects.json`). A link is
/// what [`gemini_overlay`] made, and it stays.
fn reclaim_new_entries(reclaim: &Reclaim) -> Result<(), HarnessError> {
    let Ok(entries) = std::fs::read_dir(&reclaim.from) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if reclaim.skip.iter().any(|s| name == s.as_str()) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        let target = reclaim.to.join(&name);
        if let Ok(existing) = target.symlink_metadata() {
            // A file that replaced our link: the harness wrote it with a
            // rename, so it is the newer one. A folder that both have, or
            // anything else, stays as the account has it.
            if kind.is_file() && existing.is_file() {
                std::fs::copy(entry.path(), &target)?;
            }
            continue;
        }
        std::fs::create_dir_all(&reclaim.to)?;
        if std::fs::rename(entry.path(), &target).is_err() {
            // Another file system: copy.
            if kind.is_dir() {
                copy_tree(&entry.path(), &target)?;
            } else {
                std::fs::copy(entry.path(), &target)?;
            }
        }
    }
    Ok(())
}

/// Removes a file, a link, or a folder. A link is removed, not followed.
fn remove_entry(path: &Path) -> std::io::Result<()> {
    let meta = path.symlink_metadata()?;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path).or_else(|_| std::fs::remove_dir(path))
    }
}

/// Copies a folder of plain files. A link inside the source is not
/// followed and not copied, so a skill cannot pull in a file from
/// somewhere else.
fn copy_tree(from: &Path, to: &Path) -> Result<(), HarnessError> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            // `copy` keeps the permission bits, so a script stays executable.
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(kind: HarnessKind) -> AccountConfig {
        AccountConfig::default_for(kind)
    }

    fn cached(dir: &Path, name: &str) -> SkillSource {
        let root = dir.join("cache").join(name);
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::write(root.join("SKILL.md"), format!("---\nname: {name}\n---\n")).unwrap();
        std::fs::write(root.join("scripts/run.sh"), "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                root.join("scripts/run.sh"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        SkillSource {
            name: name.to_owned(),
            dir: root,
        }
    }

    #[test]
    fn claude_gets_a_skills_only_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let skills = [cached(dir.path(), "pdf-forms"), cached(dir.path(), "notes")];
        let root = dir.path().join("session/skills");
        let written = write_session_skills(&account(HarnessKind::ClaudeCode), &root, &skills)
            .unwrap()
            .unwrap();
        assert_eq!(written.names, ["pdf-forms", "notes"]);
        assert!(written.env.is_empty() && written.reclaim.is_none());
        let plugin = claude_plugin_dir(&written);
        let manifest: Value = serde_json::from_slice(
            &std::fs::read(plugin.join(".claude-plugin/plugin.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["name"], "consort-skills");
        assert!(plugin.join("skills/pdf-forms/SKILL.md").is_file());
        assert!(plugin.join("skills/notes/scripts/run.sh").is_file());
        // The plugin has no hooks, commands, agents or MCP servers.
        let mut top: Vec<String> = std::fs::read_dir(&plugin)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        top.sort();
        assert_eq!(top, [".claude-plugin", "skills"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(plugin.join("skills/pdf-forms/scripts/run.sh"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);
        }
        let option = claude_plugins_option(&written);
        assert_eq!(option[0]["type"], "local");
        assert_eq!(option[0]["path"], plugin.to_string_lossy().as_ref());
        // The cache folder stays as it was.
        assert!(skills[0].dir.join("SKILL.md").is_file());
    }

    #[test]
    fn codex_and_opencode_get_a_plain_folder_of_skills() {
        let dir = tempfile::tempdir().unwrap();
        let skills = [cached(dir.path(), "pdf-forms")];
        for kind in [HarnessKind::OpenCode, HarnessKind::Codex] {
            let root = dir.path().join(format!("session-{kind}/skills"));
            let written = write_session_skills(&account(kind), &root, &skills)
                .unwrap()
                .unwrap();
            assert!(root.join("skills/pdf-forms/SKILL.md").is_file());
            let folder = root.join("skills").to_string_lossy().into_owned();
            assert_eq!(
                opencode_skill_paths(&written),
                std::slice::from_ref(&folder)
            );
            assert_eq!(codex_extra_root(&written), PathBuf::from(folder));
            assert!(written.env.is_empty());
        }
    }

    #[test]
    fn a_harness_without_native_skills_gets_nothing_written() {
        let dir = tempfile::tempdir().unwrap();
        let skills = [cached(dir.path(), "pdf-forms")];
        let root = dir.path().join("session/skills");
        for kind in [HarnessKind::Acp, HarnessKind::Pi] {
            assert_eq!(
                write_session_skills(&account(kind), &root, &skills).unwrap(),
                None
            );
            assert!(!root.exists());
        }
        assert_eq!(
            write_session_skills(&account(HarnessKind::ClaudeCode), &root, &[]).unwrap(),
            None
        );
    }

    #[test]
    fn a_second_write_replaces_the_first_and_the_first_duplicate_wins() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("session/skills");
        let first = cached(dir.path(), "a");
        let account = account(HarnessKind::OpenCode);
        let written = write_session_skills(&account, &root, std::slice::from_ref(&first))
            .unwrap()
            .unwrap();
        assert_eq!(written.names, ["a"]);
        let second = cached(dir.path(), "b");
        let written = write_session_skills(&account, &root, &[second.clone(), second])
            .unwrap()
            .unwrap();
        assert_eq!(written.names, ["b"]);
        assert!(!root.join("skills/a").exists(), "the old skill is gone");
        assert!(root.join("skills/b/SKILL.md").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_a_skill_is_not_copied() {
        let dir = tempfile::tempdir().unwrap();
        let skill = cached(dir.path(), "evil");
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, "private").unwrap();
        std::os::unix::fs::symlink(&outside, skill.dir.join("leak.txt")).unwrap();
        let root = dir.path().join("session/skills");
        write_session_skills(&account(HarnessKind::OpenCode), &root, &[skill]).unwrap();
        assert!(root.join("skills/evil/SKILL.md").is_file());
        assert!(
            root.join("skills/evil/leak.txt")
                .symlink_metadata()
                .is_err()
        );
    }

    /// A Gemini account with a home in the test folder: a login file, a
    /// settings file, a history folder and one skill of its own.
    #[cfg(unix)]
    fn gemini_account(dir: &Path) -> (AccountConfig, PathBuf) {
        let home = dir.join("profile/gemini");
        let gemini = home.join(".gemini");
        std::fs::create_dir_all(gemini.join("tmp/abc")).unwrap();
        std::fs::create_dir_all(gemini.join("skills/mine")).unwrap();
        std::fs::write(gemini.join("skills/mine/SKILL.md"), "mine").unwrap();
        // Stand-ins. The test never reads them as a login.
        std::fs::write(gemini.join("oauth_creds.json"), "creds-v1").unwrap();
        std::fs::write(gemini.join("settings.json"), "{}").unwrap();
        std::fs::create_dir_all(home.join(".agents/skills/agent-owned")).unwrap();
        std::fs::write(home.join(".agents/skills/agent-owned/SKILL.md"), "a").unwrap();
        let mut account = account(HarnessKind::GeminiCli);
        account.runtime.env.insert(
            "GEMINI_CLI_HOME".into(),
            home.to_string_lossy().into_owned(),
        );
        (account, home)
    }

    #[cfg(unix)]
    fn is_link(path: &Path) -> bool {
        path.symlink_metadata().unwrap().file_type().is_symlink()
    }

    #[cfg(unix)]
    #[test]
    fn gemini_gets_its_own_home_that_links_the_account_files() {
        let dir = tempfile::tempdir().unwrap();
        let (account, home) = gemini_account(dir.path());
        let skills = [cached(dir.path(), "pdf-forms")];
        let root = dir.path().join("runtime/ses_1/skills");
        let written = write_session_skills(&account, &root, &skills)
            .unwrap()
            .unwrap();
        let overlay = root.join("gemini-home");
        assert_eq!(
            written.env["GEMINI_CLI_HOME"],
            overlay.to_string_lossy().as_ref()
        );
        let gemini = overlay.join(".gemini");
        // The login, the settings and the history are links into the account.
        for name in ["oauth_creds.json", "settings.json", "tmp"] {
            assert!(is_link(&gemini.join(name)), "{name}");
        }
        // The skills folder is real. It holds the account's own skills, as
        // links, and the Consort skills, as copies.
        let skills_dir = gemini.join("skills");
        assert!(!is_link(&skills_dir));
        assert!(is_link(&skills_dir.join("mine")));
        assert!(skills_dir.join("pdf-forms/SKILL.md").is_file());
        assert!(!is_link(&skills_dir.join("pdf-forms")));
        assert!(
            overlay
                .join(".agents/skills/agent-owned/SKILL.md")
                .is_file()
        );
        // The account folder gained nothing.
        assert!(!home.join(".gemini/skills/pdf-forms").exists());

        // A token that Gemini refreshes in the session lands in the account.
        std::fs::write(gemini.join("oauth_creds.json"), "creds-v2").unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join(".gemini/oauth_creds.json")).unwrap(),
            "creds-v2"
        );
        // Something that Gemini makes for the first time stays too.
        std::fs::write(gemini.join("trustedFolders.json"), "{}").unwrap();
        std::fs::create_dir_all(gemini.join("history")).unwrap();
        std::fs::write(gemini.join("history/h.txt"), "h").unwrap();
        // A file that Gemini writes with a rename replaces our link. The
        // new content must reach the account.
        std::fs::remove_file(gemini.join("settings.json")).unwrap();
        std::fs::write(gemini.join("settings.json"), "{\"changed\":1}").unwrap();

        finish_session_skills(&written).unwrap();
        assert!(!root.exists(), "the session folder is gone");
        assert_eq!(
            std::fs::read_to_string(home.join(".gemini/trustedFolders.json")).unwrap(),
            "{}"
        );
        assert_eq!(
            std::fs::read_to_string(home.join(".gemini/history/h.txt")).unwrap(),
            "h"
        );
        assert_eq!(
            std::fs::read_to_string(home.join(".gemini/settings.json")).unwrap(),
            "{\"changed\":1}",
            "a file that replaced a link goes back to the account"
        );
        assert!(home.join(".gemini/skills/mine/SKILL.md").is_file());
        assert!(!home.join(".gemini/skills/pdf-forms").exists());
        assert!(!home.join(".gemini/skills/skills").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_consort_skill_replaces_an_account_skill_of_the_same_name_in_the_session_only() {
        let dir = tempfile::tempdir().unwrap();
        let (account, home) = gemini_account(dir.path());
        let skills = [cached(dir.path(), "mine")];
        let root = dir.path().join("runtime/ses_1/skills");
        write_session_skills(&account, &root, &skills).unwrap();
        let in_session =
            std::fs::read_to_string(root.join("gemini-home/.gemini/skills/mine/SKILL.md")).unwrap();
        assert!(in_session.contains("name: mine"), "{in_session}");
        assert_eq!(
            std::fs::read_to_string(home.join(".gemini/skills/mine/SKILL.md")).unwrap(),
            "mine",
            "the account's own skill is not written"
        );
    }

    #[cfg(unix)]
    #[test]
    fn two_gemini_sessions_do_not_share_skills() {
        let dir = tempfile::tempdir().unwrap();
        let (account, _home) = gemini_account(dir.path());
        let one = write_session_skills(
            &account,
            &dir.path().join("runtime/ses_1/skills"),
            &[cached(dir.path(), "one")],
        )
        .unwrap()
        .unwrap();
        let two = write_session_skills(
            &account,
            &dir.path().join("runtime/ses_2/skills"),
            &[cached(dir.path(), "two")],
        )
        .unwrap()
        .unwrap();
        let skills_of = |s: &SessionSkills| {
            let mut names: Vec<String> =
                std::fs::read_dir(s.root.join("gemini-home/.gemini/skills"))
                    .unwrap()
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
            names.sort();
            names
        };
        assert_eq!(skills_of(&one), ["mine", "one"]);
        assert_eq!(skills_of(&two), ["mine", "two"]);
        finish_session_skills(&one).unwrap();
        assert!(two.root.exists(), "the other session keeps its folder");
    }
}
