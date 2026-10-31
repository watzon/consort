//! The file tools. Each works inside one workspace root, after a check with
//! [`crate::paths::contain`]. These functions block, so call them from a
//! blocking task.

use std::path::{Path, PathBuf};

use consort_proto::execution_link::MAX_READ_BYTES;
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::paths::{PathRules, contain};

/// The most entries that `fs_list` returns.
const MAX_LIST: usize = 5_000;
/// The most lines that `fs_search` returns by default.
const DEFAULT_SEARCH_RESULTS: usize = 200;
/// The largest file that `fs_search` reads.
const MAX_SEARCH_FILE: u64 = 2 * 1024 * 1024;
/// A folder that the walks skip.
const SKIP: [&str; 2] = [".git", "node_modules"];

/// A tool that refused or failed. The model reads the code and the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    /// A code that does not change, such as `sha256_mismatch`.
    pub code: &'static str,
    /// What happened and what to do next.
    pub message: String,
}

impl ToolError {
    /// A new error.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The result that the model sees.
    pub fn to_value(&self) -> Value {
        json!({"error_code": self.code, "message": self.message})
    }
}

/// What a change left: the path and the hash that a replay checks.
pub type Check = Option<(String, String)>;

/// The result of a tool, and what a replay checks.
pub type ToolOutcome = Result<(Value, Check), ToolError>;

/// The hex SHA-256 of some bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn str_arg<'a>(args: &'a Value, name: &str) -> Result<&'a str, ToolError> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new("invalid_arguments", format!("`{name}` is required.")))
}

fn resolve(root: &Path, given: &str, rules: PathRules) -> Result<PathBuf, ToolError> {
    contain(root, given, rules).map_err(|e| ToolError::new("path_refused", e.to_string()))
}

fn optional_path(root: &Path, args: &Value, rules: PathRules) -> Result<PathBuf, ToolError> {
    match args.get("path").and_then(Value::as_str) {
        Some(given) if !given.trim().is_empty() && given.trim() != "." => {
            resolve(root, given, rules)
        }
        _ => std::fs::canonicalize(root).map_err(|e| {
            ToolError::new(
                "workspace_missing",
                format!("The workspace root is not there: {e}"),
            )
        }),
    }
}

fn io_error(what: &str, path: &str, err: &std::io::Error) -> ToolError {
    let code = if err.kind() == std::io::ErrorKind::NotFound {
        "not_found"
    } else {
        "io_error"
    };
    ToolError::new(code, format!("Could not {what} `{path}`: {err}"))
}

/// `fs_list`.
pub fn fs_list(root: &Path, args: &Value, rules: PathRules) -> Result<Value, ToolError> {
    let dir = optional_path(root, args, rules)?;
    let recursive = args
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let shown = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let mut entries = Vec::new();
    let mut truncated = false;
    let mut stack = vec![(dir, String::new())];
    while let Some((folder, prefix)) = stack.pop() {
        let read = std::fs::read_dir(&folder).map_err(|e| io_error("list", shown, &e))?;
        let mut names: Vec<_> = read.flatten().collect();
        names.sort_by_key(std::fs::DirEntry::file_name);
        for entry in names {
            if entries.len() >= MAX_LIST {
                truncated = true;
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = entry.path().symlink_metadata() else {
                continue;
            };
            let kind = if meta.file_type().is_symlink() {
                "link"
            } else if meta.is_dir() {
                "folder"
            } else {
                "file"
            };
            let path = format!("{prefix}{name}");
            entries.push(json!({"path": path, "kind": kind, "size": meta.len()}));
            if recursive && meta.is_dir() && !SKIP.contains(&name.as_str()) {
                stack.push((entry.path(), format!("{path}/")));
            }
        }
    }
    Ok(json!({"entries": entries, "truncated": truncated}))
}

/// `fs_stat`.
pub fn fs_stat(root: &Path, args: &Value, rules: PathRules) -> Result<Value, ToolError> {
    let given = str_arg(args, "path")?;
    let path = resolve(root, given, rules)?;
    let meta = std::fs::metadata(&path).map_err(|e| io_error("read", given, &e))?;
    let mut out = json!({
        "path": given,
        "kind": if meta.is_dir() { "folder" } else { "file" },
        "size": meta.len(),
    });
    if meta.is_file() && meta.len() <= MAX_READ_BYTES * 8 {
        let bytes = std::fs::read(&path).map_err(|e| io_error("read", given, &e))?;
        out["sha256"] = json!(sha256_hex(&bytes));
    }
    Ok(out)
}

/// `fs_read`.
pub fn fs_read(root: &Path, args: &Value, rules: PathRules) -> Result<Value, ToolError> {
    let given = str_arg(args, "path")?;
    let path = resolve(root, given, rules)?;
    let bytes = std::fs::read(&path).map_err(|e| io_error("read", given, &e))?;
    let offset = args
        .get("offset")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(bytes.len() as u64) as usize;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(MAX_READ_BYTES)
        .min(MAX_READ_BYTES) as usize;
    let end = offset.saturating_add(limit).min(bytes.len());
    let part = &bytes[offset..end];
    if part.contains(&0) {
        return Err(ToolError::new(
            "binary_file",
            format!(
                "`{given}` is not a text file. Use fs_stat to see its size, or a command to read it."
            ),
        ));
    }
    Ok(json!({
        "path": given,
        "content": String::from_utf8_lossy(part),
        "size": bytes.len(),
        "offset": offset,
        "truncated": end < bytes.len(),
        "sha256": sha256_hex(&bytes),
    }))
}

/// `fs_search`.
pub fn fs_search(root: &Path, args: &Value, rules: PathRules) -> Result<Value, ToolError> {
    let pattern = str_arg(args, "pattern")?;
    let regex = Regex::new(pattern).map_err(|e| {
        ToolError::new(
            "invalid_arguments",
            format!("The pattern is not valid: {e}"),
        )
    })?;
    let start = optional_path(root, args, rules)?;
    let real_root = std::fs::canonicalize(root)
        .map_err(|e| ToolError::new("workspace_missing", e.to_string()))?;
    let max = args
        .get("max_results")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_SEARCH_RESULTS, |n| n as usize)
        .min(2_000);
    let mut matches = Vec::new();
    let mut stack = vec![start];
    'walk: while let Some(path) = stack.pop() {
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            let Ok(read) = std::fs::read_dir(&path) else {
                continue;
            };
            let mut names: Vec<_> = read.flatten().collect();
            names.sort_by_key(std::fs::DirEntry::file_name);
            for entry in names.into_iter().rev() {
                let name = entry.file_name();
                if !SKIP.iter().any(|s| *s == name) {
                    stack.push(entry.path());
                }
            }
            continue;
        }
        if meta.len() > MAX_SEARCH_FILE {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let shown = path
            .strip_prefix(&real_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        for (index, line) in text.lines().enumerate() {
            if regex.is_match(line) {
                if matches.len() >= max {
                    return Ok(json!({"matches": matches, "truncated": true}));
                }
                matches.push(json!({"path": shown, "line": index + 1, "text": line}));
                if matches.len() >= max {
                    break 'walk;
                }
            }
        }
    }
    Ok(json!({"matches": matches, "truncated": false}))
}

/// Writes a whole file, by way of a new file next to it, so a reader never
/// sees half of it.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", uuid::Uuid::now_v7().simple()));
    std::fs::write(&temp, bytes)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&temp, meta.permissions());
    }
    if let Err(err) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(err);
    }
    Ok(())
}

/// `fs_apply_patch`. All the edits apply, or none.
pub fn fs_apply_patch(root: &Path, args: &Value, rules: PathRules) -> ToolOutcome {
    let given = str_arg(args, "path")?;
    let expected = str_arg(args, "expected_sha256")?;
    let path = resolve(root, given, rules)?;
    let bytes = std::fs::read(&path).map_err(|e| io_error("read", given, &e))?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(ToolError::new(
            "sha256_mismatch",
            format!(
                "`{given}` changed since you read it: its sha256 is now {actual}. Read it again with fs_read, then make the patch again with a new operation_id."
            ),
        ));
    }
    let mut text = String::from_utf8(bytes).map_err(|_| {
        ToolError::new(
            "binary_file",
            format!("`{given}` is not UTF-8 text, so it cannot take a patch."),
        )
    })?;
    let edits = args
        .get("edits")
        .and_then(Value::as_array)
        .filter(|e| !e.is_empty())
        .ok_or_else(|| ToolError::new("invalid_arguments", "`edits` needs at least one edit."))?;
    for (index, edit) in edits.iter().enumerate() {
        let old = str_arg(edit, "old")?;
        let new = str_arg(edit, "new")?;
        if old.is_empty() {
            return Err(ToolError::new(
                "invalid_arguments",
                format!("Edit {index} has an empty `old`."),
            ));
        }
        match text.matches(old).count() {
            1 => text = text.replacen(old, new, 1),
            0 => {
                return Err(ToolError::new(
                    "edit_not_found",
                    format!(
                        "Edit {index}: the text of `old` is not in `{given}`. Read the file again."
                    ),
                ));
            }
            n => {
                return Err(ToolError::new(
                    "edit_not_unique",
                    format!(
                        "Edit {index}: the text of `old` is in `{given}` {n} times. Give more of the text around it."
                    ),
                ));
            }
        }
    }
    write_atomic(&path, text.as_bytes()).map_err(|e| io_error("write", given, &e))?;
    let hash = sha256_hex(text.as_bytes());
    Ok((
        json!({"path": given, "sha256": hash, "edits": edits.len()}),
        Some((given.to_owned(), hash)),
    ))
}

/// `fs_write`.
pub fn fs_write(root: &Path, args: &Value, rules: PathRules) -> ToolOutcome {
    let given = str_arg(args, "path")?;
    let content = str_arg(args, "content")?;
    let path = resolve(root, given, rules)?;
    match std::fs::read(&path) {
        Ok(bytes) => {
            let actual = sha256_hex(&bytes);
            let expected = args.get("expected_sha256").and_then(Value::as_str);
            if expected != Some(actual.as_str()) {
                return Err(ToolError::new(
                    "sha256_mismatch",
                    format!(
                        "`{given}` exists and its sha256 is {actual}. Read it with fs_read, then pass that hash as expected_sha256 with a new operation_id."
                    ),
                ));
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(io_error("read", given, &err)),
    }
    write_atomic(&path, content.as_bytes()).map_err(|e| io_error("write", given, &e))?;
    let hash = sha256_hex(content.as_bytes());
    Ok((
        json!({"path": given, "sha256": hash, "size": content.len()}),
        Some((given.to_owned(), hash)),
    ))
}

/// `fs_move`.
pub fn fs_move(root: &Path, args: &Value, rules: PathRules) -> ToolOutcome {
    let from = str_arg(args, "from")?;
    let to = str_arg(args, "to")?;
    let source = resolve(root, from, rules)?;
    let target = resolve(root, to, rules)?;
    if target.symlink_metadata().is_ok() {
        return Err(ToolError::new(
            "already_exists",
            format!("`{to}` exists. Choose another path, or remove it first."),
        ));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_error("write", to, &e))?;
    }
    std::fs::rename(&source, &target).map_err(|e| io_error("move", from, &e))?;
    Ok((json!({"from": from, "to": to}), None))
}

/// `fs_remove`.
pub fn fs_remove(root: &Path, args: &Value, rules: PathRules) -> ToolOutcome {
    let given = str_arg(args, "path")?;
    let path = resolve(root, given, rules)?;
    let real_root = std::fs::canonicalize(root)
        .map_err(|e| ToolError::new("workspace_missing", e.to_string()))?;
    if path == real_root {
        return Err(ToolError::new(
            "path_refused",
            "The workspace root cannot be removed.",
        ));
    }
    let meta = path
        .symlink_metadata()
        .map_err(|e| io_error("remove", given, &e))?;
    if meta.is_dir() {
        if !args
            .get("recursive")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            std::fs::remove_dir(&path).map_err(|e| {
                ToolError::new(
                    "folder_not_empty",
                    format!(
                        "Could not remove the folder `{given}`: {e}. Pass recursive to remove what it holds."
                    ),
                )
            })?;
        } else {
            std::fs::remove_dir_all(&path).map_err(|e| io_error("remove", given, &e))?;
        }
    } else {
        std::fs::remove_file(&path).map_err(|e| io_error("remove", given, &e))?;
    }
    Ok((json!({"path": given, "removed": true}), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: PathRules = PathRules::Unix;

    #[test]
    fn a_patch_needs_the_hash_from_the_read_and_applies_all_or_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one two").unwrap();
        let read = fs_read(dir.path(), &json!({"path": "a.txt"}), R).unwrap();
        assert_eq!(read["content"], "one two");
        let hash = read["sha256"].as_str().unwrap();
        let wrong = fs_apply_patch(
            dir.path(),
            &json!({"path": "a.txt", "expected_sha256": "00", "edits": [{"old": "one", "new": "1"}]}),
            R,
        )
        .unwrap_err();
        assert_eq!(wrong.code, "sha256_mismatch");
        let partial = fs_apply_patch(
            dir.path(),
            &json!({"path": "a.txt", "expected_sha256": hash, "edits": [{"old": "one", "new": "1"}, {"old": "nope", "new": "x"}]}),
            R,
        )
        .unwrap_err();
        assert_eq!(partial.code, "edit_not_found");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one two"
        );
        let (result, check) = fs_apply_patch(
            dir.path(),
            &json!({"path": "a.txt", "expected_sha256": hash, "edits": [{"old": "one", "new": "1"}]}),
            R,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "1 two"
        );
        assert_eq!(check.unwrap().1, result["sha256"].as_str().unwrap());
    }

    #[test]
    fn a_write_to_a_file_that_exists_needs_its_hash() {
        let dir = tempfile::tempdir().unwrap();
        fs_write(dir.path(), &json!({"path": "d/n.txt", "content": "x"}), R).unwrap();
        let err = fs_write(dir.path(), &json!({"path": "d/n.txt", "content": "y"}), R).unwrap_err();
        assert_eq!(err.code, "sha256_mismatch");
        fs_write(
            dir.path(),
            &json!({"path": "d/n.txt", "content": "y", "expected_sha256": sha256_hex(b"x")}),
            R,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("d/n.txt")).unwrap(),
            "y"
        );
    }

    #[test]
    fn list_search_stat_move_and_remove_work_inside_the_root() {
        let dir = tempfile::tempdir().unwrap();
        fs_write(
            dir.path(),
            &json!({"path": "a/b.txt", "content": "hello\nworld\n"}),
            R,
        )
        .unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/x"), "hello").unwrap();
        let list = fs_list(dir.path(), &json!({"recursive": true}), R).unwrap();
        let paths: Vec<_> = list["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["path"].as_str().unwrap().to_owned())
            .collect();
        assert!(paths.contains(&"a/b.txt".to_owned()));
        assert!(!paths.iter().any(|p| p == ".git/x"));
        let found = fs_search(dir.path(), &json!({"pattern": "wor.d"}), R).unwrap();
        assert_eq!(found["matches"][0]["path"], "a/b.txt");
        assert_eq!(found["matches"][0]["line"], 2);
        assert_eq!(
            fs_search(dir.path(), &json!({"pattern": "hello"}), R).unwrap()["matches"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            fs_stat(dir.path(), &json!({"path": "a/b.txt"}), R).unwrap()["kind"],
            "file"
        );
        fs_move(dir.path(), &json!({"from": "a/b.txt", "to": "c.txt"}), R).unwrap();
        assert!(fs_remove(dir.path(), &json!({"path": "."}), R).is_err());
        assert_eq!(
            fs_remove(dir.path(), &json!({"path": "a"}), R).unwrap().0["removed"],
            true
        );
        fs_remove(dir.path(), &json!({"path": "c.txt"}), R).unwrap();
    }

    #[test]
    fn a_path_that_leaves_the_workspace_is_refused_with_a_clear_message() {
        let dir = tempfile::tempdir().unwrap();
        let err = fs_read(dir.path(), &json!({"path": "/etc/passwd"}), R).unwrap_err();
        assert_eq!(err.code, "path_refused");
        assert!(err.message.contains("workspace_inspect"));
    }
}
