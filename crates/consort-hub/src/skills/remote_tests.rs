//! Skill sources beyond a public git repo: the branch of a directory entry,
//! private repos and ssh URLs, the marketplace plugin sources `archive`,
//! `npm` and `command`, and the time limits of git and HTTP with servers that
//! accept a connection and never answer. No test uses the real network.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use consort_proto::client_api::{
    SkillDirectoryAddSource, SkillDirectorySearch, SkillInstall, SkillInstallSource, SkillPreview,
};
use consort_proto::error::ErrorCode;
use consort_proto::model::{
    SkillDirectoryEntry, SkillSource, SkillUpdateState, UseIn, canonical_json,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::fake::{FakeDirectory, TestRepo, skill_md};
use super::git::{GitOptions, Repo, failure_hint, parse_git_url, remote_commit, set_test_home};
use super::remote::npm_integrity;
use super::tarball::tests::make_tgz;
use super::upload::tests::make_zip;
use crate::hub::{Caller, Hub, test_hub};

const REG: u32 = 0o100_644;

async fn hub() -> (Hub, tempfile::TempDir) {
    let (hub, dir) = test_hub().await;
    hub.skill_sources.set_config(|c| c.allow_local = true);
    (hub, dir)
}

// A server for archives and npm packages -----------------------------------------

#[derive(Default)]
struct PackageState {
    zip: Vec<u8>,
    tgz: Vec<u8>,
    /// The integrity that the registry gives. Empty means the right one.
    integrity: String,
    calls: BTreeMap<String, usize>,
}

type Shared = Arc<Mutex<PackageState>>;

struct PackageServer {
    origin: String,
    state: Shared,
}

fn hit(state: &Shared, what: &str) {
    *state
        .lock()
        .unwrap()
        .calls
        .entry(what.to_owned())
        .or_default() += 1;
}

async fn serve_zip(State(state): State<Shared>) -> Response {
    hit(&state, "zip");
    let zip = state.lock().unwrap().zip.clone();
    ([(header::CONTENT_TYPE, "application/zip")], zip).into_response()
}

async fn serve_huge() -> Response {
    // One byte over the limit of 25 MiB.
    (
        [(header::CONTENT_TYPE, "application/zip")],
        Body::from(vec![0_u8; 25 * 1024 * 1024 + 1]),
    )
        .into_response()
}

async fn serve_tgz(State(state): State<Shared>) -> Response {
    hit(&state, "tgz");
    let tgz = state.lock().unwrap().tgz.clone();
    ([(header::CONTENT_TYPE, "application/octet-stream")], tgz).into_response()
}

async fn serve_version(
    State(state): State<Shared>,
    UrlPath((package, version)): UrlPath<(String, String)>,
) -> Response {
    hit(&state, &format!("npm {package} {version}"));
    if package != "@acme/skills" {
        return (StatusCode::NOT_FOUND, "no").into_response();
    }
    let (tgz, integrity) = {
        let state = state.lock().unwrap();
        let right = npm_integrity(&state.tgz);
        let given = if state.integrity.is_empty() {
            right
        } else {
            state.integrity.clone()
        };
        (state.tgz.len(), given)
    };
    let _ = tgz;
    let origin = state.lock().unwrap().calls.get("origin").copied();
    let _ = origin;
    axum::Json(json!({
        "name": package,
        "version": if version == "latest" { "2.1.0" } else { version.as_str() },
        "dist": {"tarball": "TARBALL", "integrity": integrity},
    }))
    .into_response()
}

async fn serve_never() -> Response {
    std::future::pending::<()>().await;
    unreachable!()
}

impl PackageServer {
    async fn start() -> Self {
        let state: Shared = Arc::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        // The version document needs the server's own address, so the
        // handler is a closure over the origin.
        let tarball = format!("{origin}/files/package.tgz");
        let version = {
            let tarball = tarball.clone();
            move |state: State<Shared>, path: UrlPath<(String, String)>| {
                let tarball = tarball.clone();
                async move {
                    let response = serve_version(state, path).await;
                    if response.status() != StatusCode::OK {
                        return response;
                    }
                    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
                        .await
                        .unwrap();
                    let text = String::from_utf8(bytes.to_vec())
                        .unwrap()
                        .replace("TARBALL", &tarball);
                    ([(header::CONTENT_TYPE, "application/json")], text).into_response()
                }
            }
        };
        let app = Router::new()
            .route("/files/plugin.zip", get(serve_zip))
            .route("/files/huge.zip", get(serve_huge))
            .route("/files/package.tgz", get(serve_tgz))
            .route("/files/never.zip", get(serve_never))
            .route("/npm/{package}/{version}", get(version))
            .with_state(state.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { origin, state }
    }

    fn calls(&self, what: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .calls
            .get(what)
            .copied()
            .unwrap_or(0)
    }
}

/// The files of a plugin with a skill in `skills/pdf`.
fn plugin_files(version: &str) -> Vec<(String, Vec<u8>, u32)> {
    vec![
        ("package.json".to_owned(), b"{}".to_vec(), REG),
        (
            "skills/pdf/SKILL.md".to_owned(),
            skill_md("pdf", "")
                .replace("Steps.", &format!("Steps {version}."))
                .into_bytes(),
            REG,
        ),
        ("skills/pdf/notes.md".to_owned(), b"Notes.".to_vec(), REG),
    ]
}

fn zip_of(version: &str) -> Vec<u8> {
    let files = plugin_files(version);
    let entries: Vec<(String, &[u8], u32)> = files
        .iter()
        .map(|(p, b, m)| (format!("plugin/{p}"), b.as_slice(), *m))
        .collect();
    let refs: Vec<(&str, &[u8], u32)> = entries
        .iter()
        .map(|(p, b, m)| (p.as_str(), *b, *m))
        .collect();
    make_zip(&refs)
}

fn tgz_of(version: &str) -> Vec<u8> {
    let files = plugin_files(version);
    let entries: Vec<(String, &[u8], u32)> = files
        .iter()
        .map(|(p, b, _)| (format!("package/{p}"), b.as_slice(), 0o644))
        .collect();
    let refs: Vec<(&str, &[u8], u32)> = entries
        .iter()
        .map(|(p, b, m)| (p.as_str(), *b, *m))
        .collect();
    make_tgz(&refs)
}

async fn add_marketplace(
    hub: &Hub,
    directory: &FakeDirectory,
    plugins: Value,
) -> Vec<SkillDirectoryEntry> {
    directory.set_marketplace(json!({"name": "example", "plugins": plugins}));
    let url = format!("{}/marketplace.json", directory.url);
    hub.skill_directory_add_source(&Caller::Hub, SkillDirectoryAddSource { url: url.clone() })
        .await
        .unwrap();
    // Only this source: the built-in ones would reach the network.
    hub.skill_directory_search(
        &Caller::Hub,
        SkillDirectorySearch {
            search: None,
            source: Some(url),
            cursor: None,
            limit: Some(100),
        },
    )
    .await
    .unwrap()
    .entries
}

fn install_entry(entry: &SkillDirectoryEntry) -> SkillInstallSource {
    SkillInstallSource::Directory {
        source: entry.source_id.clone(),
        entry_id: entry.entry_id.clone(),
    }
}

async fn preview_error(hub: &Hub, source: SkillInstallSource) -> crate::error::HubError {
    hub.skill_preview(&Caller::Hub, SkillPreview { source })
        .await
        .unwrap_err()
}

// Archive ----------------------------------------------------------------------

#[tokio::test]
async fn an_archive_plugin_lists_previews_and_installs_with_its_hash_as_the_pin() {
    let (hub, _dir) = hub().await;
    let directory = FakeDirectory::start().await;
    let server = PackageServer::start().await;
    let zip = zip_of("one");
    let hash = crate::util::sha256(&zip);
    server.state.lock().unwrap().zip = zip;
    let entries = add_marketplace(
        &hub,
        &directory,
        json!([{"name": "docs", "skills": ["./skills/pdf"], "description": "Documents.",
                "source": {"source": "archive", "url": format!("{}/files/plugin.zip", server.origin),
                           "sha256": hash.as_str()}}]),
    )
    .await;
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.name, "pdf");
    assert!(
        entry
            .repo_url
            .as_deref()
            .unwrap()
            .starts_with("archive+http://")
    );
    assert_eq!(
        entry.url.as_deref(),
        Some(&*format!("{}/files/plugin.zip", server.origin))
    );
    // Listing downloads nothing: the plugin names its folders.
    assert_eq!(server.calls("zip"), 0);

    let preview = hub
        .skill_preview(
            &Caller::Hub,
            SkillPreview {
                source: install_entry(entry),
            },
        )
        .await
        .unwrap();
    assert_eq!(preview.name, "pdf");
    assert_eq!(preview.commit.as_deref(), Some(hash.as_str()));
    let skill = hub
        .skill_install(
            &Caller::Hub,
            SkillInstall {
                source: install_entry(entry),
                use_in: UseIn::EveryChannel,
                enabled: None,
                approval_id: None,
            },
        )
        .await
        .unwrap()
        .skill;
    assert_eq!(skill.commit.as_deref(), Some(hash.as_str()));
    assert!(
        matches!(&skill.source, SkillSource::Directory { url, .. } if url.starts_with("archive+"))
    );
    assert_eq!(skill.file_count, 2, "only the folder of the skill");
}

#[tokio::test]
async fn an_archive_that_does_not_match_its_hash_is_refused() {
    let (hub, _dir) = hub().await;
    let directory = FakeDirectory::start().await;
    let server = PackageServer::start().await;
    server.state.lock().unwrap().zip = zip_of("one");
    let entries = add_marketplace(
        &hub,
        &directory,
        json!([{"name": "docs", "skills": ["./skills/pdf"],
                "source": {"source": "archive", "url": format!("{}/files/plugin.zip", server.origin),
                           "sha256": "c".repeat(64)}}]),
    )
    .await;
    let err = preview_error(&hub, install_entry(&entries[0])).await;
    assert_eq!(err.code(), ErrorCode::SkillInvalid);
    assert!(
        err.to_error_body()
            .message
            .contains("does not match the sha256")
    );
}

#[tokio::test]
async fn a_big_archive_and_a_host_on_a_private_network_are_refused() {
    let (hub, _dir) = hub().await;
    let server = PackageServer::start().await;
    let err = hub
        .fetch_remote(
            &format!("archive+{}/files/huge.zip", server.origin),
            Some("skills/pdf"),
            None,
            false,
        )
        .await
        .unwrap_err();
    assert!(err.to_error_body().message.contains("larger than"), "{err}");
    // With the test switch off, a loopback address is refused whole.
    hub.skill_sources.set_config(|c| c.allow_local = false);
    for blocked in [
        "archive+https://169.254.169.254/latest.zip",
        "archive+https://localhost/x.zip",
        "archive+https://10.0.0.5/x.zip",
    ] {
        let err = hub
            .fetch_remote(blocked, Some("s"), None, false)
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::Invalid, "{blocked}");
        assert!(
            err.to_error_body().message.contains("private network"),
            "{blocked}"
        );
    }
}

#[tokio::test]
async fn the_update_check_of_an_archive_skill_finds_a_changed_file() {
    let (hub, _dir) = hub().await;
    let directory = FakeDirectory::start().await;
    let server = PackageServer::start().await;
    server.state.lock().unwrap().zip = zip_of("one");
    // No sha256 in the marketplace: the hub looks at the bytes.
    let entries = add_marketplace(
        &hub,
        &directory,
        json!([{"name": "docs", "skills": ["./skills/pdf"],
                "source": {"source": "archive", "url": format!("{}/files/plugin.zip", server.origin)}}]),
    )
    .await;
    let skill = hub
        .skill_install(
            &Caller::Hub,
            SkillInstall {
                source: install_entry(&entries[0]),
                use_in: UseIn::EveryChannel,
                enabled: None,
                approval_id: None,
            },
        )
        .await
        .unwrap()
        .skill;
    let report = hub.check_skill_updates().await.unwrap();
    assert_eq!((report.checked, report.available, report.failed), (1, 0, 0));
    server.state.lock().unwrap().zip = zip_of("two");
    let report = hub.check_skill_updates().await.unwrap();
    assert_eq!((report.checked, report.available), (1, 1));
    let after = hub
        .store
        .read(move |conn| crate::store::skills::skill_or_404(conn, skill.id))
        .await
        .unwrap();
    let SkillUpdateState::Available { changes, .. } = after.update else {
        panic!("an update is available");
    };
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "SKILL.md");
}

// npm --------------------------------------------------------------------------

fn npm_plugin(server: &PackageServer, version: &str) -> Value {
    json!([{"name": "pkg", "skills": ["./skills/pdf"],
            "source": {"source": "npm", "package": "@acme/skills", "version": version,
                       "registry": format!("{}/npm", server.origin)}}])
}

#[tokio::test]
async fn an_npm_plugin_installs_from_its_tarball_and_pins_the_integrity() {
    let (hub, _dir) = hub().await;
    let directory = FakeDirectory::start().await;
    let server = PackageServer::start().await;
    let tgz = tgz_of("one");
    let integrity = npm_integrity(&tgz);
    server.state.lock().unwrap().tgz = tgz;
    let entries = add_marketplace(&hub, &directory, npm_plugin(&server, "2.1.0")).await;
    assert_eq!(entries.len(), 1);
    assert!(
        entries[0]
            .repo_url
            .as_deref()
            .unwrap()
            .starts_with("npm:@acme/skills@2.1.0?registry=")
    );
    assert_eq!(server.calls("tgz"), 0, "listing downloads nothing");
    let skill = hub
        .skill_install(
            &Caller::Hub,
            SkillInstall {
                source: install_entry(&entries[0]),
                use_in: UseIn::EveryChannel,
                enabled: None,
                approval_id: None,
            },
        )
        .await
        .unwrap()
        .skill;
    assert_eq!(skill.name, "pdf");
    assert_eq!(skill.commit.as_deref(), Some(integrity.as_str()));
    assert_eq!(skill.file_count, 2);
    assert_eq!(server.calls("npm @acme/skills 2.1.0"), 1);
}

#[tokio::test]
async fn an_npm_tarball_with_the_wrong_integrity_is_refused() {
    let (hub, _dir) = hub().await;
    let directory = FakeDirectory::start().await;
    let server = PackageServer::start().await;
    {
        let mut state = server.state.lock().unwrap();
        state.tgz = tgz_of("one");
        state.integrity = npm_integrity(b"something else");
    }
    let entries = add_marketplace(&hub, &directory, npm_plugin(&server, "2.1.0")).await;
    let err = preview_error(&hub, install_entry(&entries[0])).await;
    assert_eq!(err.code(), ErrorCode::SkillInvalid);
    assert!(err.to_error_body().message.contains("integrity"), "{err}");
}

#[tokio::test]
async fn an_npm_range_a_link_and_a_missing_package_are_refused_with_a_reason() {
    let (hub, _dir) = hub().await;
    let server = PackageServer::start().await;
    let registry = format!("{}/npm", server.origin);
    let range = format!("npm:@acme/skills@^2.0.0?registry={registry}");
    let err = hub
        .fetch_remote(&range, Some("skills/pdf"), None, false)
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::Invalid);
    assert!(err.to_error_body().message.contains("exact version"));
    let link = "npm:https://x.example/p.tgz";
    let err = hub.fetch_remote(link, None, None, false).await.unwrap_err();
    assert!(
        err.to_error_body().message.contains("package name"),
        "{err}"
    );
    let missing = format!("npm:@acme/other@1.0.0?registry={registry}");
    let err = hub
        .fetch_remote(&missing, None, None, false)
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);
}

#[tokio::test]
async fn the_update_check_of_an_npm_skill_reads_only_the_version_document_until_it_changes() {
    let (hub, _dir) = hub().await;
    let directory = FakeDirectory::start().await;
    let server = PackageServer::start().await;
    server.state.lock().unwrap().tgz = tgz_of("one");
    // The tag `latest` follows the registry.
    let entries = add_marketplace(&hub, &directory, npm_plugin(&server, "latest")).await;
    hub.skill_install(
        &Caller::Hub,
        SkillInstall {
            source: install_entry(&entries[0]),
            use_in: UseIn::EveryChannel,
            enabled: None,
            approval_id: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(server.calls("tgz"), 1);
    let report = hub.check_skill_updates().await.unwrap();
    assert_eq!(report.available, 0);
    assert_eq!(
        server.calls("tgz"),
        1,
        "no download when the integrity is the same"
    );
    server.state.lock().unwrap().tgz = tgz_of("two");
    let report = hub.check_skill_updates().await.unwrap();
    assert_eq!(report.available, 1);
}

// command ----------------------------------------------------------------------

#[tokio::test]
async fn a_command_plugin_is_listed_and_an_install_says_why_it_cannot_work() {
    let (hub, _dir) = hub().await;
    let directory = FakeDirectory::start().await;
    let entries = add_marketplace(
        &hub,
        &directory,
        json!([{"name": "built", "description": "Built by a tool.",
                "source": {"source": "command", "command": "my-tool claude-plugin-path"}}]),
    )
    .await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "built");
    assert_eq!(entries[0].repo_url.as_deref(), Some("command:built"));
    assert_eq!(entries[0].url, None);
    let err = preview_error(&hub, install_entry(&entries[0])).await;
    assert_eq!(err.code(), ErrorCode::SkillInvalid);
    let message = err.to_error_body().message;
    assert!(
        message.contains("does not run commands from a marketplace"),
        "{message}"
    );
    // The command text never reaches the entry.
    assert!(!canonical_json(&serde_json::to_value(&entries[0]).unwrap()).contains("my-tool"));
}

// The branch of a directory entry ------------------------------------------------

fn repo_with_two_branches() -> TestRepo {
    let repo = TestRepo::new();
    // The skill is in a folder, and also at the root.
    repo.write("skills/pdf/SKILL.md", skill_md("pdf", ""));
    repo.write("SKILL.md", skill_md("pdf", ""));
    repo.commit("main");
    repo.branch("dev");
    for path in ["skills/pdf/SKILL.md", "SKILL.md"] {
        repo.write(path, skill_md("pdf", "").replace("Steps.", "Steps on dev."));
    }
    repo.commit("dev change");
    repo.checkout("main");
    repo
}

#[tokio::test]
async fn a_directory_entry_follows_its_branch_for_the_install_and_the_update_check() {
    let (hub, _dir) = hub().await;
    let repo = repo_with_two_branches();
    let directory = FakeDirectory::start().await;
    let entries = add_marketplace(
        &hub,
        &directory,
        json!([{"name": "docs", "skills": ["./skills/pdf"],
                "source": {"source": "url", "url": repo.url(), "ref": "dev"}}]),
    )
    .await;
    assert_eq!(
        entries[0].repo_url.as_deref(),
        Some(format!("{}#dev", repo.url()).as_str())
    );
    let skill = hub
        .skill_install(
            &Caller::Hub,
            SkillInstall {
                source: install_entry(&entries[0]),
                use_in: UseIn::EveryChannel,
                enabled: None,
                approval_id: None,
            },
        )
        .await
        .unwrap()
        .skill;
    let SkillSource::Directory { url, .. } = &skill.source else {
        panic!("a directory skill");
    };
    assert!(url.ends_with("#dev"), "the branch is kept: {url}");
    // The files are the ones of the dev branch.
    let text = hub
        .store
        .read(move |conn| crate::store::skills::skill_or_404(conn, skill.id))
        .await
        .unwrap();
    assert_eq!(text.commit.as_deref().map(str::len), Some(40));
    let on_dev = super::git::remote_commit(
        hub.skill_sources.tmp_root(),
        &repo.url(),
        Some("dev"),
        hub.skill_sources.git_options(),
    )
    .await
    .unwrap();
    assert_eq!(text.commit.as_deref(), Some(on_dev.as_str()));
    // A new commit on dev is a new version. One on main is not.
    repo.checkout("dev");
    repo.write(
        "skills/pdf/SKILL.md",
        skill_md("pdf", "").replace("Steps.", "Steps on dev, again."),
    );
    repo.commit("dev again");
    repo.checkout("main");
    repo.write("README.md", "main only");
    repo.commit("main change");
    let report = hub.check_skill_updates().await.unwrap();
    assert_eq!(report.available, 1);
}

#[tokio::test]
async fn a_smithery_page_url_keeps_its_branch() {
    let (hub, _dir) = hub().await;
    let repo = repo_with_two_branches();
    let directory = FakeDirectory::start().await;
    // Smithery gives a GitHub page URL. The branch in it is the branch to
    // follow. (The fake serves a `file://` URL with a `#ref`, the form that
    // the hub keeps.)
    directory.add_skill("acme", "pdf", "Edit PDFs.", &format!("{}#dev", repo.url()));
    hub.skill_sources.set_config({
        let url = directory.url.clone();
        move |c| {
            c.smithery_url = url;
            c.allow_local = true;
        }
    });
    let source = SkillInstallSource::Directory {
        source: None,
        entry_id: "smithery:acme/pdf".to_owned(),
    };
    let preview = hub
        .skill_preview(&Caller::Hub, SkillPreview { source })
        .await
        .unwrap();
    let on_dev = remote_commit(
        hub.skill_sources.tmp_root(),
        &repo.url(),
        Some("dev"),
        hub.skill_sources.git_options(),
    )
    .await
    .unwrap();
    assert_eq!(preview.commit.as_deref(), Some(on_dev.as_str()));
}

// Private repos and ssh --------------------------------------------------------

#[test]
fn ssh_urls_are_read_when_the_user_gave_them_and_refused_otherwise() {
    let user = GitOptions::new(false).trusted();
    let other = GitOptions::new(false);
    for (input, url, git_ref) in [
        (
            "ssh://git@github.com/acme/private.git",
            "ssh://git@github.com/acme/private.git",
            None,
        ),
        (
            "git@github.com:acme/private.git",
            "git@github.com:acme/private.git",
            None,
        ),
        (
            "git@github.com:acme/private.git#v2",
            "git@github.com:acme/private.git",
            Some("v2"),
        ),
        (
            "ssh://git@git.example.com:2222/srv/x.git",
            "ssh://git@git.example.com:2222/srv/x.git",
            None,
        ),
        (
            "deploy@git.example.com:/srv/git/x.git",
            "deploy@git.example.com:/srv/git/x.git",
            None,
        ),
        // Git reads this form itself: a path with no leading slash is in the
        // home folder of the ssh user.
        (
            "git@git.example.com:team/x.git",
            "git@git.example.com:team/x.git",
            None,
        ),
    ] {
        let target = parse_git_url(input, user).unwrap();
        assert_eq!(target.url, url, "{input}");
        assert_eq!(target.git_ref.as_deref(), git_ref, "{input}");
        let refused = parse_git_url(input, other).unwrap_err();
        assert!(
            refused
                .to_error_body()
                .message
                .contains("only for a repo that you add yourself"),
            "{input}"
        );
    }
    for bad in [
        "ssh://-oProxyCommand=evil/x",
        "ssh://git@-host/x",
        "ssh://git@host/-x",
        "ssh://git@host",
        "ssh://git@ho st/x",
        "ssh://git@host:99999999/x",
        "ssh://user:pw@host/x",
        "-oProxyCommand=evil@host:x",
        "ext::sh -c id",
        "evil@host:-x",
        "git@host:../../etc/x",
        "C:/folder/repo",
    ] {
        assert!(parse_git_url(bad, user).is_err(), "{bad}");
    }
}

#[test]
fn a_ref_comes_after_a_hash() {
    let target = parse_git_url(
        "https://git.example.com/a/b.git#feature/x",
        GitOptions::new(false),
    )
    .unwrap();
    assert_eq!(target.url, "https://git.example.com/a/b.git");
    assert_eq!(target.git_ref.as_deref(), Some("feature/x"));
    for bad in [
        "https://git.example.com/a/b.git#",
        "https://git.example.com/a/b.git#--upload-pack=x",
        "https://git.example.com/a/b.git#a..b",
    ] {
        assert!(parse_git_url(bad, GitOptions::new(false)).is_err(), "{bad}");
    }
}

/// A server that answers every request with `401` and `WWW-Authenticate`,
/// and keeps the headers of each request. Git asks for a login at once.
struct LoginServer {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl LoginServer {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/private/repo.git", listener.local_addr().unwrap());
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let seen = requests.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut buffer = vec![0_u8; 8192];
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    seen.lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&buffer[..n]).into_owned());
                    let reply = "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"repo\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = socket.write_all(reply.as_bytes()).await;
                });
            }
        });
        Self { url, requests }
    }

    /// How many requests carried a login, and how many did not.
    fn logins(&self) -> (usize, usize) {
        let requests = self.requests.lock().unwrap();
        let with = requests
            .iter()
            .filter(|r| r.to_ascii_lowercase().contains("authorization: basic"))
            .count();
        (with, requests.len() - with)
    }

    fn has_login_of(&self, user_and_password: &str) -> bool {
        let wanted = base64_of(user_and_password.as_bytes());
        self.requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.contains(&format!("Basic {wanted}")))
    }
}

fn base64_of(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = u32::from(chunk[0]) << 16
            | u32::from(chunk.get(1).copied().unwrap_or(0)) << 8
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(TABLE[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A home folder whose git config has a credential helper that gives
/// `alice` and `s3cret`. The empty `helper` clears the helpers of the system
/// config: Git for Windows sets a credential manager there, which waits for a
/// person.
fn home_with_credential_helper() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".gitconfig"),
        "[credential]\n\thelper =\n\thelper = \"!f() { echo username=alice; echo password=s3cret; }; f\"\n",
    )
    .unwrap();
    home
}

#[tokio::test]
async fn a_repo_that_asks_for_a_login_gets_the_credential_helper_of_the_hub_user() {
    let (hub, _dir) = hub().await;
    let server = LoginServer::start().await;
    let home = home_with_credential_helper();
    set_test_home(Some(home.path().to_path_buf()));
    // The user typed the URL: git may use its credentials.
    let err = hub
        .skill_preview(
            &Caller::Hub,
            SkillPreview {
                source: SkillInstallSource::Git {
                    url: server.url.clone(),
                    path: None,
                    git_ref: None,
                },
            },
        )
        .await
        .unwrap_err();
    set_test_home(None);
    let (with, without) = server.logins();
    assert!(without >= 1, "the first try has no login");
    assert!(with >= 1, "the second try sends the login of the helper");
    assert!(server.has_login_of("alice:s3cret"));
    // The server refuses it too, so the error says what the user can check.
    let message = err.to_error_body().message;
    assert!(
        message.contains("hub's git credentials did not give one"),
        "{message}"
    );
    assert!(message.contains("git ls-remote"), "{message}");
    assert!(
        !message.contains("s3cret"),
        "a secret never reaches a message"
    );
}

#[tokio::test]
async fn the_assistant_cannot_make_the_hub_use_its_credentials_for_a_url_the_user_never_gave() {
    let (hub, _dir) = hub().await;
    let server = LoginServer::start().await;
    let home = home_with_credential_helper();
    set_test_home(Some(home.path().to_path_buf()));
    // `add_skill` from the assistant is not typed by the user.
    let result = hub
        .fetch_install_source(
            &SkillInstallSource::Git {
                url: server.url.clone(),
                path: None,
                git_ref: None,
            },
            false,
        )
        .await;
    set_test_home(None);
    let message = result.unwrap_err().to_error_body().message;
    let (with, _) = server.logins();
    assert_eq!(with, 0, "no login was sent");
    assert!(
        message.contains("only for a repo that you add yourself"),
        "{message}"
    );
}

#[tokio::test]
async fn a_repo_that_the_user_installed_from_may_use_credentials_for_its_update_check() {
    let (hub, _dir) = hub().await;
    let server = LoginServer::start().await;
    // A skill that came from this URL. The user installed it earlier.
    hub.store_skill(super::NewSkill {
        source: SkillSource::Git {
            url: server.url.clone(),
            path: None,
            git_ref: None,
        },
        ..super::test_skill("private-skill", false)
    })
    .await
    .unwrap();
    let home = home_with_credential_helper();
    set_test_home(Some(home.path().to_path_buf()));
    let options = hub.git_options_for(&server.url, false).await.unwrap();
    set_test_home(None);
    assert!(options.credentials);
    let other = hub
        .git_options_for("https://elsewhere.example/x.git", false)
        .await
        .unwrap();
    assert!(!other.credentials);
}

#[tokio::test]
async fn an_ssh_repo_goes_through_ssh_and_never_reads_a_password() {
    // Nothing listens on port 1, so ssh fails to connect. The point is the
    // route: git is allowed to use ssh, runs it in batch mode, and the error
    // names the host.
    let (hub, _dir) = hub().await;
    let err = hub
        .fetch_install_source(
            &SkillInstallSource::Git {
                url: "ssh://git@127.0.0.1:1/acme/private.git".to_owned(),
                path: None,
                git_ref: None,
            },
            true,
        )
        .await
        .unwrap_err();
    let message = err.to_error_body().message;
    assert!(message.contains("127.0.0.1"), "{message}");
    assert!(
        !message.contains("not allowed"),
        "ssh must be an allowed protocol: {message}"
    );
    // The same URL from the assistant is refused before git runs.
    let err = hub
        .fetch_install_source(
            &SkillInstallSource::Git {
                url: "git@github.com:acme/private.git".to_owned(),
                path: None,
                git_ref: None,
            },
            false,
        )
        .await
        .unwrap_err();
    assert!(err.to_error_body().message.contains("add yourself"));
}

#[test]
fn the_messages_name_the_fix() {
    let hint = failure_hint;
    let host = hint(
        "ssh://git@git.example.com/x.git",
        "Host key verification failed.",
        true,
    )
    .unwrap();
    assert!(host.contains("ssh git.example.com"), "{host}");
    let key = hint(
        "ssh://git@github.com/x.git",
        "git@github.com: Permission denied (publickey).",
        true,
    )
    .unwrap();
    assert!(key.contains("ssh-add -l"), "{key}");
    let slow = hint(
        "https://x.example/a.git",
        "git took longer than 2 minutes",
        false,
    )
    .unwrap();
    assert!(slow.contains("slow or down"), "{slow}");
    assert!(hint("https://x.example/a.git", "repository not found", false).is_none());
}

// Time limits --------------------------------------------------------------------

/// A server that accepts a connection and never answers.
async fn silent_server() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    (format!("http://{address}"), task)
}

#[tokio::test]
async fn git_gives_up_on_a_server_that_never_answers() {
    let (base, task) = silent_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let options = GitOptions {
        timeout: Duration::from_millis(1500),
        ..GitOptions::new(true)
    };
    let started = Instant::now();
    let err = Repo::fetch(tmp.path(), &format!("{base}/slow.git"), None, options)
        .await
        .map(|_| ())
        .unwrap_err();
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(1400), "{took:?}");
    assert!(took < Duration::from_secs(20), "{took:?}");
    let message = err.to_error_body().message;
    assert!(message.contains("slow or down"), "{message}");
    assert!(
        message.contains("1500 milliseconds") || message.contains("took longer"),
        "{message}"
    );
    // `git ls-remote` has the same limit.
    let started = Instant::now();
    let err = remote_commit(tmp.path(), &format!("{base}/slow.git"), None, options)
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(err.to_error_body().message.contains("slow or down"));
    // No temp folder stays after the fetch gave up.
    let left: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().flatten().collect();
    assert!(left.is_empty(), "{left:?}");
    task.abort();
}

#[tokio::test]
async fn the_hub_gives_a_git_install_a_clear_error_when_the_server_is_silent() {
    let (hub, _dir) = hub().await;
    let (base, task) = silent_server().await;
    hub.skill_sources
        .set_config(|c| c.git_timeout = Duration::from_millis(1500));
    let started = Instant::now();
    let err = hub
        .skill_preview(
            &Caller::Hub,
            SkillPreview {
                source: SkillInstallSource::Git {
                    url: format!("{base}/slow.git"),
                    path: None,
                    git_ref: None,
                },
            },
        )
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(err.code(), ErrorCode::NotFound);
    assert!(
        err.to_error_body()
            .message
            .contains("did not finish in time")
    );
    task.abort();
}

#[tokio::test]
async fn http_gives_up_on_a_directory_and_a_download_that_never_answer() {
    let (hub, _dir) = hub().await;
    let (base, task) = silent_server().await;
    hub.skill_sources.set_config(|c| {
        c.http_timeout = Duration::from_millis(500);
        c.download_timeout = Duration::from_millis(500);
    });
    // A marketplace file over HTTP.
    let started = Instant::now();
    let err = hub
        .skill_directory_add_source(
            &Caller::Hub,
            SkillDirectoryAddSource {
                url: format!("{base}/marketplace.json"),
            },
        )
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(10));
    let message = err.to_error_body().message;
    assert!(
        message.contains("did not answer within 500 milliseconds"),
        "{message}"
    );
    // A download of an archive.
    let started = Instant::now();
    let err = hub
        .fetch_remote(
            &format!("archive+{base}/files/never.zip"),
            Some("skills/pdf"),
            None,
            false,
        )
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(10));
    let message = err.to_error_body().message;
    assert!(
        message.contains("did not answer within 500 milliseconds"),
        "{message}"
    );
    assert_eq!(err.code(), ErrorCode::NotFound);
    // An npm registry.
    let err = hub
        .fetch_remote(
            &format!("npm:@acme/skills@1.0.0?registry={base}/npm"),
            None,
            None,
            false,
        )
        .await
        .unwrap_err();
    assert!(
        err.to_error_body()
            .message
            .contains("did not answer within"),
        "{err}"
    );
    task.abort();
}
