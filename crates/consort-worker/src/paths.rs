//! Path containment. A path from a tool call is relative to the workspace, and
//! the worker checks it with the rules of its own system.
//!
//! The lexical check refuses what can leave the workspace by its text alone.
//! The Windows rules also refuse what a lexical check on Unix would pass:
//! `C:foo.txt`, `a.txt::$DATA`, `NUL`, `con.txt` and `a.txt.`. Then
//! [`contain`] resolves the deepest folder that exists, so a symlink or a
//! junction cannot lead out.

use std::path::{Component, Path, PathBuf};

/// The rules of one system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRules {
    /// macOS and Linux.
    Unix,
    /// Windows.
    Windows,
}

impl PathRules {
    /// The rules of this machine.
    pub const fn native() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

/// Why a path is refused. The text tells the model what to do.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// An absolute path, or a Windows path with a drive or a server.
    #[error(
        "`{0}` is an absolute path. Give a path relative to the workspace root that workspace_inspect shows."
    )]
    Absolute(String),
    /// A path with `..`.
    #[error("`{0}` leaves the workspace. Give a path inside the workspace.")]
    Leaves(String),
    /// A name that Windows reads as something else.
    #[error("`{0}` has a name that Windows does not allow: {1}. Choose another name.")]
    WindowsName(String, &'static str),
    /// A symlink or a junction leads out of the workspace.
    #[error("`{0}` resolves to a folder outside the workspace. Give a path inside the workspace.")]
    Outside(String),
    /// An empty path where a file is needed.
    #[error("The path is empty. Give a path relative to the workspace root.")]
    Empty,
    /// The workspace root is gone.
    #[error("The workspace root does not exist on this worker.")]
    NoRoot,
}

const WINDOWS_DEVICES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

fn check_windows_segment(given: &str, segment: &str) -> Result<(), PathError> {
    let bad = |why| Err(PathError::WindowsName(given.to_owned(), why));
    if segment.contains(':') {
        return bad("a colon opens a drive or a data stream");
    }
    if segment
        .chars()
        .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return bad("it has a character that Windows refuses");
    }
    if segment.ends_with('.') || segment.ends_with(' ') {
        return bad("Windows removes a dot or a space at the end of a name");
    }
    let stem = segment.split('.').next().unwrap_or(segment);
    if WINDOWS_DEVICES.contains(&stem.to_ascii_lowercase().as_str()) {
        return bad("it is the name of a device");
    }
    Ok(())
}

/// Checks the text of a relative path and returns it as a relative `PathBuf`.
pub fn lexical(given: &str, rules: PathRules) -> Result<PathBuf, PathError> {
    // No trim: a space at the end means something to Windows.
    let text = given;
    if text.trim().is_empty() {
        return Err(PathError::Empty);
    }
    let separators: &[char] = match rules {
        PathRules::Unix => &['/'],
        PathRules::Windows => &['/', '\\'],
    };
    if text.starts_with(separators) {
        return Err(PathError::Absolute(given.to_owned()));
    }
    if rules == PathRules::Windows {
        let bytes = text.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' {
            return Err(PathError::Absolute(given.to_owned()));
        }
    }
    let mut out = PathBuf::new();
    for segment in text.split(separators) {
        match segment {
            "" | "." => {}
            ".." => return Err(PathError::Leaves(given.to_owned())),
            other => {
                if rules == PathRules::Windows {
                    check_windows_segment(given, other)?;
                }
                out.push(other);
            }
        }
    }
    Ok(out)
}

/// The real path inside `root` for a relative path. The file need not exist.
/// The deepest part that exists must resolve to a place inside the real root.
pub fn contain(root: &Path, given: &str, rules: PathRules) -> Result<PathBuf, PathError> {
    let relative = lexical(given, rules)?;
    let real_root = std::fs::canonicalize(root).map_err(|_| PathError::NoRoot)?;
    let mut existing = real_root.clone();
    let mut rest = Vec::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(PathError::Leaves(given.to_owned()));
        };
        if !rest.is_empty() {
            rest.push(name.to_owned());
            continue;
        }
        let next = existing.join(name);
        match std::fs::symlink_metadata(&next) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let real = std::fs::canonicalize(&next)
                    .map_err(|_| PathError::Outside(given.to_owned()))?;
                if !real.starts_with(&real_root) {
                    return Err(PathError::Outside(given.to_owned()));
                }
                existing = real;
            }
            Ok(_) => {
                let real = std::fs::canonicalize(&next)
                    .map_err(|_| PathError::Outside(given.to_owned()))?;
                if !real.starts_with(&real_root) {
                    return Err(PathError::Outside(given.to_owned()));
                }
                existing = real;
            }
            Err(_) => rest.push(name.to_owned()),
        }
    }
    for name in rest {
        existing.push(name);
    }
    Ok(existing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_rules_refuse_absolute_and_parent_paths() {
        assert!(matches!(
            lexical("/etc/passwd", PathRules::Unix),
            Err(PathError::Absolute(_))
        ));
        assert!(matches!(
            lexical("a/../../b", PathRules::Unix),
            Err(PathError::Leaves(_))
        ));
        assert_eq!(
            lexical("a/./b", PathRules::Unix).unwrap(),
            PathBuf::from("a").join("b")
        );
        assert!(matches!(
            lexical("  ", PathRules::Unix),
            Err(PathError::Empty)
        ));
    }

    /// The names that `t10-misc` found a lexical check to pass.
    #[test]
    fn windows_rules_refuse_what_a_unix_check_passes() {
        for bad in [
            "C:foo.txt",
            "C:\\foo.txt",
            "\\\\server\\share",
            "\\foo",
            "a.txt::$DATA",
            "NUL",
            "con.txt",
            "dir\\COM1.log",
            "a.txt.",
            "a.txt ",
            "a|b",
        ] {
            assert!(lexical(bad, PathRules::Windows).is_err(), "{bad}");
        }
        assert!(lexical("..\\x", PathRules::Windows).is_err());
        assert_eq!(
            lexical("dir\\file.txt", PathRules::Windows).unwrap(),
            PathBuf::from("dir").join("file.txt")
        );
        assert!(lexical("console.txt", PathRules::Windows).is_ok());
    }

    #[test]
    fn a_path_inside_the_root_is_contained() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            contain(dir.path(), "a/new.txt", PathRules::Unix).unwrap(),
            root.join("a").join("new.txt")
        );
        assert_eq!(
            contain(dir.path(), "b/c/new.txt", PathRules::Unix).unwrap(),
            root.join("b").join("c").join("new.txt")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_that_leads_out_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("out")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), dir.path().join("f")).unwrap();
        assert!(matches!(
            contain(dir.path(), "out/secret", PathRules::Unix),
            Err(PathError::Outside(_))
        ));
        assert!(matches!(
            contain(dir.path(), "f", PathRules::Unix),
            Err(PathError::Outside(_))
        ));
        assert!(matches!(
            contain(dir.path(), "out/new", PathRules::Unix),
            Err(PathError::Outside(_))
        ));
        // A link inside the workspace is fine.
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("in")).unwrap();
        assert!(contain(dir.path(), "in/new", PathRules::Unix).is_ok());
    }
}
