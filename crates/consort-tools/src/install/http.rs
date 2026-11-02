//! The HTTP client of the tool store.

use std::path::Path;
use std::time::Duration;

use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::ToolError;

/// The time limit for small requests: version files, checksums, manifests.
const SMALL_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub(crate) struct Http {
    client: reqwest::Client,
}

/// The result of a download.
pub(crate) struct Downloaded {
    /// The SHA-256 of the file, lower-case hex.
    pub sha256: String,
}

fn network_error(url: &str, err: impl std::fmt::Display) -> ToolError {
    ToolError::Network {
        url: url.to_owned(),
        message: err.to_string(),
    }
}

impl Http {
    pub(crate) fn new() -> Result<Self, ToolError> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("consort-worker/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(30))
            // Large downloads have no total limit, but a stalled one stops.
            .read_timeout(Duration::from_secs(120))
            .build()
            .map_err(|err| network_error("(client setup)", err))?;
        Ok(Http { client })
    }

    async fn send(
        &self,
        url: &str,
        timeout: Option<Duration>,
    ) -> Result<reqwest::Response, ToolError> {
        let mut request = self.client.get(url);
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let response = request
            .send()
            .await
            .map_err(|err| network_error(url, err))?;
        let status = response.status();
        if !status.is_success() {
            return Err(network_error(url, format!("HTTP status {status}")));
        }
        Ok(response)
    }

    pub(crate) async fn bytes(&self, url: &str) -> Result<Vec<u8>, ToolError> {
        let response = self.send(url, Some(SMALL_REQUEST_TIMEOUT)).await?;
        let bytes = response
            .bytes()
            .await
            .map_err(|err| network_error(url, err))?;
        Ok(bytes.to_vec())
    }

    pub(crate) async fn text(&self, url: &str) -> Result<String, ToolError> {
        let bytes = self.bytes(url).await?;
        String::from_utf8(bytes).map_err(|err| ToolError::InvalidResponse {
            url: url.to_owned(),
            message: err.to_string(),
        })
    }

    pub(crate) async fn json<T: DeserializeOwned>(&self, url: &str) -> Result<T, ToolError> {
        let bytes = self.bytes(url).await?;
        serde_json::from_slice(&bytes).map_err(|err| ToolError::InvalidResponse {
            url: url.to_owned(),
            message: err.to_string(),
        })
    }

    /// Streams `url` into a new file at `dest` and hashes it on the way.
    pub(crate) async fn download(&self, url: &str, dest: &Path) -> Result<Downloaded, ToolError> {
        self.download_checked(url, dest, None).await
    }

    /// Like [`Http::download`], and checks the size while it streams. The
    /// download stops as soon as more than `bytes` arrive, so a wrong server
    /// cannot fill the disk. A shorter file fails at the end. `name` is the
    /// file name for the error message.
    pub(crate) async fn download_sized(
        &self,
        url: &str,
        dest: &Path,
        name: &str,
        bytes: u64,
    ) -> Result<Downloaded, ToolError> {
        self.download_checked(url, dest, Some((name, bytes))).await
    }

    async fn download_checked(
        &self,
        url: &str,
        dest: &Path,
        size: Option<(&str, u64)>,
    ) -> Result<Downloaded, ToolError> {
        let mismatch = |(name, expected): (&str, u64), actual: u64| ToolError::SizeMismatch {
            name: name.to_owned(),
            expected,
            actual,
        };
        let mut response = self.send(url, None).await?;
        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(ToolError::io(format!("cannot create {}", dest.display())))?;
        let mut hasher = Sha256::new();
        let mut received: u64 = 0;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| network_error(url, err))?
        {
            received += chunk.len() as u64;
            if let Some(size) = size
                && received > size.1
            {
                return Err(mismatch(size, received));
            }
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(ToolError::io(format!("cannot write {}", dest.display())))?;
        }
        file.flush()
            .await
            .map_err(ToolError::io(format!("cannot write {}", dest.display())))?;
        if let Some(size) = size
            && received != size.1
        {
            return Err(mismatch(size, received));
        }
        Ok(Downloaded {
            sha256: format!("{:x}", hasher.finalize()),
        })
    }
}

/// Fails with [`ToolError::ChecksumMismatch`] unless the hex strings match
/// (case does not matter).
pub(crate) fn check_sha256(name: &str, expected: &str, actual: &str) -> Result<(), ToolError> {
    if expected.trim().eq_ignore_ascii_case(actual) {
        Ok(())
    } else {
        Err(ToolError::ChecksumMismatch {
            name: name.to_owned(),
            expected: expected.trim().to_ascii_lowercase(),
            actual: actual.to_owned(),
        })
    }
}

/// Finds the checksum of `file` in a `SHA256SUMS` style text
/// (`<hex>  <name>` lines, the name may have a `*` before it).
pub(crate) fn find_checksum(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == file && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_checksums() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums =
            format!("{a}  node-v1-darwin-arm64.tar.gz\n{b} *node-v1-win-x64.zip\nbad line\n");
        assert_eq!(find_checksum(&sums, "node-v1-darwin-arm64.tar.gz"), Some(a));
        assert_eq!(
            find_checksum(&sums, "node-v1-win-x64.zip"),
            Some("b".repeat(64))
        );
        assert_eq!(find_checksum(&sums, "node-v1-linux-x64.tar.gz"), None);
    }

    #[test]
    fn compares_checksums_without_case() {
        check_sha256("x", "ABC", "abc").unwrap();
        assert!(matches!(
            check_sha256("x", "abc", "abd"),
            Err(ToolError::ChecksumMismatch { .. })
        ));
    }
}
