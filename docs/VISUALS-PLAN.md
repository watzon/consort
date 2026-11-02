# Plan: visuals

Status: asked for by the user on 2026-11-01 ("Let's build this feature out"). The research and the user's decisions are in [research/visualizations.md](research/visualizations.md). The designs are in Paper, page "Redesign, Oct 30", section 15 Visuals, artboards G1 to G7. The user approved them with no notes.

A **Visual** (CONTEXT.md) is an HTML page that the assistant shows in the timeline as an item of its own. It is live while nothing covers it, and an image of the same page when something does.

## What the user asked for

1. Built-in HTML visuals in the chat, based on the T3 Code design (`html_preview` and `html_render`).
2. Live by default. Show the image while something is over the visual (a menu, a popover, a dialog, the light box). Draw nothing while the visual is off screen.
3. The page can use the network.
4. A visual is a timeline item of its own.
5. One light box for pictures and visuals. A click outside the picture or the page closes it. The top buttons are icon buttons. While the light box shows a visual, the timeline item shows the image.
6. A default theme and templates for common charts, in the app theme and its hand-drawn style.

## Parts

| Part | Crate | What it does |
|---|---|---|
| Wire types | `consort-proto` | `model::visual`: the notice body, the limits, the height table, the page style. The tools `preview_visual` and `show_visual`. `ToolContent::Image`. The client request `visual.render`. The session host command `visual.render` |
| Chart roles, fonts | `consort-theme` | Roles `chart.1` to `chart.6` and `chart.ink` in both modes of both themes. A `gpui` feature (on by default), so that the theme files, the roles and the fonts build without GPUI. The font files move to `design/fonts/` |
| Page contract | `consort-visual` (new) | Pure, no I/O. The page check (size, viewport height units), the local image references and their inlining, the bootstrap (theme variables, fonts, base sheet, size reporter, link catcher, the ink kit), the height choice, the CDP expressions |
| Ink kit | `consort-visual/kit/` | The hand-drawn chart kit in JavaScript, `Ink`, from the G6 and G7 templates. The bootstrap puts it in every page |
| Managed browser | `consort-tools` | `chrome-headless-shell` from Chrome for Testing 156.0.8078.80, pinned by size and SHA-256 per platform |
| Renderer | `consort-session-host` | One headless browser over the DevTools protocol on a WebSocket. A SOCKS5 proxy that refuses private addresses. `preview_visual` (answered here), `show_visual` (prepared here, then sent to the hub), `visual.render` |
| Store and routing | `consort-hub` | `show_visual` posts the notice. `visual.render` from a client: checks access, looks in the image cache, asks the session host |
| Client | `consort-client` | `VisualView` in the timeline, `Client::render_visual`, the mock content |
| Desktop | `consort-desktop` | The timeline item, the live webview (macOS), the cover rule, the light box, the context menu |

## Data

### The notice

A visual is a message of kind `notice` with `kind: "visual"`. A new `MessageKind` would break older peers, because `MessageKind` has no unknown variant. A draft card and a Monid call row are notices for the same reason.

- The body is `VisualNotice`: `{kind: "visual", text, title, height, heights}`.
  - `text` is the title. A client with no visuals shows it as a plain notice.
  - `height` is the height that the assistant gave, clamped to 80..2000 px. It is the last fallback.
  - `heights` is the height table: `[{width, height}]` at the widths of `VISUAL_WIDTHS`. It is empty when the session host had no browser at publish.
- `Message.attachments` has one `BlobRef`: the prepared page, `text/html`, named after the title.
- The page is not a channel file. It is a part of the message, as the files of a Monid call row are.
- A task session posts the notice in its task thread. A coordinator posts it in its channel. The rules are those of `show_draft`.

### The prepared page

The session host prepares the page before it uploads it:

1. Check the size: at most 512,000 characters of HTML from the assistant.
2. Refuse `vh`, `svh`, `lvh` and `dvh` in `<style>` blocks and `style=` attributes. The item grows to fit its page, so a page sized to the viewport grows again and again.
3. Inline local images (`src` or `url()` with an absolute path or `file://`) as `data:` URIs. The worker reads each file from the workspace. The session host checks each file by its magic bytes (PNG, JPEG, GIF, WebP, SVG text), so a renamed secret cannot leave the machine. At most 10 MiB for one image and 25 MiB for the page.
4. Store the page with no bootstrap. Each client and the renderer add the bootstrap when they load the page, with the theme of that moment.

### The height table

The session host measures the page at the widths 320, 375, 430, 480, 560, 640, 720, 860, 1000 and 1144 px, in one page load. The desktop chooses a height in this order:

1. The height that the live page reports, clamped to 80..20,000 px.
2. The height of the image at this width.
3. The height table, by linear interpolation between the two nearest widths.
4. The height that the assistant gave.

### The page style

`VisualStyle { mode, variables }` is the theme of one client: dark or light, and the CSS variables in the order of `VISUAL_VARIABLES`. The desktop makes it from its active theme. The renderer uses it as it is. The page contract in `consort-visual` names the variables:

| Variable | Role |
|---|---|
| `--background` | `surface.canvas` |
| `--foreground` | `text.primary` |
| `--muted` | `surface.card` |
| `--muted-foreground` | `text.secondary` |
| `--subtle-foreground` | `text.tertiary` |
| `--card` | `surface.card` |
| `--card-foreground` | `text.primary` |
| `--popover` | `surface.raised` |
| `--popover-foreground` | `text.primary` |
| `--border` | `border.subtle` |
| `--input` | `border.strong` |
| `--primary` | `accent.fill` |
| `--primary-foreground` | `accent.on_fill` |
| `--accent` | `accent.surface` |
| `--accent-foreground` | `accent.text` |
| `--ring` | `accent.text` |
| `--destructive` | `status.failed` |
| `--success` | `status.done` |
| `--warning` | `status.working` (Spruce: blue; the name follows T3 Code) |
| `--info` | `status.scheduled` |
| `--chart-1` to `--chart-6` | `chart.1` to `chart.6` |
| `--chart-ink` | `chart.ink` |
| `--font-sans` | Atkinson Hyperlegible Next, then system-ui |
| `--font-mono` | Atkinson Hyperlegible Mono, then ui-monospace |

### Chart roles

The first three are the pigments of the Three voices mark. Gold is not first, because the accent means "your turn". Slate is for "Other". `chart.ink` is the text on a chart fill: the canvas in dark mode, and white in light mode.

| Role | Spruce dark | Spruce light | Velvet dark | Velvet light |
|---|---|---|---|---|
| `chart.1` lapis | `#77B4ED` | `#2F74C0` | `#77B4ED` | `#2F74C0` |
| `chart.2` verdigris | `#80C7A8` | `#2E8466` | `#80C7A8` | `#2E8466` |
| `chart.3` gold | `#E7BE62` | `#B08419` | `#E7BE62` | `#B08419` |
| `chart.4` cobalt violet | `#B2A0D6` | `#7559A6` | `#B2A0D6` | `#7559A6` |
| `chart.5` madder | `#E3929C` | `#B9506A` | `#E3929C` | `#B9506A` |
| `chart.6` slate | `#8E9A94` | `#7A8781` | `#968E97` | `#857B85` |
| `chart.ink` | `#131E1A` | `#FFFFFF` | `#1F181F` | `#FFFFFF` |

New contrast rules: each pigment on `surface.canvas` at 3.0 (a graphic on the page background, WCAG 1.4.11), and `chart.ink` on each pigment at 3.0 (bold numbers in a bar). Gold on `surface.card` in light mode is 2.98, so a chart never sits on a card fill.

### The image cache

The hub keeps one row per image: the key is the SHA-256 of the page hash, the width, the scale and the style. The value is the blob of the PNG and its height. A second client with the same theme gets the same image.

## Wire

### Tools

| Tool | Audience | Answered by | What it does |
|---|---|---|---|
| `preview_visual` | every session | the session host | Renders the page at a width (default 720) in dark or light mode. Returns the PNG, the height and the console output. Nothing is posted |
| `show_visual` | every session | the session host prepares, the hub posts | Posts the page in the timeline (a coordinator) or in the task thread (a task session) |

- `preview_visual { html, width?, appearance? }`. `width` is 320..1144. `appearance` is `dark` or `light`, default `dark`.
- `show_visual { html, title, height? }`. `title` has 1 to 200 characters. `height` is 80..2000.
  - The session host adds `page` (a `BlobRef`) and `heights`, and sends `html` as an empty string. The model does not see these fields.
  - The result has the message ID and says: "The visual is in the conversation. Do not describe it in your reply."
- When the browser is not installed yet, `preview_visual` starts the install and says: "The preview browser is installing. Try again in a minute." `show_visual` posts with no height table.
- The turn gates of research, suggestion and voice turns refuse both tools. Proactive turns use the rules of `post_message`.

### `visual.render` (client to hub)

`VisualRender { message_id, width, scale, style }` returns `VisualImage { image: BlobRef, height }`.

- The caller must be able to read the message.
- `width` is 160..2400 CSS px, `scale` is 1..3.
- The hub looks in the image cache. On a miss, it sends `visual.render` to the session host and waits up to 60 s.
- Errors: `not_found` (no such visual), `unavailable` (no session host is online, or the browser is installing; the message says to try again).

### `visual.render` (hub to session host)

`VisualRenderCommand { page: BlobRef, width, scale, style }` returns `VisualImage`.

- The session host downloads the page from the hub, adds the bootstrap, renders it, uploads the PNG with `PUT /v1/blobs`, and returns the `BlobRef`.

## Renderer (session host)

- One browser process per session host, started on the first use and stopped after 5 minutes with no work. At most 2 pages render at once. A render times out after 20 s. The session host stops the browser when it stops. The process watcher records the process group of the browser, so the next start ends a browser that a killed session host left.
- The browser is the managed `chrome-headless-shell`. The first render that finds it missing starts the install in the background and fails at once: `preview_visual` says "The preview browser is installing. Try again in a minute.", `show_visual` posts with no height table, and `visual.render` answers `unavailable`. A failed install gives its message until the tool installs again or the session host restarts. `CONSORT_VISUAL_CHROME` (a path) names another browser and wins over the managed one: for a platform with no Chrome for Testing build (musl Linux), or a system Chromium.
- Flags: `--headless=new --remote-debugging-port=0 --user-data-dir=<temp> --no-first-run --no-default-browser-check --disable-gpu --hide-scrollbars --mute-audio --block-new-web-contents --disable-quic --proxy-server=socks5://127.0.0.1:<port> --proxy-bypass-list=<-loopback> --force-webrtc-ip-handling-policy=disable_non_proxied_udp`. On Linux also `--disable-dev-shm-usage`, because Docker gives a container 64 MB of `/dev/shm`. The WebSocket address comes from `<user-data-dir>/DevToolsActivePort`. A pipe would need file descriptors 3 and 4, and that needs `unsafe`.
- The sandbox of the browser stays on. The session host never turns it off by itself:
  - `--no-sandbox` comes only from the operator, with `CONSORT_VISUAL_NO_SANDBOX=1`.
  - The browser refuses to run as root with its sandbox. So a session host that runs as root on Linux without that setting does not start the browser. It says: "The preview browser does not run as root without its sandbox. Run the session host as another user, or set CONSORT_VISUAL_NO_SANDBOX=1 (read docs/deploy.md first)."
  - When the output of the browser says `No usable sandbox` (AppArmor or seccomp refuses the user namespaces), the error names docs/deploy.md, which has the steps for the preview browser.
  - A missing shared library gives its name and the list in docs/deploy.md.
- The SOCKS5 proxy resolves each name itself and refuses loopback, private, link-local, CGNAT, multicast and unspecified addresses.
- One render:
  1. `Target.createTarget` and `Target.attachToTarget` (flat).
  2. `Page.enable`, `Runtime.enable`, `Log.enable`, `Emulation.setEmulatedMedia` (`prefers-color-scheme`), a script that removes `RTCPeerConnection`, `Fetch.enable` for the page origin and for documents.
  3. `Emulation.setDeviceMetricsOverride { width, height: 800, deviceScaleFactor: scale }`.
  4. `Page.navigate` to `http://consort-visual.localhost/page.html`, answered by `Fetch.fulfillRequest` with the document. A navigation of the main frame away from the page gets an empty 204, so the frame keeps the page and shows no error page. A document of another site in a subframe may load. Any other request on the page origin fails.
  5. Wait for the load event, then `document.fonts.ready` and two animation frames (`SETTLE_EXPRESSION` of `consort-visual`).
  6. Measure (`MEASURE_EXPRESSION`), then `Page.captureScreenshot` with a clip of the measured height (at most 20,000 px).
  7. `Target.closeTarget`.
- The height table: after step 5, set each width of `VISUAL_WIDTHS`, wait two frames, and measure.
- The console output: `Runtime.consoleAPICalled`, `Runtime.exceptionThrown`, and `Log.entryAdded` (blocked loads and network errors, such as a request that the proxy refused, but not the level `verbose`). At most 20 messages of at most 500 characters. The page address becomes `page.html`, and an address on the page origin becomes its path.
- `preview_visual`: the session host answers it and sends nothing to the hub. The tool must be in the tool list of the session. The width is 320 to 1144 (default 720), the mode dark by default, the scale 1, and the style the default style of that mode. The answer is an image block with the PNG, then a text block with `PreviewVisualResult`. The image stops at 2000 px, because model APIs refuse or shrink a larger image in a long conversation. Then a third text block says that the image shows only the top of the page.
- `show_visual`: the session host checks the page and the title, and puts in the local images. Then it uploads the page and measures the height table at the same time. The table uses dark mode and the default style. Then it sends the call with `page`, `heights` and an empty `html`. Nothing goes to the hub when the page or an image is refused. When the browser installs or fails, the call goes with no `heights`.
- Local images: only files in the workspace of the session. An absolute path inside the workspace root becomes a path relative to it. A path outside gives "The image <path> is outside your workspace. Copy it into the workspace, then try again." A task session reads each file on its worker (`blob_put`, then a download from the hub, at most 10 MiB) with the root and the path rules from `workspace.open`. A coordinator reads its own folder on this machine, and a symlink may not lead out of it.
- `visual.render`: runs beside the link loop. The session host checks the width (160 to 2400), the scale (1 to 3) and the style, downloads the page (at most 25 MiB), checks its hash and that it is UTF-8, adds the bootstrap with the style of the client, renders it, and uploads the PNG as `visual.png`. Errors: `unavailable` while the browser installs or after a timeout, `invalid` for a bad request, `internal` with the message for any other failure.

## Ink kit

`Ink` is a small JavaScript library that the bootstrap puts in every page. It draws the templates of G6, G7 and G8 with the theme variables: wobbly strokes, pigment dabs, dashed pencil grid lines, glazes, and the brushstroke donut. The model calls it, for example `Ink.bars("#chart", {...})`, and can still write its own SVG or use any library.

- The geometry comes from the Python generators that drew the Paper designs. Their randomness has a fixed seed per chart, so the same data draws the same strokes on every render.
- The coordinator and task prompts get a section "Visuals": when to use one, preview first, the chart index by question, the never list, and the API of `Ink`.
- `kit/gallery.html` shows each template with the sample data of the designs. `kit/stress.html` shows each template with much more data. Both load `ink.js` by a relative path. `kit/examples/` has full pages that are written as an agent writes a visual, such as `radar.html`.

### The readout

Every chart has one readout, an HTML box inside the chart's box. It looks like the app's tooltip: `--popover`, a 1 px `--border`, `--radius-md`, 6 px 8 px of padding, 12 px text on 16 px lines, at most 240 px wide. The first line names the item in `--foreground` at weight 600. The next lines are in `--muted-foreground`, with a pigment dab before a series name. The kit writes it with `textContent`.

- It shows on a pointer hover, on a tap, and when the chart has keyboard focus. The chart takes focus (`tabindex="0"`). The arrow keys move through the marks in reading order, and Home and End go to the ends. A visually hidden `aria-live="polite"` region reads the readout. The `aria-label` summary stays on the chart.
- It hides when the pointer leaves, on a tap outside the chart, and on Escape. Escape calls `preventDefault()` only while a readout shows, so the light box does not also close.
- It sits above its mark and centered on it. With no room above, it goes below. With no room above or below, it goes beside the mark. It stays inside the chart's box, or inside the page when the box is too short, so the page never grows. A line chart puts it beside the hover line. A large area, such as a treemap tile or a sankey ribbon, puts it at the pointer.
- `pointermove` updates at most once each animation frame. A readout never draws the chart again.
- The other marks fade to 45 percent. On a line chart the nearest line gets thicker. Legend rows, line end labels and donut rows link both ways with their marks.
- `readout: false` turns it off. `readout: (item) => string | string[]` replaces the lines. `item.lines` has the default lines. An empty line is left out.

### Density

Each chart fills the width, grows in height with its data, and keeps text at 11 px or more. A label that does not fit is cut or left out only when the readout gives it.

- Bars, grouped, stacked and waterfall lie down in rows when a category gets less than about 28 px, or when a quarter of the names do not fit in two lines under the bars. `horizontal: true` or `false` forces the choice.
- Ranked shows all rows. `top: N` shows N and a "Show k more" row, which stays open.
- Line: the x labels take an even stride and always keep the first and the last. More than six series take a legend, and the series past six are dashed.
- Scatter: the dabs get smaller as the count grows, and past 400 points they are plain circles. The readout takes the nearest point within 14 px.
- Treemap: the height grows with the count (about `80 * sqrt(n)`, more at a narrow width, 200 to 680 px). A tile shows its name and value, its name, or nothing. Tiles with a side under 10 px or under 0.3 percent of the area join one slate tile "N more".
- Calendar: a long range wraps into rows of whole months, so each cell is 14 px or more.
- Week: a short event makes the hours taller, up to 56 px. A lane too narrow for three letters shows no title.
- Table: `maxRows` (default 12) and a "Show all k rows" button. A table wider than its box scrolls sideways inside it, with the first column fixed. A cell has three lines at most. A cut cell gets a readout.
- Numbers: more tiles wrap into even rows.

### Zoom and pan

- Sankey, tree and sequence lay out at the width where nothing is cut. When that is wider than the box, they scale down to fit, but not below 11/12, so 12 px text stays at 11 px or more. Past that, they cut labels to fit and offer zoom. Flow wraps into rows instead.
- Line, scatter and timeline zoom the x axis only. The y axis and the names stay. A line chart offers zoom when its points are closer than 5 px, a scatter past 400 points, a timeline when a day is under 6 px or a span is under 10 px.
- A heat map field zooms x and y together, and a drag pans in x and in y. It offers zoom when a cell is under 6 px. The shared layer pans up and down only for a chart that gives a y range in `pan` (`[x0, x1, y0, y1]`).
- `zoom: true` gives the zoom to any of these charts. `zoom: false` turns it off.
- Three 24 px buttons (zoom out, fit, zoom in) show at the top right on hover and on keyboard focus. The keys are `-`, `0` and `+`. A pinch (a wheel with `ctrlKey`) or Cmd or Ctrl with the wheel zooms around the pointer. A drag, a sideways wheel or Shift with the wheel pans.
- A plain vertical wheel always goes on to the page, so the conversation scrolls.
- Zoom never changes the height of the chart. The resting state is the fit view, so the image of the page is complete.

### Heat map

`Ink.heatmap(target, spec)` draws a value at each cell of a grid: a field (a map, or a profile under a path) or a matrix (two kinds of thing, such as day and hour). The design is G8.

- `grid` is rows of numbers from the top, with `null` for no data, or a grid string `{cols, rows, steps}`. The string has one character for each cell, row by row: the step index (`0` to `9`, then `A` to `Z`), `-` for below the first edge, and `.` for no data. `3(12)` is a run of 12 cells. White space is ignored. A 250 by 250 grid is about 62 KB, or about 32 KB with runs, so it fits in a page.
- `steps` are the edges, from low to high. A value at or past the last edge is in the last step. With no `steps`, the kit chooses about 5 round steps, and a zero in counts from zero is not painted. `diverging: true` uses `steps` for the size of a change on both sides of zero: madder below, lapis above, and no pigment near zero.
- The pigment is a glaze of one chart role. The first step is 10 to 26 percent (more when there are fewer steps), and the last is 100 percent (85 percent for a change). More is more pigment, so it is brighter in dark mode and darker in light mode. A cell with no data, or below the first edge, stays canvas. A tile below the first edge, or near zero, is an outline.
- A field draws its cells on a canvas: one pixel for each cell in an image, which `drawImage` scales with smoothing off at the device pixel ratio. The pigment comes from the CSS variables when the canvas is painted. A change of an attribute on `html` or `body`, such as a mode switch, paints it again. The grid, the ticks, the paths, the markers and the notes are SVG on top, and the labels are HTML or SVG text.
- A matrix draws SVG tiles with a gap of 3 px and a radius of 4 when a tile is 5 px wide or more and there are 20,000 cells or fewer. Else it is a field. The value sits in a tile when the tile is 40 px wide or more and the value fits. Its text is `--chart-ink` on a strong step, `--foreground` on a mid step, and `--muted-foreground` on a faint step.
- `panels` share one key. Each panel is a chart of its own in the shared layer, with its readout, its keys and its zoom. The panels sit side by side when each gets 300 px or more, and stack when the page is narrow. The key sits at the right of the panels: a column of steps that spans the height of the plot, 12 px or more for each step, with the edges, the unit and the notes under it. When that does not fit, the key is a row below the panels. A matrix and a change have the row below, with "Less" and "More" or `key: {low, high}`.
- The height: `aspect: "equal"` makes a unit as long across as up, up to 800 px. A number is the height over the width, 140 px at least. Else the cells are about square, from 120 to 560 px. A plot that is not `"equal"` grows taller, up to 1.8 times, when its notes find no room.
- Overlays use data units and are ink with a canvas halo. A path is solid up to `dashFrom` and dashed after it, smoothed with no overshoot. A marker is a start or a point (an ink dot), a moment (a gold dab), a place (a small square) or an end (an X). A note is text on a paper label. It takes the nearest place that stays in the plot and covers no other note, no marker, no leader and at most a little of a path: around its point at 8 to 72 px, then farther away (110 and 160 px), then at the top or the bottom of the plot, then with its first line only. A note away from its point gets a pencil leader. Notes that find no place go first in a second try. A note is never left out: with no place, it is drawn anyway and `console.warn` names it.
- The readout names the cell: the value (or the step, such as "35 to 40 dBZ", for a grid string), what the step means from `notes`, and the place (`x` and `y`, or the row and the column names). The hovered cell gets an ink outline. The arrow keys move one cell. `readout: (c) => [...]` gets `c.value`, `c.step`, `c.note`, `c.x`, `c.y`, `c.row` and `c.col`.

## Desktop

### The timeline item

- The title line: the visual icon, the title in `text.secondary`, and nothing else (G1).
- The page: full width of the timeline column, with large corners. It has no card border or fill (G1: "A visual has no card").
- The hover footer (G3): Open full screen, Copy image, Save image, and the time.
- States (G3): "Loading the visual" while there is no image and no live page. "The visual did not load" with Try again when the page or the image fails.
- The context menu (G2): Open full screen, Open in browser, then Copy image, Save image, Save as HTML.

### Live or image

A pure function decides each frame, so tests need no webview:

- **Live** when the window is active, the item is fully inside the clear part of the timeline (below `layout.top_tint`, above the bottom fade), and no overlay is open: a menu, a picker, a popover, a dialog, the light box, the switcher, the drawer or a full panel.
- **Image** when the item is on screen but not live, or when the page is taller than the clear part.
- **Nothing** when the item is off screen. Its webview is hidden, and after 30 s it is dropped.
- The image must exist before the live page can go. The desktop asks for the image as soon as the item is on screen. While there is no image, the item stays live where it can, and shows "Loading the visual" where it cannot.
- At most 4 webviews exist at once.

The live page uses `gpui-webview` 0.8.1 with `lb-wry` 0.53.3, on macOS only. On Windows and Linux the item always shows the image until the tests in the research note pass. The webview:

- has a data store that is not kept (incognito);
- refuses every navigation but the first page;
- sends `http` and `https` link clicks to the system browser, at most one each second;
- has an IPC handler for four messages only: the height, a link, Escape (a focused page gets every key) and a right click (so the app shows its planned menu, not the WebKit menu).

### The light box

One component for pictures and visuals (G4, G5).

- A click outside the picture or the page closes it. Escape closes it.
- The top buttons are icon buttons with tooltips: Save, Open, Close for a picture; Save image, Open in browser, Close for a visual.
- A visual shows live on a canvas sheet, at the width of the sheet. The timeline item shows the image while the light box is open.

## Order of work

1. Wire types, chart roles, fonts, `consort-visual` (the page contract), client view types.
2. In parallel: the managed browser, the renderer, the hub, the ink kit, the desktop.
3. Docs, the dev run, the review.

## Tests

- Proto: round trips of the notice, the tools and both `visual.render` bodies; the tool lists per audience.
- Theme: the new roles parse, pass their contrast rules, and map to the variables.
- `consort-visual`: the viewport unit check, image references and inlining, the magic bytes, the bootstrap order (the theme style is the first child of `<head>`), the height choice.
- Tools: the pinned table, the download and the hash check against a local server.
- Session host: the proxy refuses private addresses; `preview_visual` and `visual.render` against a real browser (ignored by default) and against a fake DevTools server; `show_visual` inlines an image that the worker reads.
- Hub: `show_visual` posts the notice in the channel and in a task thread; access and cache of `visual.render`; the turn gates.
- Client: the reducer makes a `VisualView`; `render_visual` returns the image file.
- Desktop: the live or image rule; the item states; the light box closes on a click outside; the menu items.
