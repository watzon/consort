# Research: HTML visuals in the chat

Date: 2026-11-01. Sources: T3 Code `main` at `fa31b66b` (2026-11-01), the installed T3 Code Nightly 0.1.6-nightly.20261030, the crates in the local cargo registry, GitHub and crates.io. No Consort code changed.

## Question

T3 Code shows HTML pages (charts, diagrams, mockups) inline in a thread. T3 Code is an Electron app, so each page goes in an iframe. Consort draws with GPUI, and GPUI has no HTML engine. How can Consort show the same pages?

## Verdict

The user changed parts of this verdict. See [Decisions](#decisions-2026-11-01).

- Copy the T3 Code design for all parts except the frame. That includes the two tools, the stored file, the small reference in the timeline, the theme variables, the size protocol and the preview browser. None of these parts needs Electron.
- Do not put live webviews in the timeline now. On `gpui-pre` 0.3.10, a native webview is drawn above all GPUI content. It covers the fades, menus, popovers and dialogs, and it does not clip in a list that scrolls. On Windows, it may not show at all.
- Show each visual in the timeline as an image. The session host renders the image with headless Chromium. When the user clicks the image, the right panel shows the live page in one native webview.
- Later, test two ways to get live visuals in the timeline: Servo rendered offscreen into GPUI images, and GPUI Fast window composition.

## What T3 Code does

### Tools

- `html_preview(html, width, appearance)` returns a PNG, the content height and the console output. The agent uses it to check a page before the user sees it.
- `html_render(html, title, height)` shows the page in the thread.
- Both tools run in the T3 server (`apps/server/src/mcp/toolkits/html/`). They are read-only, so plan and read-only modes allow them.
- The system prompt tells the agent to answer trends and comparisons with a chart, and to call `html_preview` before `html_render` (`packages/provider-core/src/server/orchestrationInstructions.ts:21-23`). The `html_render` result tells the agent not to describe the page in its reply.

### Storage

- At publish, the server prepares the page (`apps/server/src/htmlRender/HtmlRender.ts`).
  - It refuses `vh`, `svh`, `lvh` and `dvh` units. The frame grows to fit its content, so a page sized to the viewport grows again and again.
  - It inlines local images as `data:` URIs. It checks each file by its magic bytes, not its name, so a renamed secret cannot leave the machine. The limits are 10 MiB for one image and 25 MiB for the page.
  - It injects a bootstrap: a theme style, a size reporter and a link interceptor.
- The server writes the HTML as an attachment file. If the browser is installed, it also measures the height at 9 widths, from 320 to 1144 px.
- The thread keeps only `{attachmentId, title, height, heights}` in the output of the tool call. A replay fetches the file again.

### Frame

- The page loads in an iframe from a signed URL. The iframe has `sandbox="allow-scripts allow-forms"` and never `allow-same-origin`. The response also has `Content-Security-Policy: sandbox allow-scripts allow-forms allow-popups allow-downloads`. So the page has an opaque origin. Scripts run, but the page cannot read the app session. The page can use the network.
- The theme style is the first child of `<head>`, so the page CSS can override it. The base sheet gives `html` an 8 px padding (the gutter), the theme background, text and font.
- The host and the page use a small JSON-RPC protocol over `postMessage`, with MCP Apps method names:
  - `ui/notifications/size-changed`: the page sends its height from a `ResizeObserver`.
  - `ui/open-link`: the page sends a link click. The host accepts it only from the frame window, with the frame focused, after a user gesture, and for `http` or `https`.
  - `ui/notifications/host-context-changed`: the host sends new theme variables. The page updates with no reload.
- The live height from the page wins, clamped to 80..20000 px. Next comes the height table from the server, then the height that the agent gave.
- A hover button opens the page full screen, with zoom from 50 to 200 percent.

### Preview browser

- T3 Code uses `chrome-headless-shell` from Chrome for Testing, pinned by version and SHA-256 (`apps/server/src/preview/PreviewBrowser.ts`). It downloads the browser on first use. The first call says that the browser is still installing and asks the agent to try again.
- It drives the browser over the DevTools protocol on a pipe. A local SOCKS5 proxy refuses private, loopback and link-local addresses.
- At most 2 browsers run at once. A capture times out after 20 s, and the console output has at most 20 messages.

### Mobile

- The mobile app uses `react-native-webview` with the same URL. It cannot report a live height, so it uses the height table from the server. Content past that height scrolls.

## Why the frame is the hard part in GPUI

- GPUI's `Window` implements `HasWindowHandle` (`gpui-pre-0.3.10/src/window.rs:7616`). So `wry::WebViewBuilder::build_as_child` works with no `unsafe` in a Consort crate.
- On macOS, wry adds the `WKWebView` as a subview of GPUI's view. That view is layer-backed with a `CAMetalLayer`, so the webview is drawn above all GPUI content.
- The `gpui-webview` 0.8.1 README says: "On gpui-pre, the native view ... covers GPUI content in its bounds, including popovers, dialogs, menus and tooltips. It cannot be clipped either ... It is hidden only when it is scrolled out of view entirely."
- On Windows, GPUI makes a topmost DirectComposition target: `CreateTargetForHwnd(hwnd, true)` (`gpui-pre-windows-0.3.10/src/directx_renderer.rs:1017`). A topmost tree is drawn above child windows. Zed issue #66368 reports a blank WebView2 in a `gpui-pre` app.
- On Linux, the webview needs GTK and X11 or XWayland. `gpui-webview` pumps the GTK loop every 16 ms. Wayland is not supported.
- Each `WKWebView` gets its own WebContent process until an unpublished limit.
- wry activates the app when it makes a webview (`wry-0.57.0/src/wkwebview/mod.rs:694-702`). A webview made while Consort is in the background can bring it to the front.
- A child webview returns NO from `performKeyEquivalent`, so Cmd+C in the page needs an Edit menu item.

So a webview in the timeline breaks GPUI rules 3 and 4: the timeline would get a hard edge under the fades, and menus would open under the page.

## Options

| Option | Scripts | Live in the timeline | Platforms | Cost | Verdict |
|---|---|---|---|---|---|
| Native webview in the timeline | Yes | Yes, but covers overlays and does not clip | macOS, Windows unclear, X11 | Low code, high risk | No |
| Native webview on GPUI Fast composition | Yes | Yes on macOS and X11 | Windows still covers overlays | The whole app moves to a fork | Test later |
| Image from the session host, live page in the panel | Yes | Image only | All, for the image | Headless Chromium on the hub machine | Yes, first |
| Image from a client snapshot (`takeSnapshot`) | Yes | Image only | macOS, iOS | All 96 `WKWebView` methods in `objc2-web-kit` 0.3.2 are `unsafe` | No, breaks the `unsafe` rule |
| Servo offscreen into GPUI images | Yes, SpiderMonkey | Yes, fully drawn by GPUI | macOS, Windows, Linux, Android | Very large build, monthly API changes | Test later |
| CEF offscreen | Yes | Yes | Desktop | 135 to 315 MB, wants the macOS event loop | No |
| Blitz | No (draft PR #491 with Boa) | Static HTML and CSS only | All | Medium, beta | No |
| Only SVG, Mermaid or a chart spec | No | Yes | All | Low | Not HTML. Useful as fast paths |

Notes:

- GPUI Fast is a fork of GPUI by the GPUI Kit maintainers. GPUI Kit 0.8.1 has a `gpui-fast` feature. Its window composition is a port of Zed PR #62379, which is still open. With it, GPUI overlays draw above a webview on macOS and X11, and a container clips the webview.
- Servo is at 0.7.0 (2026-10-05). The `iced_servo` crate shows the offscreen path: `SoftwareRenderingContext`, then a pixel read-back, then a texture. In GPUI the pixels go to `img(ImageSource::Render(..))`. One Servo runtime is allowed per process. A first build takes more than 30 minutes and needs Clang 19. Nobody has checked how well it renders Chart.js or D3 pages.
- GPUI draws SVG in color through `img` with `ImageFormat::Svg` and resvg. The `svg()` element is a one-color mask, so it is not for charts.
- `merman` is a pure Rust Mermaid renderer that outputs SVG. Zed uses it in `crates/mermaid_render`.

## Recommendation

### Hub, session host and worker

These parts are the same for each client, desktop or mobile.

1. Add two hub tools that match `html_preview` and `html_render`.
2. Prepare the page at publish, as T3 Code does. Refuse viewport height units. Inline local images by magic bytes. The worker reads the images from the workspace, as it does for `publish_artifact`. Inject the bootstrap. Store the page as a blob.
3. Keep only a reference in the timeline: the blob hash, the title, the height and the height table.
4. Map theme roles to the CSS variables of T3 Code, so that pages written for T3 Code also work in Consort. For example, `surface_canvas` becomes `--background`, `text_primary` becomes `--foreground`, `surface_card` becomes `--card`, `border_subtle` becomes `--border`, `accent_fill` becomes `--primary`, and `status_failed` becomes `--destructive`. Themes have no chart colors, so `--chart-1` to `--chart-6` need new roles or a fixed set.
5. Supply the app fonts (Atkinson Hyperlegible Next and Mono) to the page with `@font-face`. Without them, the heights from the server do not match the client.
6. Install `chrome-headless-shell` as a managed tool in `consort-tools`, pinned by version and SHA-256. The session host uses it for the preview tool and for images.
7. Let the client ask for an image of a visual at its width, mode, theme and scale. The session host renders the image once and keeps it as a blob.

### Desktop

1. In the timeline, show the image at the width of the message. While it renders, show a placeholder.
2. A click opens the live page in the right panel, as a document opens there now. Use one webview from `gpui-webview` 0.8.1, or from wry directly.
3. Configure the webview: a data store that is not kept, a navigation handler that refuses all pages but this one, and an IPC handler for size and link messages. Links go to the system browser after the same checks as T3 Code.
4. Hide the webview while a menu, a popover or a dialog is open above the panel.
5. Where no webview works (Windows until the test passes, Linux on Wayland), the panel offers Open in browser. It writes the page to a temporary file and opens it in the system browser.
6. The visual needs a context menu in GPUI rule 2 and copy that follows the design system.

### Mobile, later

Use the native webview of each platform, as the T3 Code mobile app does. The height table from the server covers the missing live height.

## Tests to run before a build plan

1. macOS: a panel webview from `gpui-webview` 0.8.1 on `gpui-pre` 0.3.10. Check the bounds while the panel resizes, scrolling in the page, Cmd+C, focus back to the composer, and hide and show around menus and dialogs.
2. Windows: does a child WebView2 show on `gpui-pre` 0.3.10? Test with and without `GPUI_DISABLE_DIRECT_COMPOSITION=1`.
3. Headless Chromium on a Linux hub and in the Docker image: shared libraries, fonts, memory, and the time for one image.
4. Servo 0.7: render 12 real model pages (Chart.js, D3, Mermaid, canvas and SVG written by hand) into a GPUI window. Measure the build time, the binary size, the frame time and the input.
5. GPUI Fast: does the app run with the `gpui-fast` feature? Does a webview in the timeline clip and stay under the fades?

## Decisions (2026-11-01)

The user decided these after the first version of this note. They change the recommendation above in the ways that follow.

1. **Live by default.** A visual in the timeline is live while all of it is between the fades and nothing is over it. It shows its image while a menu, a popover, a dialog or the light box is over it, or while a part of it is under the top tint, under the bottom fade or off screen. A visual off screen is not drawn. So the image must exist before the live page can go away: the session host renders it at the width, the theme and the scale of the client.
2. **Network.** A page can use the network, as in T3 Code. The first version of this note recommended a block. The user saw no reason for one.
3. **Place in the timeline.** A visual is a timeline item of its own.
4. **Full screen.** A visual opens in the light box, not in the right panel. The light box becomes one component for pictures and visuals. A click outside the picture or the page closes it, and its top buttons are icon buttons. While the light box shows a visual, the timeline item shows the image.
5. **Theme and templates.** Visuals follow the app theme and its hand-drawn style. A default theme and templates for common charts come with the app. The theme gets new roles `chart.1` to `chart.6`.
6. **Term.** "Visual" is in `CONTEXT.md`.

The designs are in Paper, page "Redesign, Oct 30", section 15 Visuals: G1 to G7. G7 has the chart index by question, the list of charts that a visual never uses, and 18 more templates. With G1 and G6, the kit has a template for each chart in the index.

Consequences to check in the build plan:

- A visual taller than the clear part of the timeline is never live there. Open full screen shows it live.
- The image is the page as it first drew, so hover effects stop while the image shows.
- The Paper designs mix two theme colors as a glaze: one color with transparency over the other. Paper cannot draw `color-mix()` of two variables. A browser can, so the templates can use either way.
