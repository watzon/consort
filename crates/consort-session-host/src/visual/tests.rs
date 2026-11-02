//! Tests of the renderer against a real `chrome-headless-shell`. They are
//! ignored by default. Run them with the path of the browser:
//!
//! ```text
//! CONSORT_TEST_CHROME=/path/to/chrome-headless-shell \
//!   CARGO_TARGET_DIR=target/visual-renderer \
//!   cargo nextest run -p consort-session-host visual_real --run-ignored only --no-capture
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::*;

const CHROME_VAR: &str = "CONSORT_TEST_CHROME";

fn chrome() -> PathBuf {
    PathBuf::from(
        std::env::var_os(CHROME_VAR)
            .unwrap_or_else(|| panic!("set {CHROME_VAR} to the path of chrome-headless-shell")),
    )
}

fn renderer() -> Renderer {
    Renderer::new(RendererConfig::with_path(chrome()))
}

/// The width and height of a PNG, from its IHDR chunk.
fn png_size(png: &[u8]) -> (u32, u32) {
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
    assert_eq!(&png[12..16], b"IHDR");
    let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
    (width, height)
}

fn page(body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>html,body{{margin:0}}</style></head><body>{body}</body></html>"
    )
}

#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_renders_a_tall_div() {
    let renderer = renderer();
    let document = page("<div style=\"height:300px;background:#c33\"></div>");
    let started = Instant::now();
    let first = renderer
        .render(document.clone(), 400, 2, VisualMode::Dark)
        .await
        .unwrap();
    let cold = started.elapsed();
    assert_eq!(first.height, 300);
    assert_eq!(png_size(&first.png), (800, 600));

    let started = Instant::now();
    let second = renderer
        .render(document, 400, 1, VisualMode::Dark)
        .await
        .unwrap();
    let warm = started.elapsed();
    assert_eq!(second.height, 300);
    assert_eq!(png_size(&second.png), (400, 300));
    eprintln!("first render with the browser start: {cold:?}; second render: {warm:?}");
    renderer.stop().await;
}

#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_measures_a_page_taller_than_the_viewport() {
    let renderer = renderer();
    let document = page("<div style=\"height:2500px;background:#3c3\"></div>");
    let rendered = renderer
        .render(document, 320, 1, VisualMode::Dark)
        .await
        .unwrap();
    assert_eq!(rendered.height, 2500);
    assert_eq!(png_size(&rendered.png), (320, 2500));
    renderer.stop().await;
}

#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_clamps_the_tallest_page() {
    let renderer = renderer();
    let document =
        page("<div style=\"height:25000px;background:linear-gradient(#c33,#33c)\"></div>");
    let started = Instant::now();
    let rendered = renderer
        .render(document, 1144, 3, VisualMode::Dark)
        .await
        .unwrap();
    eprintln!(
        "1144 x 20000 at scale 3: {:?}, {} bytes of PNG",
        started.elapsed(),
        rendered.png.len()
    );
    assert_eq!(rendered.height, consort_proto::model::VISUAL_FIT_MAX);
    assert_eq!(png_size(&rendered.png), (3432, 60_000));
    renderer.stop().await;
}

#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_loads_a_page_at_the_size_limit() {
    let renderer = renderer();
    // A comment fills the page to about 25 MiB, the limit of a prepared
    // page with its images.
    let filler = "x".repeat(25 * 1024 * 1024 - 200);
    let document = page(&format!(
        "<!--{filler}--><div style=\"height:200px\"></div>"
    ));
    let started = Instant::now();
    let rendered = renderer
        .render(document, 720, 1, VisualMode::Dark)
        .await
        .unwrap();
    eprintln!("a page of 25 MiB: {:?}", started.elapsed());
    assert_eq!(rendered.height, 200);
    renderer.stop().await;
}

#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_passes_the_color_scheme() {
    let renderer = renderer();
    let document = page(
        "<script>console.log(matchMedia('(prefers-color-scheme: light)').matches ? 'scheme light' : 'scheme dark')</script>",
    );
    let light = renderer
        .render(document.clone(), 320, 1, VisualMode::Light)
        .await
        .unwrap();
    assert!(
        light.console.iter().any(|m| m.text == "scheme light"),
        "{:?}",
        light.console
    );
    let dark = renderer
        .render(document, 320, 1, VisualMode::Dark)
        .await
        .unwrap();
    assert!(
        dark.console.iter().any(|m| m.text == "scheme dark"),
        "{:?}",
        dark.console
    );
    renderer.stop().await;
}

#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_returns_console_messages_and_errors() {
    let renderer = renderer();
    let document = page(
        "<p>hi</p><script>console.log('hello', 42); console.warn('careful'); console.error('bad');</script>\
         <script>throw new Error('boom')</script>\
         <script>for (let i = 0; i < 30; i++) console.info('n' + i)</script>",
    );
    let rendered = renderer
        .render(document, 320, 1, VisualMode::Dark)
        .await
        .unwrap();
    let console = &rendered.console;
    assert_eq!(console.len(), 20, "{console:?}");
    assert_eq!(console[0].level, "log");
    assert_eq!(console[0].text, "hello 42");
    assert_eq!(console[1].level, "warning");
    assert_eq!(console[1].text, "careful");
    assert_eq!(console[2].level, "error");
    assert_eq!(console[2].text, "bad");
    assert_eq!(console[3].level, "error");
    assert!(console[3].text.contains("boom"), "{:?}", console[3]);
    assert!(
        !console[3].text.contains("consort-visual.localhost"),
        "{:?}",
        console[3]
    );
    assert_eq!(console[4].level, "info");
    // 4 messages, then 30 info messages: 16 fit and 14 do not.
    assert_eq!(rendered.omitted, 14);
    renderer.stop().await;
}

#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_height_table_follows_the_width() {
    let renderer = renderer();
    let text = "Visuals wrap their text at the width of the column. ".repeat(40);
    let document = page(&format!(
        "<p style=\"margin:0;font:16px/20px sans-serif\">{text}</p>"
    ));
    let started = Instant::now();
    let heights = renderer
        .measure(document.clone(), &[320, 1000], VisualMode::Dark)
        .await
        .unwrap();
    eprintln!(
        "height table at 2 widths with the browser start: {:?}",
        started.elapsed()
    );
    assert_eq!(heights.len(), 2);
    assert_eq!(heights[0].width, 320);
    assert_eq!(heights[1].width, 1000);
    assert!(heights[0].height > heights[1].height, "{heights:?}");
    let started = Instant::now();
    let all = renderer
        .measure(
            document,
            &consort_proto::model::VISUAL_WIDTHS,
            VisualMode::Dark,
        )
        .await
        .unwrap();
    eprintln!("height table at 10 widths: {:?}", started.elapsed());
    assert_eq!(all.len(), 10);
    assert_eq!(all[0], heights[0]);
    assert_eq!(all[8], heights[1]);
    // The table does not grow as the width grows.
    assert!(
        all.windows(2).all(|w| w[0].height >= w[1].height),
        "{all:?}"
    );
    renderer.stop().await;
}

#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_page_cannot_reach_loopback() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // Count only the requests of this page. A browser of another test that
    // is still stopping can connect to this port when the system gives it
    // out again (it was that browser's proxy port), and it speaks SOCKS.
    let accepted = Arc::new(AtomicUsize::new(0));
    let others = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let counter = accepted.clone();
    let seen = others.clone();
    let server = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut head = [0u8; 256];
            let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut head))
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or(0);
            let text = String::from_utf8_lossy(&head[..read]).into_owned();
            let ours = ["/a.png", "/b.png", "/x ", "/y "]
                .iter()
                .any(|path| text.starts_with("GET ") && text.contains(path));
            if ours {
                counter.fetch_add(1, Ordering::SeqCst);
            } else {
                seen.lock()
                    .unwrap()
                    .push(format!("{:?}", &head[..read.min(16)]));
            }
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 2\r\n\r\nok")
                .await;
        }
    });
    let document = page(&format!(
        "<img src=\"http://127.0.0.1:{port}/a.png\">\
         <img src=\"http://localhost:{port}/b.png\">\
         <script>\
         for (const url of ['http://127.0.0.1:{port}/x', 'http://localhost:{port}/y']) {{\
           const request = new XMLHttpRequest();\
           try {{ request.open('GET', url, false); request.send(); console.log('reached ' + request.status); }}\
           catch (e) {{ console.log('blocked ' + e.name); }}\
         }}\
         </script>"
    ));
    let renderer = renderer();
    let rendered = renderer
        .render(document, 320, 1, VisualMode::Dark)
        .await
        .unwrap();
    let texts: Vec<_> = rendered.console.iter().map(|m| m.text.as_str()).collect();
    assert!(
        texts.iter().filter(|t| t.starts_with("blocked")).count() == 2,
        "{texts:?}"
    );
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        0,
        "the page reached the local listener"
    );
    let others = others.lock().unwrap().clone();
    if !others.is_empty() {
        eprintln!("connections that were not from this page: {others:?}");
    }
    server.abort();
    renderer.stop().await;
}

#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_starts_again_after_the_idle_stop() {
    let mut config = RendererConfig::with_path(chrome());
    config.idle_stop = Duration::from_secs(1);
    let renderer = Renderer::new(config);
    let document = page("<div style=\"height:100px\"></div>");
    renderer
        .render(document.clone(), 320, 1, VisualMode::Dark)
        .await
        .unwrap();
    let first = renderer.current().await.unwrap();
    let profile = first.profile_dir().to_path_buf();
    let pid = first.pid();
    assert!(profile.is_dir());
    drop(first);
    assert!(renderer.is_running().await);

    // The idle stop asks the browser to quit, waits for it and deletes the
    // profile. Under load that takes a few seconds.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while (renderer.is_running().await || profile.exists()) && std::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!renderer.is_running().await);
    assert!(renderer.current().await.is_none());
    assert!(!profile.exists(), "the profile is still there");

    let again = renderer
        .render(document, 320, 1, VisualMode::Dark)
        .await
        .unwrap();
    assert_eq!(again.height, 100);
    let second = renderer.current().await.unwrap();
    assert_ne!(second.pid(), pid);
    assert_ne!(second.profile_dir(), profile);
    drop(second);
    renderer.stop().await;
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_starts_again_after_a_crash() {
    let renderer = renderer();
    let document = page("<div style=\"height:100px\"></div>");
    renderer
        .render(document.clone(), 320, 1, VisualMode::Dark)
        .await
        .unwrap();
    let browser = renderer.current().await.unwrap();
    let pid = browser.pid().unwrap();
    assert!(consort_harness::process::kill_group(pid));
    // Wait until the connection closes.
    let deadline = Instant::now() + Duration::from_secs(5);
    while browser.is_alive() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!browser.is_alive());
    let old_profile = browser.profile_dir().to_path_buf();
    drop(browser);

    let again = renderer
        .render(document, 320, 1, VisualMode::Dark)
        .await
        .unwrap();
    assert_eq!(again.height, 100);
    assert_ne!(renderer.current().await.unwrap().pid(), Some(pid));
    assert!(
        !old_profile.exists(),
        "the profile of the crashed browser is still there"
    );
    renderer.stop().await;
}

#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_renders_two_pages_at_once() {
    let renderer = renderer();
    let a = page("<div style=\"height:150px\"></div>");
    let b = page("<div style=\"height:250px\"></div>");
    // Start the browser first, so the time below is only the renders.
    renderer
        .render(a.clone(), 320, 1, VisualMode::Dark)
        .await
        .unwrap();
    let started = Instant::now();
    let (first, second, third) = tokio::join!(
        renderer.render(a, 320, 1, VisualMode::Dark),
        renderer.render(b.clone(), 640, 2, VisualMode::Light),
        renderer.measure(b, &[320, 720], VisualMode::Dark),
    );
    eprintln!(
        "three jobs at once, two pages at a time: {:?}",
        started.elapsed()
    );
    assert_eq!(first.unwrap().height, 150);
    let second = second.unwrap();
    assert_eq!(second.height, 250);
    assert_eq!(png_size(&second.png), (1280, 500));
    let third = third.unwrap();
    assert_eq!(
        third.iter().map(|h| h.height).collect::<Vec<_>>(),
        [250, 250]
    );
    renderer.stop().await;
}

#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_times_out_a_page_that_never_ends() {
    let mut config = RendererConfig::with_path(chrome());
    config.render_timeout = Duration::from_secs(3);
    let renderer = Renderer::new(config);
    let started = Instant::now();
    let err = renderer
        .render(
            page("<script>while (true) {}</script>"),
            320,
            1,
            VisualMode::Dark,
        )
        .await
        .unwrap_err();
    assert_eq!(err, RenderError::Timeout(Duration::from_secs(3)));
    assert!(started.elapsed() < Duration::from_secs(15));
    // The browser still works.
    let ok = renderer
        .render(
            page("<div style=\"height:90px\"></div>"),
            320,
            1,
            VisualMode::Dark,
        )
        .await
        .unwrap();
    assert_eq!(ok.height, 90);
    renderer.stop().await;
}

#[tokio::test]
#[ignore = "needs CONSORT_TEST_CHROME"]
async fn visual_real_keeps_the_main_frame_on_the_page() {
    let renderer = renderer();
    let document = page(
        "<div style=\"height:120px\"></div><script>location.href = 'https://example.com/';</script>",
    );
    let rendered = renderer
        .render(document, 320, 1, VisualMode::Dark)
        .await
        .unwrap();
    assert_eq!(rendered.height, 120);
    renderer.stop().await;
}

#[tokio::test]
async fn visual_renderer_reports_an_installing_browser() {
    let renderer = Renderer::new(RendererConfig::new(|| ChromeSource::Installing));
    let err = renderer
        .render(page("<p>x</p>"), 320, 1, VisualMode::Dark)
        .await
        .unwrap_err();
    assert_eq!(err, RenderError::Installing);
    assert_eq!(err.to_string(), PREVIEW_VISUAL_INSTALLING);
    let failed = Renderer::new(RendererConfig::new(|| {
        ChromeSource::Failed("the download failed".to_owned())
    }));
    let err = failed
        .measure(page("<p>x</p>"), &[320], VisualMode::Dark)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        RenderError::Unavailable("the download failed".to_owned())
    );
    assert!(!failed.is_running().await);
}
