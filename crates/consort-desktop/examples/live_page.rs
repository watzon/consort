//! The spike of the live page of a visual (docs/VISUALS-PLAN.md, "Live or
//! image"). It opens a window with a scrolling list. One row of the list
//! holds a live page with a small chart, from `src/ui/live_page.rs`. A
//! button opens a fake menu over the list, and a log at the right shows the
//! messages of the page and the keys that reach GPUI.
//!
//! Run it with `cargo run -p consort-desktop --example live_page`. With
//! "Cover rule on", the page follows `src/ui/visual_cover.rs`: it is live
//! only while all of its row is in the clear part of the list and the menu
//! is closed. With the rule off, the page shows wherever its row draws.
//!
//! On Windows and Linux it prints a message and exits.

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("The live page spike runs on macOS only. Visuals show their image on this platform.");
}

#[cfg(target_os = "macos")]
fn main() {
    spike::run();
}

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/ui/live_page.rs"]
mod live_page;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/ui/visual_cover.rs"]
mod visual_cover;

#[cfg(target_os = "macos")]
mod spike {
    use std::time::{Duration, Instant};

    use consort_theme::{ActiveTokens as _, Tokens};
    use gpui_kit::{
        AnyElement, AppContext as _, Context, Entity, FocusHandle, Hsla, InteractiveElement,
        IntoElement, KeyBinding, KeyDownEvent, ListAlignment, ListState, Menu, MenuItem, OsAction,
        ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
        Window, WindowBounds, WindowOptions, actions, div, list, point, px, size,
    };

    use crate::live_page::{LivePage, PageEvent};
    use crate::visual_cover::{Presence, TimelineArea, visual_presence};

    actions!(live_page_spike, [Quit, Copy, Paste, SelectAll]);

    /// The row of the list that holds the page.
    const PAGE_ROW: usize = 6;
    const ROWS: usize = 40;
    /// The spike draws a top tint and a bottom fade of these heights, as
    /// the timeline does with `layout.top_tint` and `layout.fade_bottom`.
    const TINT: f32 = 108.;
    const FADE: f32 = 32.;
    /// The height of the row of the page. A visual would take the height
    /// that its page reports.
    const PAGE_HEIGHT: f32 = 260.;

    pub fn run() {
        gpui_kit::application().run(|cx| {
            gpui_kit::init(cx);
            consort_theme::init(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-c", Copy, None),
                KeyBinding::new("cmd-v", Paste, None),
                KeyBinding::new("cmd-a", SelectAll, None),
            ]);
            // Copy, Paste and Select all send `copy:`, `paste:` and
            // `selectAll:` to the first responder, which can be the page.
            cx.set_menus(vec![
                Menu::new("Live page spike").items([MenuItem::action("Quit", Quit)]),
                Menu::new("Edit").items([
                    MenuItem::os_action("Copy", Copy, OsAction::Copy),
                    MenuItem::os_action("Paste", Paste, OsAction::Paste),
                    MenuItem::os_action("Select all", SelectAll, OsAction::SelectAll),
                ]),
            ]);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(gpui_kit::Bounds::centered(
                    None,
                    size(px(1100.), px(760.)),
                    cx,
                ))),
                titlebar: Some(gpui_kit::TitlebarOptions {
                    title: Some("Live page spike".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..WindowOptions::default()
            };
            let opened = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| Spike::new(window, cx))
            });
            if let Err(err) = opened {
                eprintln!("live_page: cannot open the window: {err}");
                cx.quit();
            }
        });
    }

    struct Spike {
        list: ListState,
        page: Option<Entity<LivePage>>,
        page_subscription: Option<Subscription>,
        /// When the last page started to build.
        started: Instant,
        rule: bool,
        menu_open: bool,
        presence: Option<Presence>,
        log: Vec<String>,
        focus: FocusHandle,
        reloads: u32,
        _activation: Subscription,
    }

    impl Spike {
        fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
            let activation = cx.observe_window_activation(window, |this, window, cx| {
                let active = window.is_window_active();
                this.note(format!("window active: {active}"));
                cx.notify();
            });
            let mut spike = Self {
                list: ListState::new(ROWS, ListAlignment::Top, px(200.)),
                page: None,
                page_subscription: None,
                started: Instant::now(),
                rule: true,
                menu_open: false,
                presence: None,
                log: Vec::new(),
                focus: cx.focus_handle(),
                reloads: 0,
                _activation: activation,
            };
            spike.create(window, cx);
            spike
        }

        fn note(&mut self, line: String) {
            println!("live_page: {line}");
            self.log.push(line);
            let extra = self.log.len().saturating_sub(40);
            self.log.drain(..extra);
        }

        fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            self.page = None;
            self.page_subscription = None;
            self.started = Instant::now();
            match LivePage::build(chart_page(0), window, cx) {
                Ok(page) => {
                    let built = self.started.elapsed();
                    self.note(format!("built in {} ms", built.as_millis()));
                    let _ = page.read(cx).set_dark(true, cx);
                    self.page_subscription =
                        Some(cx.subscribe(&page, |this, _, event: &PageEvent, cx| {
                            let line = match event {
                                PageEvent::Loaded => format!(
                                    "loaded {} ms after the build started",
                                    this.started.elapsed().as_millis()
                                ),
                                PageEvent::Message(text) => format!("page says: {text}"),
                                PageEvent::Refused(url) => format!("refused: {url}"),
                            };
                            this.note(line);
                            cx.notify();
                        }));
                    self.page = Some(page);
                }
                Err(err) => self.note(format!("the build failed: {err}")),
            }
            cx.notify();
        }

        /// Applies the cover rule to the page, from the bounds of its row.
        fn follow_rule(&mut self, window: &Window, cx: &mut Context<Self>) {
            let Some(page) = self.page.clone() else {
                return;
            };
            let viewport = self.list.viewport_bounds();
            let presence = match self.list.bounds_for_item(PAGE_ROW) {
                Some(row) => {
                    let area = TimelineArea::new(viewport, px(TINT), Some(px(FADE)));
                    let item = page_bounds(row);
                    visual_presence(
                        item,
                        area,
                        self.menu_open,
                        window.is_window_active(),
                        area.fits(item.size.height),
                    )
                }
                None => Presence::Hidden,
            };
            let presence = if self.rule { presence } else { Presence::Live };
            if self.presence != Some(presence) {
                self.presence = Some(presence);
                self.note(format!("presence: {presence:?}"));
            }
            page.update(cx, |page, cx| match presence {
                Presence::Live => page.show(cx),
                Presence::Image | Presence::Hidden => page.hide(cx),
            });
        }

        fn render_row(&mut self, ix: usize, t: &Tokens) -> AnyElement {
            let c = &t.colors;
            if ix != PAGE_ROW {
                return div()
                    .px(t.space.xl)
                    .py(t.space.sm)
                    .text_color(c.text_secondary)
                    .child(format!(
                        "Row {ix}. A message of the timeline, so the list has something to scroll."
                    ))
                    .into_any_element();
            }
            let live = self.presence == Some(Presence::Live);
            let mut frame = div()
                .relative()
                .h(px(PAGE_HEIGHT))
                .rounded(t.radius.lg)
                .border(t.border_width)
                .border_color(c.border_strong)
                .bg(c.surface_card)
                .overflow_hidden();
            if !live {
                frame = frame.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(c.text_tertiary)
                        .child("The image of the page shows here while it is not live."),
                );
            }
            if let Some(page) = self.page.clone() {
                frame = frame.child(page);
            }
            div()
                .px(t.space.xl)
                .py(t.space.sm)
                .child(frame)
                .into_any_element()
        }
    }

    /// The page fills its row, less the padding of the row.
    fn page_bounds(row: gpui_kit::Bounds<gpui_kit::Pixels>) -> gpui_kit::Bounds<gpui_kit::Pixels> {
        gpui_kit::Bounds::from_corners(
            point(row.left() + px(24.), row.top() + px(8.)),
            point(row.right() - px(24.), row.bottom() - px(8.)),
        )
    }

    fn button(
        id: &'static str,
        label: impl Into<SharedString>,
        t: &Tokens,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let c = &t.colors;
        div()
            .id(id)
            .px(t.space.md)
            .py(t.space.xs)
            .rounded(t.radius.md)
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(c.border_strong)
            .text_color(c.text_primary)
            .cursor_pointer()
            .hover(|s| s.bg(c.hover_raised))
            .child(label.into())
    }

    impl Render for Spike {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let t = cx.tokens().clone();
            let c = t.colors.clone();
            self.follow_rule(window, cx);
            let toolbar = div()
                .flex()
                .flex_wrap()
                .gap(t.space.sm)
                .p(t.space.md)
                .child(
                    button(
                        "menu",
                        if self.menu_open {
                            "Close menu"
                        } else {
                            "Open menu"
                        },
                        &t,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = !this.menu_open;
                        cx.notify();
                    })),
                )
                .child(
                    button(
                        "rule",
                        if self.rule {
                            "Cover rule on"
                        } else {
                            "Cover rule off"
                        },
                        &t,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.rule = !this.rule;
                        cx.notify();
                    })),
                )
                .child(
                    button("reload", "Load another page", &t).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.reloads += 1;
                            let html = chart_page(this.reloads);
                            if let Some(page) = this.page.clone() {
                                let result = page.update(cx, |page, cx| page.load_html(&html, cx));
                                this.note(format!("load_html: {result:?}"));
                            }
                        },
                    )),
                )
                .child(
                    button("create", "Make the page again", &t)
                        .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
                )
                .child(
                    button("later", "Make it again in 3 s", &t).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.note("the page is made again in 3 s".into());
                            // A webview made while the app is in the background
                            // may bring it to the front.
                            cx.spawn_in(window, async move |this, cx| {
                                cx.background_executor().timer(Duration::from_secs(3)).await;
                                let _ =
                                    this.update_in(cx, |this, window, cx| this.create(window, cx));
                            })
                            .detach();
                        },
                    )),
                )
                .child(button("drop", "Drop the page", &t).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.page = None;
                        this.page_subscription = None;
                        this.note("dropped the page".into());
                        cx.notify();
                    },
                )))
                .child(
                    button("to-page", "Scroll to the page", &t).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.list.scroll_to_reveal_item(PAGE_ROW);
                            cx.notify();
                        },
                    )),
                );
            let rows = list(
                self.list.clone(),
                cx.processor(|this, ix: usize, _, cx| {
                    let t = cx.tokens().clone();
                    this.render_row(ix, &t)
                }),
            )
            .size_full();
            let canvas = c.surface_canvas;
            let tint = |from: Hsla, to: Hsla| {
                gpui_kit::linear_gradient(
                    180.,
                    gpui_kit::linear_color_stop(from, 0.),
                    gpui_kit::linear_color_stop(to, 1.),
                )
            };
            let timeline = div()
                .id("timeline")
                .relative()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(rows)
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(TINT))
                        .bg(tint(canvas.opacity(0.95), canvas.opacity(0.))),
                )
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(px(FADE))
                        .bg(tint(canvas.opacity(0.), canvas)),
                )
                .when_menu(self.menu_open, &t);
            let log = div()
                .id("log")
                .flex_none()
                .w(px(360.))
                .h_full()
                .overflow_y_scroll()
                .p(t.space.md)
                .bg(c.surface_sidebar)
                .text_color(c.text_secondary)
                .children(self.log.iter().map(|line| div().child(line.clone())));
            div()
                .id("spike")
                .key_context("Spike")
                .track_focus(&self.focus)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    let key = &event.keystroke;
                    let cmd = if key.modifiers.platform { "cmd-" } else { "" };
                    this.note(format!("GPUI key: {cmd}{}", key.key));
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &Copy, _, cx| {
                    this.note("GPUI action: Copy".into());
                    // GPUI takes Cmd+C before the page. Read the selection
                    // of the page instead.
                    if let Some(page) = this.page.clone() {
                        let read = page.read(cx).read_selection(
                            |text| println!("live_page: selection of the page: {text}"),
                            cx,
                        );
                        if let Err(err) = read {
                            this.note(format!("read_selection failed: {err}"));
                        }
                    }
                    cx.notify();
                }))
                .on_mouse_down(
                    gpui_kit::MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.focus.focus(window, cx);
                    }),
                )
                .size_full()
                .flex()
                .flex_col()
                .bg(c.surface_canvas)
                .text_color(c.text_primary)
                .child(toolbar)
                .child(div().flex().flex_1().min_h_0().child(timeline).child(log))
        }
    }

    trait WhenMenu {
        fn when_menu(self, open: bool, t: &Tokens) -> Self;
    }

    impl WhenMenu for gpui_kit::Stateful<gpui_kit::Div> {
        /// A fake menu over the list, where the page row shows at the start.
        fn when_menu(self, open: bool, t: &Tokens) -> Self {
            if !open {
                return self;
            }
            let c = &t.colors;
            let items = [
                "Open full screen",
                "Open in browser",
                "Copy image",
                "Save image",
            ];
            self.child(
                div()
                    .absolute()
                    .top(px(260.))
                    .left(px(120.))
                    .w(px(240.))
                    .p(t.space.xs)
                    .rounded(t.radius.md)
                    .bg(c.surface_raised)
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .shadow(t.shadow_float())
                    .children(items.map(|item| {
                        div()
                            .px(t.space.sm)
                            .py(t.space.xs)
                            .text_color(c.text_primary)
                            .child(item)
                    })),
            )
        }
    }

    /// A small chart page with inline SVG. It reports its height and its
    /// focus, keys and copies to the app, and has links that the live page
    /// must refuse.
    fn chart_page(load: u32) -> String {
        let bars: String = [42, 66, 51, 80, 73, 95, 60]
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let x = 20 + i * 52;
                let y = 120 - h;
                format!(
                    "<rect x='{x}' y='{y}' width='36' height='{h}' rx='4' fill='#77B4ED' \
                     onclick=\"post('bar {i}')\"/>"
                )
            })
            .collect();
        format!(
            r#"<!doctype html><html><head><meta charset="utf-8"><style>
html,body{{margin:0;background:#131E1A;color:#E6EEE9;font:13px -apple-system,system-ui,sans-serif}}
body{{padding:12px 16px}}
h3{{margin:0 0 6px;font-size:15px}}
.row{{display:flex;gap:8px;align-items:center;margin-top:8px}}
button,input{{font:inherit}}
a{{color:#80C7A8}}
</style></head><body>
<h3>Disk use, load {load}</h3>
<svg width="380" height="124" viewBox="0 0 380 124">{bars}</svg>
<div class="row">
<button onclick="post('button click')">Send IPC</button>
<input id="box" placeholder="Type here">
<a href="https://example.com/">A link</a>
<a href="https://example.com/" target="_blank">A new window</a>
<button onclick="location.href='https://example.com/'">Go away</button>
</div>
<p id="text">Select these words, then press Cmd+C.</p>
<script>
function post(m){{window.ipc.postMessage(String(m));}}
const ro=new ResizeObserver(()=>post('height '+document.documentElement.scrollHeight+' at width '+innerWidth));
ro.observe(document.documentElement);
addEventListener('focus',()=>post('page focus'));
addEventListener('blur',()=>post('page blur'));
addEventListener('keydown',e=>post('page key: '+(e.metaKey?'cmd-':'')+e.key));
document.addEventListener('copy',()=>post('page copy: '+String(getSelection())));
post('script ran');
</script></body></html>"#
        )
    }
}
