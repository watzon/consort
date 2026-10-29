//! `state.json` in the tools root: the result of the last update check, and
//! version comparison.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::ToolError;

/// The saved state of the tool store.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoreState {
    /// The time of the last full update check, in seconds since 1970.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check: Option<u64>,
    /// The last check result of each tool, by tool name.
    #[serde(default)]
    pub tools: BTreeMap<String, ToolCheck>,
}

/// The last update check result of one tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ToolCheck {
    pub latest: String,
    pub checked_at: u64,
}

impl StoreState {
    /// Reads the state. A missing or damaged file gives an empty state, so a
    /// bad file never stops the worker.
    pub(crate) fn load(path: &Path) -> StoreState {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Writes the state through a temporary file and a rename.
    pub(crate) fn save(&self, path: &Path) -> Result<(), ToolError> {
        let json = serde_json::to_vec_pretty(self).map_err(|err| ToolError::Io {
            context: "cannot encode state.json".to_owned(),
            source: std::io::Error::other(err),
        })?;
        let tmp = path.with_extension(format!("json.tmp-{:016x}", rand::random::<u64>()));
        fs::write(&tmp, json).map_err(ToolError::io(format!("cannot write {}", tmp.display())))?;
        fs::rename(&tmp, path).map_err(|err| {
            let _ = fs::remove_file(&tmp);
            ToolError::Io {
                context: format!("cannot write {}", path.display()),
                source: err,
            }
        })
    }
}

/// Seconds since 1970.
pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// True when `candidate` is a newer version than `installed`.
///
/// The release part (before `-` or `+`) is compared number by number. With
/// equal release parts, a version without a pre-release part is newer.
pub(crate) fn is_newer(candidate: &str, installed: &str) -> bool {
    let (c_release, c_pre) = split_version(candidate);
    let (i_release, i_pre) = split_version(installed);
    let c_parts = numbers(c_release);
    let i_parts = numbers(i_release);
    let len = c_parts.len().max(i_parts.len());
    for index in 0..len {
        let c = c_parts.get(index).copied().unwrap_or(0);
        let i = i_parts.get(index).copied().unwrap_or(0);
        if c != i {
            return c > i;
        }
    }
    match (c_pre, i_pre) {
        (None, Some(_)) => true,
        (Some(c), Some(i)) => c > i,
        _ => false,
    }
}

fn split_version(version: &str) -> (&str, Option<&str>) {
    let version = version.trim().trim_start_matches('v');
    let version = version.split('+').next().unwrap_or(version);
    match version.split_once('-') {
        Some((release, pre)) => (release, Some(pre)),
        None => (version, None),
    }
}

fn numbers(release: &str) -> Vec<u64> {
    release
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

/// True for a version text that is safe as a folder name.
pub(crate) fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && !version.starts_with('.')
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("1.2.4", "1.2.3"));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("v24.22.2", "24.22.1"));
        assert!(is_newer("1.0.0", "1.0.0-beta.1"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0.0-beta.1", "1.0.0"));
        assert!(!is_newer("2.1.308", "2.1.312"));
        assert!(is_newer("3000.11.228", "3000.11.9"));
    }

    #[test]
    fn checks_version_text() {
        assert!(valid_version("2.1.308"));
        assert!(valid_version("1.0.0-beta.1+build"));
        for bad in ["", "..", ".hidden", "1/2", "1\\2", "a b"] {
            assert!(!valid_version(bad), "{bad}");
        }
    }

    #[test]
    fn state_survives_a_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.json");
        assert_eq!(StoreState::load(&path), StoreState::default());
        let mut state = StoreState {
            last_check: Some(5),
            ..StoreState::default()
        };
        state.tools.insert(
            "codex".into(),
            ToolCheck {
                latest: "1.0.0".into(),
                checked_at: 5,
            },
        );
        state.save(&path).unwrap();
        assert_eq!(StoreState::load(&path), state);
        fs::write(&path, b"not json").unwrap();
        assert_eq!(StoreState::load(&path), StoreState::default());
    }
}
