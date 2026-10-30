//! A document in the right panel (Paper F1).
//!
//! A click on a `.md`, `.markdown` or `.txt` file (the file card in a chat
//! bubble, or a row of the Files tab) opens the file in the panel at the
//! right, rendered. The header has the file name in mono at the left, then
//! Copy, Save and Open in another app, and Close at the far right end. Under
//! it is one line with the glyph of the task that wrote the file, the task
//! name, the size and the date.
//!
//! The text comes through the same blob path as Open and Save
//! (`Client::fetch_blob`). The desktop never touches the network, and it
//! reads the cached file off the UI thread. While it loads, the panel says
//! so. A failure says what went wrong, and offers Try again and Open in
//! another app. A file larger than
//! [`consort_client::document::MAX_DOCUMENT_BYTES`] never loads: the panel
//! says so and offers Open in another app.
//!
//! Rule 4: the card that opened the panel closes it, and so does Close. The
//! card of the open document has the selected ground. Other file types open
//! in the app of the system, as before. See DECISIONS 736 and 737.

use consort_client::{AttachmentView, ChannelFileView, TaskStatus, document};
use consort_theme::Tokens;
use gpui_kit::{
    AnyElement, ClipboardItem, Context, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement as _, Styled, Window, div,
    prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Panel},
    ui::{
        parts::{
            ButtonKind, button, detail, icon_button, line, panel_header, selectable, status_glyph,
            text,
        },
        routines::panel_scroll,
    },
};

/// What the panel has of the file.
#[derive(Clone, Debug, PartialEq)]
pub enum DocumentState {
    /// The file is on its way from the hub.
    Loading,
    /// The text of the file.
    Ready(String),
    /// The download or the read failed. The text says what went wrong.
    Failed(String),
    /// The file is larger than the panel loads.
    TooLarge,
}

/// The document that the panel shows.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenDocument {
    pub sha256: String,
    pub name: String,
    pub size: u64,
    pub size_label: String,
    /// "Today" or "Oct 24", when the views know it.
    pub date_label: Option<String>,
    /// The task that wrote the file: its name and its status.
    pub task: Option<(String, TaskStatus)>,
    pub state: DocumentState,
}

impl OpenDocument {
    fn new(
        sha256: String,
        name: String,
        size: u64,
        size_label: String,
        date_label: Option<String>,
        task: Option<(String, TaskStatus)>,
    ) -> OpenDocument {
        let state = if document::fits(size) {
            DocumentState::Loading
        } else {
            DocumentState::TooLarge
        };
        OpenDocument {
            sha256,
            name,
            size,
            size_label,
            date_label,
            task,
            state,
        }
    }

    /// The line under the header: size and date, with the task before them.
    pub fn summary(&self) -> String {
        match &self.date_label {
            Some(date) => format!("{}, {}", self.size_label, date.to_lowercase()),
            None => self.size_label.clone(),
        }
    }
}

impl ConsortApp {
    /// Whether a file with this hash is the open document.
    pub fn document_is_open(&self, sha256: &str) -> bool {
        self.panel == Panel::Document && self.document.as_ref().is_some_and(|d| d.sha256 == sha256)
    }

    /// A click on a file card of a message. A document opens in the panel,
    /// and a click on the card of the open document closes the panel (rule
    /// 4). Any other file opens in the app of the system.
    pub fn click_attachment(&mut self, att: &AttachmentView, cx: &mut Context<Self>) {
        if !document::is_document(&att.name) {
            self.open_blob(att.sha256.clone(), att.size, att.name.clone(), cx);
            return;
        }
        let date = self
            .state
            .channel_files
            .iter()
            .find(|f| f.sha256 == att.sha256)
            .map(|f| f.date_label.clone());
        let task = att
            .artifact
            .as_ref()
            .map(|a| (a.task_name.clone(), a.status));
        self.toggle_document(
            OpenDocument::new(
                att.sha256.clone(),
                att.name.clone(),
                att.size,
                att.size_label.clone(),
                date,
                task,
            ),
            cx,
        );
    }

    /// A click on a row of the Files tab. Returns whether the file opened in
    /// the panel.
    pub fn click_channel_file(&mut self, file: &ChannelFileView, cx: &mut Context<Self>) {
        if !document::is_document(&file.name) {
            self.open_blob(file.sha256.clone(), file.size, file.name.clone(), cx);
            return;
        }
        let task = file
            .task_id
            .as_deref()
            .and_then(|id| self.state.task(id))
            .map(|t| (t.title.clone(), t.status));
        self.toggle_document(
            OpenDocument::new(
                file.sha256.clone(),
                file.name.clone(),
                file.size,
                file.size_label.clone(),
                Some(file.date_label.clone()),
                task,
            ),
            cx,
        );
    }

    fn toggle_document(&mut self, open: OpenDocument, cx: &mut Context<Self>) {
        if self.document_is_open(&open.sha256) {
            self.close_panel(cx);
            return;
        }
        self.panel = Panel::Document;
        let load = open.state == DocumentState::Loading;
        self.document = Some(open);
        self.sync_lists();
        if load {
            self.load_document(cx);
        }
        cx.notify();
    }

    /// Fetches the blob through the client, then reads the cached file off
    /// the UI thread. An answer for a document that is not open any more is
    /// dropped.
    pub fn load_document(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.document.as_mut() else {
            return;
        };
        open.state = DocumentState::Loading;
        let (sha, size) = (open.sha256.clone(), open.size);
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let fetched = client.fetch_blob(sha.clone(), size, None).await;
            let state = match fetched {
                Ok(path) => {
                    let read = cx
                        .background_executor()
                        .spawn(async move { document::read(std::path::Path::new(&path)) })
                        .await;
                    match read {
                        Ok(text) => DocumentState::Ready(text),
                        Err(message) if message.contains("up to") => DocumentState::TooLarge,
                        Err(message) => DocumentState::Failed(message),
                    }
                }
                Err(err) => {
                    DocumentState::Failed(format!("Consort could not get this document. {err}"))
                }
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(open) = this.document.as_mut().filter(|d| d.sha256 == sha) {
                    open.state = state;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The panel of the open document.
    pub fn render_document_panel(
        &mut self,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let Some(open) = self.document.clone() else {
            return div().into_any_element();
        };
        let (save, open_app) = (open.clone(), open.clone());
        let copy_text = match &open.state {
            DocumentState::Ready(text) => Some(text.clone()),
            _ => None,
        };
        let tool = |id: &'static str, name: &'static str, enabled: bool, cx: &gpui_kit::App| {
            icon_button(
                id,
                name,
                t.space.xxl,
                t.icon.size,
                c.text_secondary,
                c.hover_sidebar,
                t,
                cx,
            )
            .debug_selector(move || id.to_owned())
            .when(!enabled, |d| d.opacity(t.opacity.nav_disabled))
        };
        let header = panel_header(t)
            .child(
                div().flex_1().min_w_0().child(
                    line(open.name.clone(), t.text.mono, c.text_primary)
                        .font_family(t.font_mono.clone())
                        .debug_selector(|| "document-name".into()),
                ),
            )
            .child(
                tool("document-copy", "copy", copy_text.is_some(), cx).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if let Some(text) = &copy_text {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                            this.show_notice("Copied the document text.".into(), cx);
                        }
                    },
                )),
            )
            .child(
                tool("document-save", "download", true, cx).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.save_blob(save.sha256.clone(), save.size, save.name.clone(), cx)
                    },
                )),
            )
            .child(
                tool("document-open-app", "external-link", true, cx).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.open_blob(
                            open_app.sha256.clone(),
                            open_app.size,
                            open_app.name.clone(),
                            cx,
                        )
                    },
                )),
            )
            .child(
                tool("document-close", "x", true, cx)
                    .on_click(cx.listener(|this, _, _, cx| this.close_panel(cx))),
            );
        let summary = div()
            .debug_selector(|| "document-summary".into())
            .flex()
            .flex_none()
            .items_center()
            .gap(t.space.xs + t.space.xxs)
            .px(t.space.lg)
            .py(t.space.sm)
            .border_b(t.border_width)
            .border_color(c.border_subtle)
            .when_some(open.task.clone(), |d, (name, status)| {
                d.child(status_glyph(
                    "document-task-status",
                    status,
                    t.icon.dense,
                    t,
                    cx,
                ))
                .child(
                    line(format!("{name}."), detail(t), c.text_secondary)
                        .debug_selector(|| "document-task".into()),
                )
            })
            .child(text(open.summary(), detail(t), c.text_tertiary).flex_none());
        let body = match &open.state {
            DocumentState::Loading => message_body(
                "document-loading",
                "Loading the document.",
                None,
                Vec::new(),
                t,
            ),
            DocumentState::Failed(message) => {
                let retry = button("document-retry", "Try again", ButtonKind::Secondary, t)
                    .debug_selector(|| "document-retry".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.load_document(cx);
                        cx.notify();
                    }));
                message_body(
                    "document-error",
                    "Consort could not show this document.",
                    Some(message.clone()),
                    vec![retry, self.open_in_app_button(&open, t, cx)],
                    t,
                )
            }
            DocumentState::TooLarge => message_body(
                "document-too-large",
                "This document is too large to show here.",
                Some(document::too_large_message(&open.size_label)),
                vec![self.open_in_app_button(&open, t, cx)],
                t,
            ),
            DocumentState::Ready(content) => {
                let key = format!("document-{}", open.sha256);
                let page: AnyElement = if document::is_markdown(&open.name) {
                    self.render_markdown(&key, content, Vec::new(), c.surface_card, t, cx)
                        .debug_selector(|| "document-text".into())
                        .into_any_element()
                } else {
                    // Plain text keeps its line breaks and its spaces.
                    selectable(
                        SharedString::from(key),
                        content.clone(),
                        t.text.mono,
                        c.text_primary,
                    )
                    .font_family(t.font_mono.clone())
                    .debug_selector(|| "document-text".into())
                    .into_any_element()
                };
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(panel_scroll(
                        "document-scroll",
                        div().px(t.space.lg).py(t.space.lg).child(page),
                        t,
                    ))
                    .into_any_element()
            }
        };
        div()
            .id("document-panel")
            .debug_selector(|| "document-panel".into())
            .flex()
            .flex_col()
            .flex_none()
            .w(self.right_panel_width(t))
            .h_full()
            .bg(c.surface_canvas)
            .border_l(t.border_width)
            .border_color(c.border_subtle)
            .child(header)
            .child(summary)
            .child(body)
            .into_any_element()
    }

    fn open_in_app_button(
        &mut self,
        open: &OpenDocument,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let open = open.clone();
        crate::ui::parts::icon_text_button(
            "document-open-app-text",
            "external-link",
            "Open in another app",
            t,
        )
        .debug_selector(|| "document-open-app-text".into())
        .on_click(cx.listener(move |this, _, _, cx| {
            this.open_blob(open.sha256.clone(), open.size, open.name.clone(), cx)
        }))
    }
}

/// The body for a state with no text: a title, the reason and the actions.
fn message_body(
    id: &'static str,
    title: &'static str,
    reason: Option<String>,
    actions: Vec<gpui_kit::Stateful<gpui_kit::Div>>,
    t: &Tokens,
) -> AnyElement {
    let c = &t.colors;
    div()
        .debug_selector(move || id.to_owned())
        .flex()
        .flex_col()
        .flex_1()
        .items_start()
        .gap(t.space.sm)
        .p(t.space.lg)
        .child(text(title, t.text.ui_strong, c.text_primary))
        .when_some(reason, |d, reason| {
            d.child(selectable(
                SharedString::from(format!("{id}-reason")),
                reason,
                t.text.ui,
                c.text_secondary,
            ))
        })
        .when(!actions.is_empty(), |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .pt(t.space.xs)
                    .children(actions),
            )
        })
        .into_any_element()
}
