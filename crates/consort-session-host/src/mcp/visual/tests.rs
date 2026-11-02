use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use consort_proto::ids::SessionId;
use consort_proto::model::{VISUAL_WIDTHS, VisualHeight};
use consort_proto::tools::{PREVIEW_VISUAL_INSTALLING, ToolDefinition};
use serde_json::json;
use tokio::sync::Mutex;

use super::*;
use crate::bridge::McpHttpClient;
use crate::mcp::{McpSession, ToolBackend, start};
use crate::visual::testing::{EngineCall, FAKE_PNG, FakeEngine, MemoryBackend};

/// The PNG signature, and enough bytes to look like a file.
const PNG: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
];

fn listed() -> Vec<ToolDefinition> {
    vec![
        HubTool::PostUpdate.definition(),
        HubTool::PreviewVisual.definition(),
        HubTool::ShowVisual.definition(),
    ]
}

/// A coordinator server with the visual tools.
fn server(
    workspace: &Path,
    backend: Arc<MemoryBackend>,
    engine: Arc<FakeEngine>,
) -> HubToolsServer {
    HubToolsServer {
        session_id: SessionId::new(),
        workspace: workspace.to_path_buf(),
        backend,
        exec: None,
        worker_root: None,
        visual: engine,
        tools: Arc::new(Mutex::new(None)),
        calls: Arc::new(AtomicU64::new(0)),
    }
}

fn texts(result: &ToolCallResult) -> Vec<String> {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            ToolContent::Text { text } => Some(text.clone()),
            ToolContent::Image { .. } => None,
        })
        .collect()
}

fn error_of(result: &ToolCallResult) -> ErrorBody {
    assert!(result.is_error, "{result:?}");
    let value = result.structured_content.clone().unwrap();
    serde_json::from_value(value["error"].clone()).unwrap()
}

/// The page that a `show_visual` call uploaded, by the `page` it forwarded.
fn uploaded_page(backend: &MemoryBackend) -> (ShowVisualArgs, String) {
    let calls = backend.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].name, "show_visual");
    let args: ShowVisualArgs = serde_json::from_value(calls[0].arguments.clone()).unwrap();
    let page = args.page.clone().expect("the call carries the page");
    let bytes = backend.blob(&page.sha256).expect("the page was uploaded");
    (args, String::from_utf8(bytes).unwrap())
}

#[tokio::test]
async fn preview_visual_answers_an_image_and_the_result_and_forwards_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MemoryBackend::with_tools(listed()));
    let engine = Arc::new(FakeEngine::new(312));
    let server = server(dir.path(), backend.clone(), engine.clone());
    let result = server
        .call(
            "preview_visual".to_owned(),
            json!({"html": "<div id=\"chart\"></div><script>Ink.bars('#chart', {})</script>"}),
            None,
        )
        .await;
    assert!(!result.is_error, "{result:?}");
    assert_eq!(result.content.len(), 2);
    let ToolContent::Image { data, mime_type } = &result.content[0] else {
        panic!("the first block is the image: {result:?}");
    };
    assert_eq!(mime_type, "image/png");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap(),
        FAKE_PNG
    );
    let ToolContent::Text { text } = &result.content[1] else {
        panic!("the second block is the result: {result:?}");
    };
    let parsed: PreviewVisualResult = serde_json::from_str(text).unwrap();
    assert_eq!(parsed.width, VISUAL_PREVIEW_WIDTH_DEFAULT);
    assert_eq!(parsed.height, 312);
    assert_eq!(parsed.console.len(), 1);
    assert_eq!(parsed.console[0].text, "drawn");
    assert_eq!(parsed.omitted, 0);
    assert!(result.structured_content.is_none());

    // Nothing went to the hub.
    assert!(backend.calls().is_empty());
    assert!(backend.uploads().is_empty());
    let calls = engine.calls();
    let [
        EngineCall::Render {
            document,
            width,
            scale,
            mode,
            max_height,
        },
    ] = calls.as_slice()
    else {
        panic!("one render: {calls:?}");
    };
    assert_eq!(*width, 720);
    assert_eq!(*scale, 1);
    assert_eq!(*mode, VisualMode::Dark);
    assert_eq!(*max_height, PREVIEW_IMAGE_MAX_HEIGHT);
    // The document has the theme of dark mode and the ink kit, then the
    // page.
    assert!(document.contains("color-scheme:dark"));
    let ink = document.find("consort-visual-ink").unwrap();
    let page = document.find("<div id=\"chart\">").unwrap();
    assert!(ink < page);
}

#[tokio::test]
async fn preview_visual_reaches_the_harness_as_an_image_block() {
    let dir = tempfile::tempdir().unwrap();
    let backend: Arc<dyn ToolBackend> = Arc::new(MemoryBackend::with_tools(listed()));
    let endpoint = start(McpSession {
        session_id: SessionId::new(),
        workspace: dir.path().to_path_buf(),
        backend,
        exec: None,
        worker_root: None,
        visual: Arc::new(FakeEngine::new(200)),
    })
    .await
    .unwrap();
    let client = McpHttpClient::new(&endpoint.url, &endpoint.token).unwrap();
    client
        .post(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}
        }}))
        .await
        .unwrap();
    client
        .post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await
        .unwrap();
    let answer = client
        .post(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "preview_visual", "arguments": {"html": "<p>Hi</p>", "width": 480, "appearance": "light"}
        }}))
        .await
        .unwrap();
    let result = &answer[0]["result"];
    assert_eq!(result["isError"], false, "{answer:?}");
    assert_eq!(result["content"][0]["type"], "image");
    assert_eq!(result["content"][0]["mimeType"], "image/png");
    assert!(result["content"][0]["data"].as_str().unwrap().len() > 10);
    assert_eq!(result["content"][1]["type"], "text");
    let parsed: PreviewVisualResult =
        serde_json::from_str(result["content"][1]["text"].as_str().unwrap()).unwrap();
    assert_eq!((parsed.width, parsed.height), (480, 200));
    endpoint.stop().await;
}

#[tokio::test]
async fn preview_visual_checks_the_width_and_passes_the_appearance() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MemoryBackend::with_tools(listed()));
    let engine = Arc::new(FakeEngine::new(100));
    let server = server(dir.path(), backend, engine.clone());
    for width in [VISUAL_PREVIEW_WIDTH_MIN - 1, VISUAL_PREVIEW_WIDTH_MAX + 1] {
        let result = server
            .call(
                "preview_visual".to_owned(),
                json!({"html": "<p>x</p>", "width": width}),
                None,
            )
            .await;
        let err = error_of(&result);
        assert_eq!(err.code, ErrorCode::Invalid);
        assert!(err.message.contains("320 to 1144"), "{}", err.message);
    }
    assert!(engine.calls().is_empty());
    let result = server
        .call(
            "preview_visual".to_owned(),
            json!({"html": "<p>x</p>", "width": 1144, "appearance": "light"}),
            None,
        )
        .await;
    assert!(!result.is_error, "{result:?}");
    let calls = engine.calls();
    let [
        EngineCall::Render {
            document,
            width,
            mode,
            ..
        },
    ] = calls.as_slice()
    else {
        panic!("one render: {calls:?}");
    };
    assert_eq!((*width, *mode), (1144, VisualMode::Light));
    assert!(document.contains("color-scheme:light"));
}

#[tokio::test]
async fn preview_visual_says_that_the_browser_installs() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MemoryBackend::with_tools(listed()));
    let engine = Arc::new(FakeEngine::failing(RenderError::Installing));
    let result = server(dir.path(), backend, engine)
        .call(
            "preview_visual".to_owned(),
            json!({"html": "<p>x</p>"}),
            None,
        )
        .await;
    let err = error_of(&result);
    assert_eq!(err.code, ErrorCode::Unavailable);
    assert_eq!(err.message, PREVIEW_VISUAL_INSTALLING);

    // A page that does not end: the model reads what to change.
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MemoryBackend::with_tools(listed()));
    let engine = Arc::new(FakeEngine::failing(RenderError::Timeout(
        Duration::from_secs(20),
    )));
    let result = server(dir.path(), backend, engine)
        .call(
            "preview_visual".to_owned(),
            json!({"html": "<p>x</p>"}),
            None,
        )
        .await;
    assert!(error_of(&result).message.contains("did not finish in 20 s"));
}

#[tokio::test]
async fn preview_visual_tells_the_model_when_the_image_is_cut() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MemoryBackend::with_tools(listed()));
    let engine = Arc::new(FakeEngine::new(3_200));
    let result = server(dir.path(), backend, engine)
        .call(
            "preview_visual".to_owned(),
            json!({"html": "<p>x</p>"}),
            None,
        )
        .await;
    assert!(!result.is_error);
    let texts = texts(&result);
    assert_eq!(texts.len(), 2);
    assert_eq!(
        texts[1],
        "The page is 3200 px tall. The image shows only the top 2000 px."
    );
}

#[tokio::test]
async fn the_visual_tools_need_the_tool_list_of_the_hub() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MemoryBackend::with_tools(vec![
        HubTool::PostUpdate.definition(),
    ]));
    let engine = Arc::new(FakeEngine::new(100));
    let server = server(dir.path(), backend.clone(), engine.clone());
    for name in ["preview_visual", "show_visual"] {
        let result = server
            .call(
                name.to_owned(),
                json!({"html": "<p>x</p>", "title": "X"}),
                None,
            )
            .await;
        let err = error_of(&result);
        assert_eq!(err.code, ErrorCode::Unsupported, "{name}");
        assert!(err.message.contains(name), "{}", err.message);
    }
    assert!(engine.calls().is_empty());
    assert!(backend.calls().is_empty());
    assert!(backend.uploads().is_empty());
}

#[tokio::test]
async fn show_visual_refuses_a_page_with_viewport_units_before_any_upload() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MemoryBackend::with_tools(listed()));
    let engine = Arc::new(FakeEngine::new(100));
    let server = server(dir.path(), backend.clone(), engine.clone());
    for name in ["show_visual", "preview_visual"] {
        let result = server
            .call(
                name.to_owned(),
                json!({"html": "<div style=\"height:100vh\">Tall</div>", "title": "Tall"}),
                None,
            )
            .await;
        let err = error_of(&result);
        assert_eq!(err.code, ErrorCode::Invalid, "{name}");
        assert!(err.message.contains("viewport height"), "{}", err.message);
    }
    let empty = server
        .call(
            "show_visual".to_owned(),
            json!({"html": "<p>x</p>", "title": "  "}),
            None,
        )
        .await;
    assert!(error_of(&empty).message.contains("title"));
    assert!(backend.uploads().is_empty());
    assert!(backend.calls().is_empty());
    assert!(engine.calls().is_empty());
}

#[tokio::test]
async fn show_visual_uploads_the_page_and_forwards_the_page_and_the_heights() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MemoryBackend::with_tools(listed()));
    let engine = Arc::new(FakeEngine::new(288));
    let server = server(dir.path(), backend.clone(), engine.clone());
    let html = "<h2>Tokens by week</h2><div id=\"c\"></div>";
    let result = server
        .call(
            "show_visual".to_owned(),
            json!({"html": html, "title": "Tokens: by week?", "height": 300}),
            None,
        )
        .await;
    assert!(!result.is_error, "{result:?}");
    // The answer of the hub reaches the model.
    assert_eq!(result.structured_content.unwrap()["tool"], "show_visual");

    let (args, page) = uploaded_page(&backend);
    assert_eq!(args.html, "");
    assert_eq!(args.title, "Tokens: by week?");
    assert_eq!(args.height, Some(300));
    // The page is stored as the model wrote it, with no bootstrap.
    assert_eq!(page, html);
    let blob = args.page.unwrap();
    assert_eq!(blob.name, "Tokens by week.html");
    assert_eq!(blob.mime, "text/html");
    assert_eq!(blob.size, html.len() as u64);
    assert_eq!(blob.sha256, blob_hash(html.as_bytes()));
    let heights = args.heights.unwrap();
    assert_eq!(
        heights,
        VISUAL_WIDTHS
            .iter()
            .map(|&width| VisualHeight { width, height: 288 })
            .collect::<Vec<_>>()
    );
    // The height table is one load in dark mode, with the bootstrap.
    let calls = engine.calls();
    let [
        EngineCall::Measure {
            document,
            widths,
            mode,
        },
    ] = calls.as_slice()
    else {
        panic!("one measure: {calls:?}");
    };
    assert_eq!(widths, &VISUAL_WIDTHS.to_vec());
    assert_eq!(*mode, VisualMode::Dark);
    assert!(document.contains("consort-visual-theme"));
    assert!(document.contains(html));
}

#[tokio::test]
async fn show_visual_posts_with_no_heights_while_the_browser_installs() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MemoryBackend::with_tools(listed()));
    let engine = Arc::new(FakeEngine::failing(RenderError::Installing));
    let result = server(dir.path(), backend.clone(), engine)
        .call(
            "show_visual".to_owned(),
            json!({"html": "<p>x</p>", "title": "X"}),
            None,
        )
        .await;
    assert!(!result.is_error, "{result:?}");
    let (args, page) = uploaded_page(&backend);
    assert_eq!(page, "<p>x</p>");
    assert_eq!(args.heights, None);
    // The hub reads no table from the call.
    let calls = backend.calls();
    assert!(calls[0].arguments.get("heights").is_none());
    assert!(calls[0].arguments.get("page").is_some());

    // Any other failure of the browser posts the visual too.
    let backend = Arc::new(MemoryBackend::with_tools(listed()));
    let engine = Arc::new(FakeEngine::failing(RenderError::NoUsableSandbox));
    let result = server(dir.path(), backend.clone(), engine)
        .call(
            "show_visual".to_owned(),
            json!({"html": "<p>x</p>", "title": "X"}),
            None,
        )
        .await;
    assert!(!result.is_error, "{result:?}");
    assert_eq!(uploaded_page(&backend).0.heights, None);
}

#[tokio::test]
async fn show_visual_inlines_an_image_of_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("img")).unwrap();
    let chart = dir.path().join("img").join("my chart.png");
    std::fs::write(&chart, PNG).unwrap();
    let backend = Arc::new(MemoryBackend::with_tools(listed()));
    let engine = Arc::new(FakeEngine::new(100));
    let server = server(dir.path(), backend.clone(), engine.clone());
    let path = chart.to_string_lossy().into_owned();
    let url = format!("file://{}", path.replace(' ', "%20"));
    let html = format!(
        "<img src=\"{path}\"><div style=\"background:url('{url}')\"></div><img src=\"https://example.com/a.png\">"
    );
    let result = server
        .call(
            "show_visual".to_owned(),
            json!({"html": html, "title": "Chart"}),
            None,
        )
        .await;
    assert!(!result.is_error, "{result:?}");
    let (_, page) = uploaded_page(&backend);
    let data = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(PNG)
    );
    assert_eq!(page.matches(&data).count(), 2, "{page}");
    assert!(!page.contains(&path), "{page}");
    assert!(page.contains("https://example.com/a.png"));
    // The renderer measured the page with the image in it.
    let EngineCall::Measure { document, .. } = &engine.calls()[0] else {
        panic!("a measure");
    };
    assert!(document.contains(&data));

    // The preview inlines it the same way, and uploads nothing.
    let result = server
        .call(
            "preview_visual".to_owned(),
            json!({"html": format!("<img src=\"{path}\">")}),
            None,
        )
        .await;
    assert!(!result.is_error, "{result:?}");
    let EngineCall::Render { document, .. } = &engine.calls()[1] else {
        panic!("a render");
    };
    assert!(document.contains(&data));
    assert_eq!(backend.uploads().len(), 1);
}

#[tokio::test]
async fn show_visual_refuses_an_image_outside_the_workspace_a_missing_one_and_a_renamed_file() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("ws");
    std::fs::create_dir(&workspace).unwrap();
    let outside_file = dir.path().join("secret.png");
    std::fs::write(&outside_file, PNG).unwrap();
    std::fs::write(workspace.join("notes.png"), "API_KEY=abc123\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_file, workspace.join("link.png")).unwrap();
    let backend = Arc::new(MemoryBackend::with_tools(listed()));
    let engine = Arc::new(FakeEngine::new(100));
    let server = server(&workspace, backend.clone(), engine.clone());
    let show = |path: String| {
        let server = server.clone();
        async move {
            server
                .call(
                    "show_visual".to_owned(),
                    json!({"html": format!("<img src=\"{path}\">"), "title": "X"}),
                    None,
                )
                .await
        }
    };

    let outside_path = outside_file.to_string_lossy().into_owned();
    let err = error_of(&show(outside_path.clone()).await);
    assert_eq!(
        err.message,
        format!(
            "The image {outside_path} is outside your workspace. Copy it into the workspace, then try again."
        )
    );
    // `..` that leads out is outside too.
    let escape = format!("{}/../secret.png", workspace.display());
    assert!(error_of(&show(escape).await).message.contains("outside"));
    #[cfg(unix)]
    {
        let link = workspace.join("link.png").to_string_lossy().into_owned();
        assert!(error_of(&show(link).await).message.contains("outside"));
    }

    let missing = workspace.join("gone.png").to_string_lossy().into_owned();
    let err = error_of(&show(missing.clone()).await);
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(err.message.contains(&missing), "{}", err.message);

    let renamed = workspace.join("notes.png").to_string_lossy().into_owned();
    let err = error_of(&show(renamed.clone()).await);
    assert_eq!(err.code, ErrorCode::Invalid);
    assert!(err.message.contains(&renamed), "{}", err.message);
    assert!(err.message.contains("not a PNG"), "{}", err.message);

    // Nothing reached the hub or the renderer.
    assert!(backend.uploads().is_empty());
    assert!(backend.calls().is_empty());
    assert!(engine.calls().is_empty());
}

#[test]
fn a_worker_root_gives_paths_relative_to_it() {
    let unix = WorkerRoot::new("/home/ana/work/task", false, false);
    assert_eq!(
        unix.relative("/home/ana/work/task/img/chart.png")
            .as_deref(),
        Some("img/chart.png")
    );
    assert_eq!(
        unix.relative("/home/ana/work/task/./a.png").as_deref(),
        Some("a.png")
    );
    assert_eq!(unix.relative("/home/ana/work/other/a.png"), None);
    assert_eq!(unix.relative("/home/ana/work/task"), None);
    assert_eq!(unix.relative("/home/ana/work/task/../x.png"), None);
    assert_eq!(unix.relative("home/ana/work/task/a.png"), None);
    // Case counts on this worker.
    assert_eq!(unix.relative("/home/ana/Work/task/a.png"), None);

    let windows = WorkerRoot::new("C:\\work\\task", true, true);
    assert_eq!(
        windows
            .relative("C:\\work\\task\\img\\chart.png")
            .as_deref(),
        Some("img/chart.png")
    );
    assert_eq!(
        windows.relative("c:/Work/TASK/img/chart.png").as_deref(),
        Some("img/chart.png")
    );
    assert_eq!(windows.relative("D:\\work\\task\\a.png"), None);
    assert_eq!(windows.relative("/work/task/a.png"), None);
    assert_eq!(windows.relative("C:\\work\\task\\..\\x.png"), None);
}

// Tests against a real `chrome-headless-shell`. They are ignored by
// default. Run them with the path of the browser:
//
// CONSORT_TEST_CHROME=/path/to/chrome-headless-shell \
//   cargo nextest run -p consort-session-host visual_real --run-ignored only
//
// With CONSORT_TEST_VISUAL_OUT=/some/folder, the gallery test writes the
// image of each template there, for a person to look at.

fn real_server(workspace: &Path) -> (HubToolsServer, crate::visual::Renderer) {
    let chrome = std::env::var_os("CONSORT_TEST_CHROME")
        .expect("set CONSORT_TEST_CHROME to the path of chrome-headless-shell");
    let renderer = crate::visual::Renderer::new(crate::visual::RendererConfig::with_path(chrome));
    let mut server = server(
        workspace,
        Arc::new(MemoryBackend::with_tools(listed())),
        Arc::new(FakeEngine::new(1)),
    );
    server.visual = Arc::new(renderer.clone());
    (server, renderer)
}

/// The PNG of a preview result, and its width and height from the IHDR
/// chunk.
fn preview_png(result: &ToolCallResult) -> (Vec<u8>, u32, u32) {
    let ToolContent::Image { data, .. } = &result.content[0] else {
        panic!("no image: {result:?}");
    };
    let png = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
    let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
    (png, width, height)
}

fn preview_result(result: &ToolCallResult) -> PreviewVisualResult {
    serde_json::from_str(&texts(result)[0]).unwrap()
}

#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_preview_draws_ink_bars() {
    let dir = tempfile::tempdir().unwrap();
    let (server, renderer) = real_server(dir.path());
    let html = "<h3 style=\"margin:0 0 8px\">Disk use</h3><div id=\"c\"></div>\
        <script>Ink.bars('#c', {unit: 'GB', data: [['hub', 42], ['homelab', 118], ['bigboi', 76], ['truenas', null, 'Offline']]});</script>";
    let result = server
        .call(
            "preview_visual".to_owned(),
            json!({"html": html, "width": 640}),
            None,
        )
        .await;
    assert!(!result.is_error, "{result:?}");
    let parsed = preview_result(&result);
    assert_eq!(parsed.width, 640);
    assert!(parsed.height > 100, "{parsed:?}");
    assert!(
        parsed.console.iter().all(|m| m.level != "error"),
        "{:?}",
        parsed.console
    );
    let (_, width, height) = preview_png(&result);
    assert_eq!((width, height), (640, parsed.height));
    // The kit drew an SVG into the target.
    let probe = server
        .call(
            "preview_visual".to_owned(),
            json!({"html": format!("{html}<script>console.log('svg ' + document.querySelectorAll('#c svg path').length)</script>")}),
            None,
        )
        .await;
    let console = preview_result(&probe).console;
    let paths: usize = console
        .iter()
        .find_map(|m| m.text.strip_prefix("svg ").map(|n| n.parse().unwrap()))
        .expect("the probe ran");
    assert!(paths >= 3, "{console:?}");
    renderer.stop().await;
}

#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_preview_reports_a_blocked_load() {
    let dir = tempfile::tempdir().unwrap();
    let (server, renderer) = real_server(dir.path());
    let result = server
        .call(
            "preview_visual".to_owned(),
            json!({"html": "<p>x</p><img src=\"chart.png\"><img src=\"http://10.0.0.1/a.png\">"}),
            None,
        )
        .await;
    assert!(!result.is_error, "{result:?}");
    let console = preview_result(&result).console;
    assert!(
        console
            .iter()
            .any(|m| m.level == "error" && m.text.contains("(chart.png)")),
        "{console:?}"
    );
    assert!(
        console
            .iter()
            .any(|m| m.level == "error" && m.text.contains("10.0.0.1")),
        "{console:?}"
    );
    renderer.stop().await;
}

#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_previews_the_gallery_templates() {
    let gallery = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../consort-visual/kit/gallery.html"),
    )
    .unwrap();
    // The document has the kit and the fonts already. The gallery loads
    // them by relative paths, which a visual cannot use.
    let base: String = gallery
        .lines()
        .filter(|line| !line.contains("@font-face") && !line.contains("<script src=\"ink.js\">"))
        .collect::<Vec<_>>()
        .join("\n");
    let ids: Vec<String> = gallery
        .match_indices("add('")
        .map(|(at, _)| {
            let rest = &gallery[at + 5..];
            rest[..rest.find('\'').unwrap()].to_owned()
        })
        .collect();
    assert!(ids.len() > 20, "{ids:?}");
    let out = std::env::var_os("CONSORT_TEST_VISUAL_OUT").map(std::path::PathBuf::from);
    let dir = tempfile::tempdir().unwrap();
    let (server, renderer) = real_server(dir.path());
    let mut failures = Vec::new();
    for mode in ["dark", "light"] {
        for id in &ids {
            let page = base.replace(
                "location.hash.slice(1).split(',')",
                &format!("'{mode},only:{id}'.split(',')"),
            );
            let result = server
                .call(
                    "preview_visual".to_owned(),
                    json!({"html": page, "width": 720, "appearance": mode}),
                    None,
                )
                .await;
            assert!(!result.is_error, "{id} {mode}: {result:?}");
            let parsed = preview_result(&result);
            let (png, _, _) = preview_png(&result);
            let errors: Vec<_> = parsed
                .console
                .iter()
                .filter(|m| m.level == "error")
                .collect();
            eprintln!(
                "{mode} {id}: {} px, {} bytes, console {:?}",
                parsed.height,
                png.len(),
                parsed.console
            );
            if id != "bad" && (!errors.is_empty() || parsed.height < 100) {
                failures.push(format!("{mode} {id}: {} px, {errors:?}", parsed.height));
            }
            if let Some(out) = &out {
                std::fs::create_dir_all(out).unwrap();
                std::fs::write(out.join(format!("{mode}-{id}.png")), &png).unwrap();
            }
        }
    }
    renderer.stop().await;
    assert!(failures.is_empty(), "{failures:#?}");
}
