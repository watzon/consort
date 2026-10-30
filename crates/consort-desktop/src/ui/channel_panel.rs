//! The Channel panel: the channel's name and purpose, and the tabs Files,
//! Routines and Memory. It shares the right side with the Tasks panel, one
//! at a time. Only Files is built. Routines and Memory come with phase 2,
//! so their tabs are disabled and say so on hover.
//!
//! The Files tab lists folders, then files, then the artifacts of each task.
//! It uploads with a button or a drop on the panel, and each file row has
//! the menu Open, Download, Rename, Move and Delete.

use consort_client::{ChannelFileView, UploadStatus, UploadTarget, files::FilesListing};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Div, ExternalPaths, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, SharedString, StatefulInteractiveElement as _,
    Styled, Window, div, linear_color_stop, linear_gradient, prelude::FluentBuilder as _,
};

use crate::{
    app::{ChannelTab, ConsortApp, Dialog, DropOver, Field},
    ui::{
        attachments::file_icon,
        dialogs::{field_block, input_box},
        menu::{OpenMenu, file_menu},
        parts::{
            ButtonKind, button, detail, focusable, icon, icon_button, icon_text_button, line,
            status_color, status_glyph, text,
        },
        window_controls::DragArea as _,
    },
};

/// What a disabled tab says on hover.
pub const PHASE_2: &str = "Comes with phase 2";

impl ConsortApp {
    /// Loads the files of the open channel when they are not loaded.
    pub fn load_channel_files(&mut self, cx: &mut Context<Self>) {
        if self.state.files_loaded.contains(&self.channel_id) || self.channel_id.is_empty() {
            return;
        }
        let client = self.client.clone();
        let channel_id = self.channel_id.clone();
        self.spawn_command(cx, async move {
            client
                .run(consort_client::Command::LoadChannelFiles { channel_id })
                .await
        });
    }

    /// The listing of the open folder of the open channel.
    pub fn files_listing(&self) -> FilesListing {
        let tasks: Vec<_> = self
            .state
            .tasks
            .iter()
            .map(|t| (t.id.clone(), t.title.clone(), t.status))
            .collect();
        consort_client::files::listing(
            &self.state.channel_files,
            &self.channel_id,
            &self.files_folder,
            &tasks,
            consort_client::format::now_ms(),
        )
    }

    pub fn render_channel_panel(
        &mut self,
        t: &Tokens,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        self.load_channel_files(cx);
        let channel = self.state.channel(&self.channel_id).cloned();
        let name = channel
            .as_ref()
            .map(|ch| ch.name.clone())
            .unwrap_or_default();
        let purpose = channel
            .as_ref()
            .map(|ch| ch.purpose.clone())
            .unwrap_or_default();
        let listing = self.files_listing();
        let file_count = self
            .state
            .channel_files
            .iter()
            .filter(|f| f.channel_id == self.channel_id)
            .count();
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(t.space.xs)
            .h(t.component.sidebar.title_bar)
            .pl(t.space.lg)
            .pr(t.space.sm)
            .drag_area()
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_baseline()
                    .gap(t.space.sm)
                    .child(
                        div()
                            .flex_none()
                            .type_style(t.text.title)
                            .line_height(t.text.body.line)
                            .text_color(c.text_primary)
                            .child(name.clone()),
                    )
                    .child(line(purpose, detail(t), c.text_tertiary)),
            )
            .child(
                icon_button(
                    "channel-panel-close",
                    "x",
                    t.space.xxl,
                    t.icon.size,
                    c.text_secondary,
                    c.hover_sidebar,
                    t,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.close_panel(cx))),
            );
        let tabs = [
            (ChannelTab::Files, "Files", Some(file_count), true),
            (ChannelTab::Routines, "Routines", None, false),
            (ChannelTab::Memory, "Memory", None, false),
        ];
        let mut segmented = div()
            .flex()
            .flex_1()
            .gap(t.space.xxs)
            .p(t.space.xxs)
            .bg(c.surface_card)
            .rounded(t.radius.md);
        for (tab, label, count, built) in tabs {
            let active = built && self.channel_tab == tab;
            let id = SharedString::from(format!("channel-tab-{label}"));
            let tab_el = div()
                .id(id.clone())
                .debug_selector(move || id.to_string())
                .flex()
                .flex_1()
                .justify_center()
                .gap(t.space.xs + t.space.xxs)
                .py(t.space.xs)
                .rounded(t.radius.sm)
                .when(active, |d| d.bg(c.surface_raised))
                .child(text(
                    label,
                    if active { t.text.ui_strong } else { t.text.ui },
                    if active {
                        c.text_primary
                    } else if built {
                        c.text_secondary
                    } else {
                        c.text_tertiary
                    },
                ))
                .when_some(count.filter(|n| *n > 0), |d, n| {
                    d.child(text(n.to_string(), t.text.ui, c.text_tertiary))
                });
            segmented = segmented.child(if built {
                focusable(tab_el, t)
                    .cursor_pointer()
                    .when(!active, |d| d.hover(|s| s.bg(c.hover_card)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.channel_tab = tab;
                        cx.notify();
                    }))
            } else {
                // The tab waits for phase 2. It takes no click.
                tab_el.tooltip(|_, cx| {
                    cx.new(|_| crate::ui::channel_panel::Tooltip(PHASE_2.into()))
                        .into()
                })
            });
        }
        let dropping = self
            .drop_hover
            .filter(|(over, _)| *over == DropOver::Files)
            .map(|(_, n)| n);
        let body = self.render_files_tab(&listing, &name, dropping, t, cx);
        div()
            .id("channel-panel")
            .debug_selector(|| "channel-panel".into())
            .flex()
            .flex_col()
            .flex_none()
            .w(t.layout.panel_width)
            .h_full()
            .bg(c.surface_sidebar)
            .border_l(t.border_width)
            .border_color(c.border_subtle)
            .on_drag_move::<ExternalPaths>(cx.listener(
                |this, event: &gpui_kit::DragMoveEvent<ExternalPaths>, _, cx| {
                    let inside = event.bounds.contains(&event.event.position);
                    let count = event.drag(cx).paths().len();
                    this.drag_files_over(DropOver::Files, inside, count, cx);
                },
            ))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                cx.stop_propagation();
                this.drop_files(DropOver::Files, paths, cx);
            }))
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .py(t.space.xs)
                    .px(t.space.lg)
                    .child(segmented),
            )
            .child(body)
            .child(
                div()
                    .flex_none()
                    .pt(t.space.md)
                    .pb(t.space.md + t.space.xxs)
                    .px(t.space.lg)
                    .border_t(t.border_width)
                    .border_color(c.border_subtle)
                    .child(text(
                        format!(
                            "Drop files on this panel to upload them to {name}. Tasks in this channel can read every file."
                        ),
                        detail(t),
                        c.text_tertiary,
                    )),
            )
            .into_any_element()
    }

    fn render_files_tab(
        &mut self,
        listing: &FilesListing,
        channel_name: &str,
        dropping: Option<usize>,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let target = UploadTarget::ChannelFiles {
            channel_id: self.channel_id.clone(),
            folder: self.files_folder.clone(),
        };
        let uploads = self.uploads_of(&target);
        let upload_target = target.clone();
        let upload_button = icon_text_button("files-upload", "upload", "Upload", t)
            .debug_selector(|| "files-upload".into())
            .px(t.space.sm + t.space.xxs)
            .on_click(
                cx.listener(move |this, _, _, cx| this.choose_files(upload_target.clone(), cx)),
            );
        let loaded = self.state.files_loaded.contains(&self.channel_id);
        if loaded && listing.empty && uploads.is_empty() {
            return self.render_files_empty(channel_name, dropping, target, t, cx);
        }
        let mut toolbar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(t.space.xs + t.space.xxs)
            .pt(t.space.sm + t.space.xxs)
            .pr(t.space.md)
            .pb(t.space.xs + t.space.xxs)
            .pl(t.space.lg);
        if listing.folder.is_empty() {
            toolbar = toolbar.child(
                div()
                    .debug_selector(|| "files-summary".into())
                    .flex_1()
                    .child(text(listing.summary.clone(), t.text.ui, c.text_secondary)),
            );
        } else {
            let parent = consort_client::files::split_path(&listing.folder)
                .0
                .to_owned();
            toolbar = toolbar
                .child(
                    icon_button(
                        "files-back",
                        "chevron-left",
                        t.component.composer_chip,
                        t.icon.size,
                        c.text_secondary,
                        c.hover_sidebar,
                        t,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.files_folder = parent.clone();
                        cx.notify();
                    })),
                )
                .child(
                    div()
                        .debug_selector(|| "files-folder".into())
                        .flex_1()
                        .min_w_0()
                        .child(line(
                            listing.folder.clone(),
                            t.text.ui_strong,
                            c.text_primary,
                        )),
                );
        }
        toolbar = toolbar.child(upload_button);

        let f = t.component.files;
        let mut list = div()
            .flex()
            .flex_col()
            .gap(t.border_width)
            .px(t.space.sm)
            .pb(t.space.sm);
        for folder in &listing.folders {
            let path = folder.path.clone();
            let selector = format!("folder-{}", folder.path);
            list = list.child(
                file_row(SharedString::from(format!("folder-{}", folder.path)), t)
                    .debug_selector(move || selector)
                    .cursor_pointer()
                    .hover(|s| s.bg(c.hover_sidebar))
                    .child(icon("folder", t.icon.size, c.text_secondary))
                    .child(line(folder.name.clone(), t.text.ui, c.text_primary).flex_1())
                    .child(column(folder.count_label.clone(), f.size_col, t))
                    .child(column(folder.date_label.clone(), f.date_col, t))
                    .child(div().size(f.more).flex_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.files_folder = path.clone();
                        cx.notify();
                    })),
            );
        }
        for file in &listing.files {
            list = list.child(self.render_file_row(file, false, t, cx));
        }
        for upload in &uploads {
            let fraction = match &upload.status {
                UploadStatus::Uploading { fraction, .. } => *fraction,
                UploadStatus::Ready => 1.0,
                _ => 0.0,
            };
            let state = match &upload.status {
                UploadStatus::Uploading { label, .. } => format!("Uploading, {label}"),
                UploadStatus::Preparing => "Preparing".to_owned(),
                UploadStatus::Ready => "Saving".to_owned(),
                UploadStatus::Failed(message) => message.clone(),
            };
            let failed = matches!(upload.status, UploadStatus::Failed(_));
            let id = upload.id.clone();
            let selector = format!("files-upload-{}", upload.id);
            list = list.child(
                file_row(SharedString::from(format!("files-upload-{}", upload.id)), t)
                    .debug_selector(move || selector)
                    .child(icon(
                        if failed {
                            "failed"
                        } else {
                            file_icon(&upload.mime)
                        },
                        t.icon.size,
                        if failed {
                            c.status_failed
                        } else {
                            c.text_secondary
                        },
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(t.space.xs)
                            .child(line(upload.name.clone(), t.text.ui, c.text_primary))
                            .when(!failed, |d| {
                                d.child(
                                    div()
                                        .w_full()
                                        .h(f.row_progress)
                                        .rounded(t.radius.full)
                                        .bg(c.status_working.opacity(t.opacity.progress_track))
                                        .child(
                                            div()
                                                .h_full()
                                                .w(gpui_kit::relative(fraction))
                                                .rounded(t.radius.full)
                                                .bg(c.status_working),
                                        ),
                                )
                            }),
                    )
                    .child(
                        line(
                            state,
                            detail(t),
                            if failed {
                                c.status_failed
                            } else {
                                c.text_tertiary
                            },
                        )
                        .flex_none()
                        .max_w(f.size_col + f.date_col + t.space.page),
                    )
                    .child(
                        icon_button(
                            SharedString::from(format!("files-upload-cancel-{}", upload.id)),
                            "x",
                            f.more,
                            t.component.files.chip_close_icon,
                            c.text_tertiary,
                            c.hover_sidebar,
                            t,
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let client = this.client.clone();
                            let id = id.clone();
                            this.spawn_command(cx, async move {
                                client
                                    .run(consort_client::Command::RemoveUpload { id })
                                    .await
                            });
                        })),
                    ),
            );
        }
        if !listing.artifacts.is_empty() {
            list = list.child(
                div()
                    .flex()
                    .gap(t.space.xs + t.space.xxs)
                    .pt(t.space.lg)
                    .pb(t.space.xs)
                    .px(t.space.sm)
                    .child(text("Artifacts", t.text.label, c.text_tertiary))
                    .child(text(
                        match listing.artifacts.len() {
                            1 => "from 1 task".to_owned(),
                            n => format!("from {n} tasks"),
                        },
                        detail(t),
                        c.text_tertiary,
                    )),
            );
            for group in &listing.artifacts {
                let task_id = group.task.task_id.clone();
                let selector = format!("artifact-task-{task_id}");
                list = list.child(
                    file_row(SharedString::from(format!("artifact-task-{task_id}")), t)
                        .debug_selector(move || selector)
                        .cursor_pointer()
                        .hover(|s| s.bg(c.hover_sidebar))
                        .child(status_glyph(
                            SharedString::from(format!("artifact-status-{task_id}")),
                            group.task.status,
                            t.icon.size,
                            t,
                            cx,
                        ))
                        .child(
                            line(group.task.task_name.clone(), t.text.ui, c.text_primary).flex_1(),
                        )
                        .child(text(
                            group.task.status.label(),
                            detail(t),
                            status_color(group.task.status, t),
                        ))
                        .child(div().size(f.more).flex_none())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_thread(task_id.clone(), window, cx)
                        })),
                );
                for file in &group.files {
                    list = list.child(self.render_file_row(file, true, t, cx));
                }
            }
        }
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(toolbar)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("files-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .child(list),
                    )
                    // The list scrolls inside the panel with soft edges.
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(t.space.sm)
                            .bg(linear_gradient(
                                180.,
                                linear_color_stop(c.surface_sidebar, 0.),
                                linear_color_stop(c.sidebar_clear, 1.),
                            )),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0()
                            .h(t.layout.fade_bottom)
                            .bg(linear_gradient(
                                0.,
                                linear_color_stop(c.surface_sidebar, 0.),
                                linear_color_stop(c.sidebar_clear, 1.),
                            )),
                    )
                    .when_some(dropping, |d, count| {
                        // The drop state covers the list, so the rows do not
                        // show through its words.
                        d.child(
                            div()
                                .debug_selector(|| "files-drop".into())
                                .absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .bottom_0()
                                .pb(t.space.sm)
                                .px(t.space.sm)
                                .bg(c.surface_sidebar)
                                .child(
                                    div()
                                        .size_full()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .justify_center()
                                        .gap(t.space.sm)
                                        .rounded(t.radius.lg)
                                        .border(t.stroke)
                                        .border_dashed()
                                        .border_color(c.accent_text)
                                        .bg(c.accent_fill.opacity(t.opacity.drop_fill))
                                        .child(
                                            div()
                                                .size(t.component.files.drop_icon_tile)
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(t.radius.full)
                                                .bg(c.tint_accent)
                                                .child(icon(
                                                    "upload",
                                                    t.component.files.drop_icon,
                                                    c.accent_text,
                                                )),
                                        )
                                        .child(text(
                                            match count {
                                                1 => "Drop to upload 1 file".to_owned(),
                                                n => format!("Drop to upload {n} files"),
                                            },
                                            t.text.ui_strong,
                                            c.text_primary,
                                        ))
                                        .child(text(
                                            if self.files_folder.is_empty() {
                                                "Into the top folder".to_owned()
                                            } else {
                                                format!("Into {}", self.files_folder)
                                            },
                                            detail(t),
                                            c.text_tertiary,
                                        )),
                                ),
                        )
                    }),
            )
    }

    /// The empty Files tab: it is its own drop zone.
    fn render_files_empty(
        &mut self,
        channel_name: &str,
        dropping: Option<usize>,
        target: UploadTarget,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        div().flex_1().min_h_0().p(t.space.lg).child(
            div()
                .id("files-empty")
                .debug_selector(|| "files-empty".into())
                .flex()
                .flex_col()
                .items_center()
                .gap(t.space.sm)
                .py(t.space.page + t.space.xl)
                .px(t.space.xl)
                .rounded(t.radius.lg)
                .border(t.border_width)
                .border_dashed()
                .border_color(if dropping.is_some() {
                    c.accent_text
                } else {
                    c.border_strong
                })
                .when(dropping.is_some(), |d| {
                    d.bg(c.accent_fill.opacity(t.opacity.drop_fill))
                })
                .child(
                    div()
                        .size(t.component.files.drop_icon_tile + t.space.sm)
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(t.radius.full)
                        .bg(c.surface_card)
                        .child(icon("folder", t.icon.size, c.text_secondary)),
                )
                .child(text(
                    format!("No files in {channel_name} yet"),
                    t.text.ui_strong,
                    c.text_primary,
                ))
                .child(
                    div().text_center().child(text(
                        "Drop files here, or upload them. Tasks in this channel can read them and add their own.",
                        t.text.ui,
                        c.text_secondary,
                    )),
                )
                .child(
                    icon_text_button("files-upload-empty", "upload", "Upload files", t)
                        .mt(t.space.sm)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.choose_files(target.clone(), cx)),
                        ),
                ),
        )
    }

    fn render_file_row(
        &mut self,
        file: &ChannelFileView,
        artifact: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let f = t.component.files;
        let id = SharedString::from(format!("file-row-{}", file.path));
        let selector = id.to_string();
        let more_id = SharedString::from(format!("file-more-{}", file.path));
        let group = SharedString::from(format!("file-row-group-{}", file.path));
        let open = file.clone();
        let menu_file = file.clone();
        let more_file = file.clone();
        let menu_open = self.menu.as_ref().is_some_and(|m| {
            m.items
                .first()
                .is_some_and(|i| i.action == crate::ui::menu::MenuAction::OpenFile(file.clone()))
        });
        file_row(id, t)
            .debug_selector(move || selector)
            .group(group.clone())
            .when(artifact, |d| d.pl(f.artifact_indent))
            .cursor_pointer()
            .when(menu_open, |d| d.bg(c.surface_selected))
            .when(!menu_open, |d| d.hover(|s| s.bg(c.hover_sidebar)))
            .child(icon(file_icon(&file.mime), t.icon.size, c.text_secondary))
            .child(line(file.name.clone(), t.text.ui, c.text_primary).flex_1())
            .child(column(file.size_label.clone(), f.size_col, t))
            .child(column(file.date_label.clone(), f.date_col, t))
            .child(
                icon_button(
                    more_id,
                    "more-horizontal",
                    f.more,
                    t.icon.size,
                    c.text_secondary,
                    c.hover_selected,
                    t,
                    cx,
                )
                .when(!menu_open, |d| {
                    d.opacity(0.).group_hover(group, |s| s.opacity(1.))
                })
                .when(
                    self.files_menu_for.as_deref() == Some(file.path.as_str()),
                    |d| d.relative().child(self.triggers.track("files-menu-anchor")),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(
                    move |this, event: &gpui_kit::ClickEvent, _, cx| {
                        cx.stop_propagation();
                        let at = event.position();
                        this.open_menu(OpenMenu::at(at, file_menu(&more_file)), cx);
                    },
                )),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if open.image {
                    let caption = format!(
                        "In the channel files since {}.",
                        open.date_label.to_lowercase()
                    );
                    let image = consort_client::AttachmentView {
                        sha256: open.sha256.clone(),
                        name: open.name.clone(),
                        mime: open.mime.clone(),
                        size: open.size,
                        image: true,
                        detail: consort_client::uploads::file_detail(
                            &open.mime, &open.name, open.size,
                        ),
                        size_label: open.size_label.clone(),
                        local_path: open.local_path.clone(),
                        artifact: None,
                    };
                    this.open_lightbox(vec![image], 0, caption, cx);
                } else {
                    this.open_blob(open.sha256.clone(), open.size, open.name.clone(), cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_menu(OpenMenu::at(event.position, file_menu(&menu_file)), cx);
                    cx.stop_propagation();
                }),
            )
            .into_any_element()
    }

    // The file dialogs.

    /// The title, description, width, body and actions of a file dialog.
    pub fn file_dialog(
        &mut self,
        dialog: &Dialog,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(String, String, gpui_kit::Pixels, Div, Div)> {
        let c = &t.colors;
        match dialog {
            Dialog::ConfirmDeleteFile(channel_id, path) => {
                let (channel_id, path) = (channel_id.clone(), path.clone());
                let name = consort_client::files::split_path(&path).1.to_owned();
                Some((
                    format!("Delete {name}?"),
                    "The file leaves the channel files. Messages that have it attached keep their copy. You cannot undo this.".to_owned(),
                    t.layout.dialog_width,
                    div(),
                    self.dialog_actions_kind(
                        "Delete file",
                        ButtonKind::Danger,
                        t,
                        cx,
                        move |this, window, cx| {
                            let client = this.client.clone();
                            let (channel_id, path) = (channel_id.clone(), path.clone());
                            this.submit_dialog(
                                async move {
                                    client
                                        .run(consort_client::Command::DeleteChannelFile {
                                            channel_id,
                                            path,
                                        })
                                        .await
                                },
                                window,
                                cx,
                            );
                        },
                    ),
                ))
            }
            Dialog::RenameFile(channel_id, path) => {
                let (channel_id, path) = (channel_id.clone(), path.clone());
                let focused = {
                    use gpui_kit::Focusable as _;
                    self.inputs
                        .name
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                };
                Some((
                    "Rename file".to_owned(),
                    "The new name shows in the channel files. Tasks read the file under this name."
                        .to_owned(),
                    t.layout.dialog_width,
                    field_block("Name", input_box(&self.inputs.name, focused, t), None, t),
                    self.dialog_actions("Rename", t, cx, move |this, window, cx| {
                        let name = this.field(Field::Name, cx);
                        if name.is_empty() || name.contains('/') {
                            this.dialog_error =
                                Some("Enter a name with no slash, such as plan.pdf.".into());
                            cx.notify();
                            return;
                        }
                        let folder = consort_client::files::split_path(&path).0.to_owned();
                        let to_path = consort_client::files::join_path(&folder, &name);
                        let client = this.client.clone();
                        let (channel_id, from_path) = (channel_id.clone(), path.clone());
                        this.submit_dialog(
                            async move {
                                client
                                    .run(consort_client::Command::MoveChannelFile {
                                        channel_id,
                                        from_path,
                                        to_path,
                                    })
                                    .await
                            },
                            window,
                            cx,
                        );
                    }),
                ))
            }
            Dialog::MoveFile(channel_id, path) => {
                let (channel_id, path) = (channel_id.clone(), path.clone());
                let name = consort_client::files::split_path(&path).1.to_owned();
                let folders =
                    consort_client::files::folders_of(&self.state.channel_files, &channel_id);
                let mut list = div()
                    .id("move-folders")
                    .flex()
                    .flex_col()
                    .gap(t.space.xxs)
                    .max_h(t.component.files_view)
                    .overflow_y_scroll();
                for folder in folders {
                    let chosen = self.move_to == folder;
                    let pick = folder.clone();
                    let label = if folder.is_empty() {
                        "Top folder".to_owned()
                    } else {
                        folder.clone()
                    };
                    let selector = format!("move-to-{label}");
                    list = list.child(
                        focusable(div().id(SharedString::from(format!("move-to-{label}"))), t)
                            .debug_selector(move || selector)
                            .flex()
                            .items_center()
                            .gap(t.space.sm)
                            .py(t.space.xs + t.space.xxs)
                            .px(t.space.sm)
                            .rounded(t.radius.md)
                            .cursor_pointer()
                            .when(chosen, |d| d.bg(c.surface_selected))
                            .when(!chosen, |d| d.hover(|s| s.bg(c.hover_raised)))
                            .child(icon("folder", t.icon.size, c.text_secondary))
                            .child(line(label, t.text.ui, c.text_primary).flex_1())
                            .when(chosen, |d| {
                                d.child(icon("check", t.icon.dense, c.accent_text))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.move_to = pick.clone();
                                cx.notify();
                            })),
                    );
                }
                Some((
                    format!("Move {name}"),
                    "Choose the folder for the file.".to_owned(),
                    t.layout.dialog_width,
                    scroll_with_fades(list, t),
                    self.dialog_actions("Move", t, cx, move |this, window, cx| {
                        let to_path = consort_client::files::join_path(&this.move_to, &name);
                        let client = this.client.clone();
                        let (channel_id, from_path) = (channel_id.clone(), path.clone());
                        this.submit_dialog(
                            async move {
                                client
                                    .run(consort_client::Command::MoveChannelFile {
                                        channel_id,
                                        from_path,
                                        to_path,
                                    })
                                    .await
                            },
                            window,
                            cx,
                        );
                    }),
                ))
            }
            Dialog::ChooseChannelFiles(target) => {
                let channel_id = match target {
                    UploadTarget::Composer { channel_id }
                    | UploadTarget::ChannelFiles { channel_id, .. } => channel_id.clone(),
                    UploadTarget::Reply { task_id } => self
                        .state
                        .task(task_id)
                        .map(|t| t.channel_id.clone())
                        .unwrap_or_default(),
                };
                self.load_channel_files_of(&channel_id, cx);
                let mut files: Vec<ChannelFileView> = self
                    .state
                    .channel_files
                    .iter()
                    .filter(|f| f.channel_id == channel_id)
                    .cloned()
                    .collect();
                files.sort_by_key(|f| f.path.to_lowercase());
                let mut list = div()
                    .id("choose-files")
                    .flex()
                    .flex_col()
                    .gap(t.space.xxs)
                    .max_h(t.component.files_view)
                    .overflow_y_scroll();
                if files.is_empty() {
                    list = list.child(text(
                        "This channel has no files yet. Upload one from this computer.",
                        t.text.ui,
                        c.text_secondary,
                    ));
                }
                for file in files {
                    let selector = format!("choose-{}", file.path);
                    let target = target.clone();
                    let pick = file.clone();
                    list = list.child(
                        focusable(
                            div().id(SharedString::from(format!("choose-{}", file.path))),
                            t,
                        )
                        .debug_selector(move || selector)
                        .flex()
                        .items_center()
                        .gap(t.space.sm)
                        .py(t.space.xs + t.space.xxs)
                        .px(t.space.sm)
                        .rounded(t.radius.md)
                        .cursor_pointer()
                        .hover(|s| s.bg(c.hover_raised))
                        .child(icon(file_icon(&file.mime), t.icon.size, c.text_secondary))
                        .child(line(file.path.clone(), t.text.ui, c.text_primary).flex_1())
                        .child(text(file.size_label.clone(), detail(t), c.text_tertiary))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if let Some(blob) = pick.blob() {
                                    this.attach(
                                        target.clone(),
                                        vec![consort_client::AttachInput::Blob(blob)],
                                        cx,
                                    );
                                }
                                this.close_dialog(window, cx);
                            },
                        )),
                    );
                }
                Some((
                    "Choose from channel files".to_owned(),
                    "The file goes with your message. It stays in the channel files too."
                        .to_owned(),
                    t.layout.dialog_width,
                    scroll_with_fades(list, t),
                    div()
                        .flex()
                        .justify_end()
                        .pt(t.space.lg)
                        .border_t(t.border_width)
                        .border_color(c.border_subtle)
                        .child(
                            button("dialog-cancel", "Cancel", ButtonKind::Quiet, t).on_click(
                                cx.listener(|this, _, window, cx| this.close_dialog(window, cx)),
                            ),
                        ),
                ))
            }
            _ => None,
        }
    }

    fn load_channel_files_of(&mut self, channel_id: &str, cx: &mut Context<Self>) {
        if channel_id.is_empty() || self.state.files_loaded.iter().any(|c| c == channel_id) {
            return;
        }
        let client = self.client.clone();
        let channel_id = channel_id.to_owned();
        self.spawn_command(cx, async move {
            client
                .run(consort_client::Command::LoadChannelFiles { channel_id })
                .await
        });
    }
}

/// A row of the Files tab: 34 px, `md` corners.
fn file_row(id: SharedString, t: &Tokens) -> gpui_kit::Stateful<Div> {
    let f = t.component.files;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(t.component.sidebar.row_gap)
        .py(f.row_pad_y)
        .px(t.space.sm)
        .rounded(t.radius.md)
}

/// A list that scrolls inside a dialog, with soft edges at the top and the
/// bottom, so its rows never stop at a hard line.
fn scroll_with_fades(list: gpui_kit::Stateful<Div>, t: &Tokens) -> Div {
    let c = &t.colors;
    let fade = |top: bool| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .h(t.space.md)
            .when(top, |d| d.top_0())
            .when(!top, |d| d.bottom_0())
            .bg(linear_gradient(
                if top { 180. } else { 0. },
                linear_color_stop(c.surface_raised, 0.),
                linear_color_stop(c.raised_clear, 1.),
            ))
    };
    div()
        .relative()
        .child(list.py(t.space.xs))
        .child(fade(true))
        .child(fade(false))
}

/// A right-aligned column of a row: the size or the date.
fn column(value: String, width: gpui_kit::Pixels, t: &Tokens) -> Div {
    text(value, detail(t), t.colors.text_tertiary)
        .w(width)
        .flex_none()
        .text_right()
}

/// A plain tooltip: one line of text on `surface.raised`.
pub struct Tooltip(pub SharedString);

impl gpui_kit::Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = ConsortApp::tokens(cx);
        div()
            .py(t.space.xs)
            .px(t.space.sm)
            .rounded(t.radius.md)
            .bg(t.colors.surface_raised)
            .border(t.border_width)
            .border_color(t.colors.border_subtle)
            .shadow(t.shadow_float())
            .font_family(t.font_sans.clone())
            .type_style(t.text.label)
            .text_color(t.colors.text_secondary)
            .child(self.0.clone())
    }
}
