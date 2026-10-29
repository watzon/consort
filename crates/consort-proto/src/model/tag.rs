//! Tags: short names on workers and channels, such as `work` or `gpu`.
//!
//! A worker has user tags (set in a client) and detected tags (from the
//! worker's inventory). A channel's tags are requirements: all work from the
//! channel runs only on a worker that has every tag of the channel.

use std::fmt;

use crate::error::{ErrorBody, ErrorCode};

/// The most characters in a tag.
pub const TAG_MAX_CHARS: usize = 32;

/// The error when a text is not a valid tag. Maps to an `invalid_tag` error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidTag {
    /// The text that was given.
    pub tag: String,
}

impl fmt::Display for InvalidTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.tag.is_empty() {
            f.write_str("A tag cannot be empty. ")?;
        } else {
            write!(f, "\"{}\" is not a valid tag. ", self.tag)?;
        }
        write!(
            f,
            "Use 1 to {TAG_MAX_CHARS} lowercase letters, digits or hyphens."
        )
    }
}

impl std::error::Error for InvalidTag {}

impl From<InvalidTag> for ErrorBody {
    fn from(err: InvalidTag) -> Self {
        ErrorBody::new(ErrorCode::InvalidTag, err.to_string())
    }
}

/// Whether `tag` is 1 to [`TAG_MAX_CHARS`] characters, each an ASCII
/// lowercase letter, an ASCII digit or `-`.
pub fn is_valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= TAG_MAX_CHARS
        && tag
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Checks one tag. See [`is_valid_tag`].
pub fn validate_tag(tag: &str) -> Result<(), InvalidTag> {
    if is_valid_tag(tag) {
        Ok(())
    } else {
        Err(InvalidTag {
            tag: tag.to_owned(),
        })
    }
}

/// Trims each tag, checks it, and drops repeats. The order of the first
/// occurrences stays. Fails on the first tag that is not valid.
pub fn normalize_tags<I, S>(tags: I) -> Result<Vec<String>, InvalidTag>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let tag = tag.as_ref().trim();
        validate_tag(tag)?;
        if !out.iter().any(|t| t == tag) {
            out.push(tag.to_owned());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_tags() {
        for tag in [
            "work",
            "gpu",
            "ram-16gb",
            "cpu-8",
            "gpu-nvidia",
            "x64",
            "a",
            "-",
            &"a".repeat(32),
        ] {
            assert!(is_valid_tag(tag), "{tag} should be valid");
            assert_eq!(validate_tag(tag), Ok(()));
        }
    }

    #[test]
    fn invalid_tags() {
        for tag in [
            "",
            "Work",
            "GPU",
            "two words",
            "under_score",
            "dot.ted",
            "ümlaut",
            " gpu",
            &"a".repeat(33),
        ] {
            assert!(!is_valid_tag(tag), "{tag:?} should be invalid");
            let err = validate_tag(tag).unwrap_err();
            assert_eq!(err.tag, tag);
        }
    }

    #[test]
    fn error_text_and_code() {
        let err = validate_tag("Work").unwrap_err();
        assert_eq!(
            err.to_string(),
            "\"Work\" is not a valid tag. Use 1 to 32 lowercase letters, digits or hyphens."
        );
        let empty = validate_tag("").unwrap_err();
        assert!(empty.to_string().starts_with("A tag cannot be empty."));
        let body: ErrorBody = err.into();
        assert_eq!(body.code, ErrorCode::InvalidTag);
    }

    #[test]
    fn normalize_trims_and_drops_repeats() {
        assert_eq!(
            normalize_tags([" gpu ", "work", "gpu"]).unwrap(),
            vec!["gpu".to_owned(), "work".to_owned()]
        );
        assert_eq!(
            normalize_tags(Vec::<String>::new()).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            normalize_tags(["work", "Bad Tag"]).unwrap_err().tag,
            "Bad Tag"
        );
        assert_eq!(normalize_tags(["  "]).unwrap_err().tag, "");
    }
}
