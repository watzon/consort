//! Test fixtures for skills: a fake skill directory server (the Smithery
//! API shapes and a `marketplace.json`) and git repos made on the disk. No
//! test talks to the real Smithery or GitHub.
//!
//! The integration tests include this file with `#[path]`, so it uses only
//! axum, serde_json, tempfile, tokio and the `git` CLI, and nothing from
//! the hub.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::{Value, json};

#[derive(Default)]
pub struct DirectoryState {
    /// Smithery skills, in the shape of `GET /skills`.
    pub skills: Vec<Value>,
    /// The document at `/marketplace.json`.
    pub marketplace: Value,
    /// When true, every route answers 503.
    pub down: bool,
    /// The calls, for example `GET /skills?q=pdf`.
    pub calls: Vec<String>,
}

type Shared = Arc<Mutex<DirectoryState>>;

/// A fake skill directory on a local port.
#[derive(Clone)]
pub struct FakeDirectory {
    /// The base URL, for example `http://127.0.0.1:1234`.
    pub url: String,
    pub state: Shared,
}

fn down() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, "down").into_response()
}

async fn search(State(state): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let mut state = state.lock().unwrap();
    let query = q.get("q").cloned().unwrap_or_default();
    state.calls.push(format!("GET /skills?q={query}"));
    if state.down {
        return down();
    }
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let skills: Vec<Value> = state
        .skills
        .iter()
        .filter(|s| {
            let hay = format!(
                "{} {}",
                s["displayName"].as_str().unwrap_or(""),
                s["description"].as_str().unwrap_or("")
            )
            .to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .cloned()
        .collect();
    let count = skills.len();
    axum::Json(json!({
        "skills": skills,
        "pagination": {"currentPage": 1, "pageSize": 100, "totalPages": 1, "totalCount": count}
    }))
    .into_response()
}

async fn one(
    State(state): State<Shared>,
    UrlPath((ns, slug)): UrlPath<(String, String)>,
) -> Response {
    let mut state = state.lock().unwrap();
    state.calls.push(format!("GET /skills/{ns}/{slug}"));
    if state.down {
        return down();
    }
    match state
        .skills
        .iter()
        .find(|s| s["namespace"] == ns.as_str() && s["slug"] == slug.as_str())
    {
        Some(skill) => axum::Json(skill.clone()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            axum::Json(json!({"error": "Skill not found"})),
        )
            .into_response(),
    }
}

async fn marketplace(State(state): State<Shared>) -> Response {
    let mut state = state.lock().unwrap();
    state.calls.push("GET /marketplace.json".to_owned());
    if state.down {
        return down();
    }
    axum::Json(state.marketplace.clone()).into_response()
}

impl FakeDirectory {
    pub async fn start() -> Self {
        let state: Shared = Arc::default();
        let app = Router::new()
            .route("/skills", get(search))
            .route("/skills/{ns}/{slug}", get(one))
            .route("/marketplace.json", get(marketplace))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            url: format!("http://{addr}"),
            state,
        }
    }

    /// Adds a Smithery skill whose files are in `git_url`.
    pub fn add_skill(&self, namespace: &str, slug: &str, description: &str, git_url: &str) {
        self.state.lock().unwrap().skills.push(json!({
            "id": format!("{namespace}-{slug}"),
            "namespace": namespace,
            "slug": slug,
            "displayName": slug,
            "description": description,
            "prompt": null,
            "qualityScore": 0.5,
            "totalActivations": 7,
            "gitUrl": git_url,
            "verified": false,
            "listed": true,
            "createdAt": "2026-10-01T00:00:00Z"
        }));
    }

    pub fn set_marketplace(&self, doc: Value) {
        self.state.lock().unwrap().marketplace = doc;
    }

    pub fn set_down(&self, down: bool) {
        self.state.lock().unwrap().down = down;
    }

    /// How many calls started with `prefix`.
    pub fn count(&self, prefix: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| c.starts_with(prefix))
            .count()
    }
}

/// A `SKILL.md` with a name, a description and more front matter lines.
pub fn skill_md(name: &str, extra: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: Does {name} things.\n{extra}---\n\n# {name}\n\nSteps.\n"
    )
}

/// A git repo in a temp folder, made with the `git` CLI. Its URL is a
/// `file://` URL. It lets a client fetch any object and use filters, like
/// GitHub, so the hub's partial fetch runs as it does there.
pub struct TestRepo {
    pub dir: tempfile::TempDir,
    /// Files that `commit` marks as executable in the git index. A file
    /// system without a mode bit (Windows) needs this too.
    executables: std::sync::Mutex<Vec<String>>,
}

fn run_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "2026-10-29T12:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-10-29T12:00:00Z")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

impl TestRepo {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        run_git(path, &["init", "-q", "-b", "main"]);
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("uploadpack.allowFilter", "true"),
            ("uploadpack.allowAnySHA1InWant", "true"),
            ("commit.gpgsign", "false"),
        ] {
            run_git(path, &["config", key, value]);
        }
        Self {
            dir,
            executables: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn url(&self) -> String {
        // A Windows path has backslashes and a drive letter: `C:\x` becomes
        // `file:///C:/x`.
        let path = self.dir.path().display().to_string().replace('\\', "/");
        if path.starts_with('/') {
            format!("file://{path}")
        } else {
            format!("file:///{path}")
        }
    }

    pub fn write(&self, path: &str, bytes: impl AsRef<[u8]>) {
        let full = self.dir.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, bytes).unwrap();
    }

    pub fn write_executable(&self, path: &str, bytes: impl AsRef<[u8]>) {
        self.write(path, bytes);
        self.executables.lock().unwrap().push(path.to_owned());
    }

    #[cfg(unix)]
    pub fn link(&self, path: &str, target: &str) {
        let full = self.dir.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, full).unwrap();
    }

    pub fn remove(&self, path: &str) {
        std::fs::remove_file(self.dir.path().join(path)).unwrap();
    }

    /// Commits every change and returns the commit SHA.
    pub fn commit(&self, message: &str) -> String {
        run_git(self.dir.path(), &["add", "-A"]);
        for path in self.executables.lock().unwrap().iter() {
            run_git(self.dir.path(), &["update-index", "--chmod=+x", "--", path]);
        }
        run_git(
            self.dir.path(),
            &["commit", "-q", "--allow-empty", "-m", message],
        );
        run_git(self.dir.path(), &["rev-parse", "HEAD"])
    }
}
