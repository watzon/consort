//! A local HTTP server and fixture builders for the tool store tests.

#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use consort_tools::{
    Arch, ChromeFile, ChromePin, Libc, Os, Platform, Sources, ToolStore, ToolStoreOptions,
};
use sha2::{Digest, Sha256};

pub const TEST_KEY: &str = include_str!("../fixtures/test-signing-key.asc");
pub const TEST_MANIFEST: &[u8] = include_bytes!("../fixtures/test-claude-manifest.json");
pub const TEST_MANIFEST_SIG: &[u8] = include_bytes!("../fixtures/test-claude-manifest.json.sig");
/// The fake Claude binary. Its SHA-256 is in the test manifest.
pub const FAKE_CLAUDE: &[u8] = b"#!/bin/sh\necho fake claude 9.9.1\n";

pub const NODE_VERSION: &str = "24.1.0";
pub const CLAUDE_VERSION: &str = "9.9.1";
/// The version in the test pin of the preview browser.
pub const CHROME_VERSION: &str = "9.9.9.1";
/// The file name of the fake preview browser zip on the test platform.
pub const CHROME_ZIP: &str = "chrome-headless-shell-mac-arm64.zip";

#[derive(Clone, Default)]
struct Files {
    bodies: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    hits: Arc<Mutex<HashMap<String, usize>>>,
}

/// A local server that answers with the bodies that a test puts in it.
pub struct Server {
    pub base: String,
    files: Files,
}

async fn serve_file(State(files): State<Files>, uri: Uri) -> Response {
    let path = uri.path().to_owned();
    *files.hits.lock().unwrap().entry(path.clone()).or_insert(0) += 1;
    match files.bodies.lock().unwrap().get(&path) {
        Some(body) => (StatusCode::OK, body.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

impl Server {
    pub async fn start() -> Server {
        let files = Files::default();
        let app = Router::new().fallback(serve_file).with_state(files.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Server {
            base: format!("http://{addr}"),
            files,
        }
    }

    pub fn put(&self, path: &str, body: impl Into<Vec<u8>>) {
        self.files
            .bodies
            .lock()
            .unwrap()
            .insert(path.to_owned(), body.into());
    }

    pub fn hits(&self, path: &str) -> usize {
        self.files
            .hits
            .lock()
            .unwrap()
            .get(path)
            .copied()
            .unwrap_or(0)
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn sources(&self) -> Sources {
        Sources {
            node_dist: self.url("/node"),
            node_musl_dist: self.url("/node-musl"),
            claude_releases: self.url("/claude"),
            npm_registry: self.url("/npm"),
            codex_latest_release: self.url("/github/latest"),
            codex_downloads: self.url("/github/download"),
            acp_registry: self.url("/acp/registry.json"),
            chrome_for_testing: self.url("/chrome"),
            opencode_version: self.url("/opencode/version"),
        }
    }

    /// Node `NODE_VERSION` with a good checksum, as the newest 24 LTS.
    pub fn put_node(&self) {
        let archive = node_archive();
        self.put(
            "/node/index.json",
            r#"[{"version":"v26.0.0","lts":false},{"version":"v24.1.0","lts":"Krypton"},{"version":"v24.0.0","lts":"Krypton"}]"#,
        );
        let name = format!("node-v{NODE_VERSION}-darwin-arm64.tar.gz");
        self.put(
            &format!("/node/v{NODE_VERSION}/SHASUMS256.txt"),
            format!(
                "{}  {name}\n{}  node-v{NODE_VERSION}-win-x64.zip\n",
                sha256_hex(&archive),
                "0".repeat(64)
            ),
        );
        self.put(&format!("/node/v{NODE_VERSION}/{name}"), archive);
    }

    /// Claude Code `CLAUDE_VERSION`, signed with the test key.
    pub fn put_claude(&self) {
        self.put("/claude/stable", format!("{CLAUDE_VERSION}\n"));
        self.put(
            &format!("/claude/{CLAUDE_VERSION}/manifest.json"),
            TEST_MANIFEST,
        );
        self.put(
            &format!("/claude/{CLAUDE_VERSION}/manifest.json.sig"),
            TEST_MANIFEST_SIG,
        );
        self.put(
            &format!("/claude/{CLAUDE_VERSION}/darwin-arm64/claude"),
            FAKE_CLAUDE,
        );
    }

    /// A Codex release, and marks it as the latest.
    pub fn put_codex(&self, version: &str) {
        let archive = tar_gz(&[
            Entry::file(
                "codex-package.json",
                br#"{"layoutVersion":1,"entrypoint":"bin/codex","pathDir":"codex-path"}"#,
                0o644,
            ),
            Entry::file(
                "bin/codex",
                format!("#!/bin/sh\necho codex {version}\n").as_bytes(),
                0o755,
            ),
            Entry::file("codex-path/rg", b"#!/bin/sh\n", 0o755),
        ]);
        let asset = "codex-package-aarch64-apple-darwin.tar.gz";
        self.put(
            &format!("/github/download/rust-v{version}/codex-package_SHA256SUMS"),
            format!("{}  {asset}\n", sha256_hex(&archive)),
        );
        self.put(
            &format!("/github/download/rust-v{version}/{asset}"),
            archive,
        );
        self.set_codex_latest(version);
    }

    pub fn set_codex_latest(&self, version: &str) {
        self.put(
            "/github/latest",
            format!(r#"{{"tag_name":"rust-v{version}"}}"#),
        );
    }

    pub fn put_npm_latest(&self, package: &str, version: &str) {
        self.put(
            &format!("/npm/{}/latest", package.replace('/', "%2f")),
            format!(r#"{{"name":"{package}","version":"{version}"}}"#),
        );
    }

    /// The fake preview browser zip, at the path that the store asks for.
    pub fn put_chrome(&self) {
        self.put_chrome_at(CHROME_VERSION);
    }

    /// The fake preview browser zip as another version.
    pub fn put_chrome_at(&self, version: &str) {
        self.put(
            &format!("/chrome/{version}/mac-arm64/{CHROME_ZIP}"),
            chrome_zip(),
        );
    }

    /// A registry with test agents, and the files that they download.
    pub fn put_registry(&self) {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        zip.start_file("opencode", options).unwrap();
        zip.write_all(b"#!/bin/sh\necho opencode\n").unwrap();
        let opencode_zip = zip.finish().unwrap().into_inner();
        self.put("/files/opencode-darwin-arm64.zip", opencode_zip.clone());

        let evil = tar_gz(&[
            Entry::file("evil-agent", b"#!/bin/sh\n", 0o755),
            Entry::file("../evil.txt", b"evil", 0o644),
        ]);
        self.put("/files/evil.tar.gz", evil.clone());

        let raw = b"#!/bin/sh\necho raw\n".to_vec();
        self.put("/files/rawbin", raw.clone());
        self.put(
            "/files/nosha.tar.gz",
            tar_gz(&[Entry::file("nosha", b"x", 0o755)]),
        );

        let registry = serde_json_text(&[
            format!(
                r#"{{"id":"opencode-v2","name":"OpenCode V2","version":"2.2.1","description":"d",
                "icon":"https://example.invalid/opencode.svg",
                "distribution":{{"binary":{{"darwin-aarch64":{{"archive":"{}","cmd":"./opencode","args":["acp"],"sha256":"{}"}}}}}}}}"#,
                self.url("/files/opencode-darwin-arm64.zip"),
                sha256_hex(&opencode_zip)
            ),
            format!(
                r#"{{"id":"evil","name":"Evil","version":"1.0.0","description":"d",
                "distribution":{{"binary":{{"darwin-aarch64":{{"archive":"{}","cmd":"./evil-agent","sha256":"{}"}}}}}}}}"#,
                self.url("/files/evil.tar.gz"),
                sha256_hex(&evil)
            ),
            format!(
                r#"{{"id":"rawbin","name":"Raw","version":"1.0.0","description":"d",
                "distribution":{{"binary":{{"darwin-aarch64":{{"archive":"{}","cmd":"./rawbin","args":["serve"],"env":{{"RAW":"1"}},"sha256":"{}"}}}}}}}}"#,
                self.url("/files/rawbin"),
                sha256_hex(&raw)
            ),
            format!(
                r#"{{"id":"nosha","name":"No checksum","version":"1.0.0","description":"d",
                "distribution":{{"binary":{{"darwin-aarch64":{{"archive":"{}","cmd":"./nosha"}}}}}}}}"#,
                self.url("/files/nosha.tar.gz"),
            ),
            r#"{"id":"fake-npx","name":"Fake npx","version":"1.0.0","description":"d",
                "distribution":{"npx":{"package":"fake-npx-agent@1.0.0","args":["--acp"],"env":{"FOO":"bar"}}}}"#
                .to_owned(),
            r#"{"id":"broken-npx","name":"Broken npx","version":"1.0.0","description":"d",
                "distribution":{"npx":{"package":"fail-agent@1.0.0"}}}"#
                .to_owned(),
            r#"{"id":"uv-only","name":"uv","version":"1.0.0","description":"d",
                "distribution":{"uvx":{"package":"uv-only@1.0.0"}}}"#
                .to_owned(),
        ]);
        self.put("/acp/registry.json", registry);
    }
}

fn serde_json_text(agents: &[String]) -> String {
    format!(
        r#"{{"version":"1.0.0","agents":[{}],"extensions":[]}}"#,
        agents.join(",")
    )
}

/// One tar entry. The name bytes go into the header as they are, so tests
/// can build unsafe names.
pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
    pub mode: u32,
    pub link: Option<String>,
}

impl Entry {
    pub fn file(name: &str, data: &[u8], mode: u32) -> Entry {
        Entry {
            name: name.to_owned(),
            data: data.to_vec(),
            mode,
            link: None,
        }
    }

    pub fn symlink(name: &str, target: &str) -> Entry {
        Entry {
            name: name.to_owned(),
            data: Vec::new(),
            mode: 0o777,
            link: Some(target.to_owned()),
        }
    }
}

pub fn tar_gz(entries: &[Entry]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    for entry in entries {
        let mut header = tar::Header::new_gnu();
        let name = &mut header.as_old_mut().name;
        name[..entry.name.len()].copy_from_slice(entry.name.as_bytes());
        match &entry.link {
            Some(target) => {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_link_name(target).unwrap();
                header.set_size(0);
            }
            None => {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(entry.data.len() as u64);
            }
        }
        header.set_mode(entry.mode);
        header.set_cksum();
        builder.append(&header, entry.data.as_slice()).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

pub fn node_archive() -> Vec<u8> {
    let top = format!("node-v{NODE_VERSION}-darwin-arm64");
    tar_gz(&[
        Entry::file(
            &format!("{top}/bin/node"),
            b"#!/bin/sh\necho v24.1.0\n",
            0o755,
        ),
        Entry::file(
            &format!("{top}/lib/node_modules/npm/bin/npm-cli.js"),
            b"console.log('npm')\n",
            0o644,
        ),
        Entry::symlink(
            &format!("{top}/bin/npm"),
            "../lib/node_modules/npm/bin/npm-cli.js",
        ),
    ])
}

/// A fake preview browser zip with the layout of Chrome for Testing: one
/// top folder. The files have no mode bits for execution, as in a zip that
/// Windows made, so the store must make the program executable.
pub fn chrome_zip() -> Vec<u8> {
    static ZIP: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    ZIP.get_or_init(|| {
        let top = "chrome-headless-shell-mac-arm64";
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().unix_permissions(0o644);
        for (name, body) in [
            (
                "chrome-headless-shell",
                &b"#!/bin/sh\necho fake chrome\n"[..],
            ),
            ("icudtl.dat", b"icu"),
            ("locales/en-US.pak", b"pak"),
        ] {
            zip.start_file(format!("{top}/{name}"), options).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap().into_inner()
    })
    .clone()
}

/// The pin of the fake zip on the test platform.
pub fn chrome_pin() -> ChromePin {
    chrome_pin_at(CHROME_VERSION)
}

/// The pin of the fake zip as another version.
pub fn chrome_pin_at(version: &str) -> ChromePin {
    let zip = chrome_zip();
    ChromePin {
        version: version.to_owned(),
        files: vec![ChromeFile {
            platform: "mac-arm64".to_owned(),
            bytes: zip.len() as u64,
            sha256: sha256_hex(&zip),
        }],
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The platform of all store tests, so the asset names are fixed.
pub fn test_platform() -> Platform {
    Platform::new(Os::Macos, Arch::Arm64, Libc::System)
}

pub fn store(server: &Server, root: &Path, npm: Option<PathBuf>) -> ToolStore {
    store_with(server, root, npm, test_platform(), chrome_pin())
}

/// A store with its own platform and preview browser pin.
pub fn store_with(
    server: &Server,
    root: &Path,
    npm: Option<PathBuf>,
    platform: Platform,
    chrome: ChromePin,
) -> ToolStore {
    let options = ToolStoreOptions {
        platform,
        sources: server.sources(),
        npm_program: npm,
        claude_signing_key: TEST_KEY.to_owned(),
        chrome,
    };
    ToolStore::with_options(root.to_owned(), options).unwrap()
}

/// Writes a fake `npm` shell script. It logs its arguments to `log` and
/// writes a package into `--prefix`. A package named `fail-*` fails.
#[cfg(unix)]
pub fn fake_npm(dir: &Path, log: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = format!(
        r#"#!/bin/sh
echo "$@" >> "{log}"
prefix=""
spec=""
while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) prefix="$2"; shift 2 ;;
    install|--*) shift ;;
    *) spec="$1"; shift ;;
  esac
done
case "$spec" in
  @*) name="@$(echo "${{spec#@}}" | cut -d@ -f1)" ;;
  *) name="$(echo "$spec" | cut -d@ -f1)" ;;
esac
dir="$prefix/node_modules/$name"
case "$name" in
  fail-*) echo "npm ERR! 404 Not Found - $name" >&2; exit 1 ;;
esac
mkdir -p "$dir"
case "$name" in
  @agentclientprotocol/claude-agent-acp)
    mkdir -p "$dir/dist"; echo 'console.log("acp")' > "$dir/dist/index.js" ;;
  @google/gemini-cli)
    mkdir -p "$dir/bundle"; echo 'console.log("gemini")' > "$dir/bundle/gemini.js" ;;
  *)
    mkdir -p "$dir/bin"
    printf '#!/usr/bin/env node\nconsole.log(1)\n' > "$dir/bin/cli"
    printf '{{"name":"%s","bin":{{"%s":"bin/cli"}}}}' "$name" "$name" > "$dir/package.json" ;;
esac
"#,
        log = log.display()
    );
    let path = dir.join("fake-npm");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// The names in a folder, sorted.
pub fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}
