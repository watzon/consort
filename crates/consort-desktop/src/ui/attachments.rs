//! Attachments: the chips of the composer and the reply box, the drop state,
//! the add menu, paste, and the pictures and files in bubbles and thread
//! posts. The light box is in `ui/lightbox.rs`. The sizes come from
//! `component.files`.

use std::path::PathBuf;

use consort_client::{
    AttachInput, AttachmentView, PastedBytes, UploadStatus, UploadTarget, UploadView,
    chat::{AttachmentPart, AttachmentRow, Lane},
};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, ClipboardEntry, Context, Div, ExternalPaths, ImageFormat, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ObjectFit, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, StyledImage as _, div, img,
    prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, DropOver},
    ui::{
        menu::{OpenMenu, add_menu, attachment_menu, upload_menu},
        parts::{ButtonKind, button, detail, focusable, icon, icon_button, line, text},
    },
};

/// The icon of a file: a picture or a page.
pub fn file_icon(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        "image"
    } else {
        "file"
    }
}

/// The media type and the file extension of a pasted picture.
pub fn pasted_type(format: ImageFormat) -> (&'static str, &'static str) {
    match format {
        ImageFormat::Png => ("image/png", "png"),
        ImageFormat::Jpeg => ("image/jpeg", "jpg"),
        ImageFormat::Webp => ("image/webp", "webp"),
        ImageFormat::Gif => ("image/gif", "gif"),
        ImageFormat::Svg => ("image/svg+xml", "svg"),
        ImageFormat::Bmp => ("image/bmp", "bmp"),
        ImageFormat::Tiff => ("image/tiff", "tiff"),
        _ => ("application/octet-stream", "bin"),
    }
}

/// The folder that a save dialog opens in: Downloads, else the temp folder.
pub fn save_folder() -> PathBuf {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Downloads"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

/// The inputs of a paste: the pictures and the copied files on the
/// clipboard. Empty when the clipboard holds only text, so the text box
/// takes the paste as usual.
pub fn paste_inputs(item: &gpui_kit::ClipboardItem, stamp: &str) -> Vec<AttachInput> {
    let mut out = Vec::new();
    for entry in item.entries() {
        match entry {
            ClipboardEntry::Image(image) if !image.bytes.is_empty() => {
                let (mime, ext) = pasted_type(image.format);
                out.push(AttachInput::Pasted {
                    name: format!("pasted-image-{stamp}.{ext}"),
                    mime: mime.to_owned(),
                    bytes: PastedBytes(std::sync::Arc::new(image.bytes.clone())),
                });
            }
            ClipboardEntry::ExternalPaths(paths) => {
                out.extend(
                    paths
                        .paths()
                        .iter()
                        .filter(|p| p.is_file())
                        .map(|p| AttachInput::Path(p.display().to_string())),
                );
            }
            _ => {}
        }
    }
    out
}

/// "Drop to attach 3 files".
pub fn drop_label(count: usize) -> String {
    match count {
        1 => "Drop to attach 1 file".to_owned(),
        n => format!("Drop to attach {n} files"),
    }
}

impl ConsortApp {
    /// The uploads of a composer, a reply box or a folder.
    pub fn uploads_of(&self, target: &UploadTarget) -> Vec<UploadView> {
        self.state
            .uploads
            .iter()
            .filter(|u| u.target.as_ref() == Some(target))
            .cloned()
            .collect()
    }

    /// The IDs of the uploads of a target, for a send.
    pub fn upload_ids(&self, target: &UploadTarget) -> Vec<String> {
        self.uploads_of(target).into_iter().map(|u| u.id).collect()
    }

    /// Attaches files to a target. The uploads start at once.
    pub fn attach(
        &mut self,
        target: UploadTarget,
        inputs: Vec<AttachInput>,
        cx: &mut Context<Self>,
    ) {
        if inputs.is_empty() {
            return;
        }
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client.attach(target, inputs).await.map(|_| ())
        });
    }

    pub fn attach_paths(
        &mut self,
        target: UploadTarget,
        paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        let inputs = paths
            .iter()
            .filter(|p| p.is_file())
            .map(|p| AttachInput::Path(p.display().to_string()))
            .collect();
        self.attach(target, inputs, cx);
    }

    /// The target of a drop where the files would go now.
    pub fn drop_target(&self, over: DropOver) -> Option<UploadTarget> {
        match over {
            DropOver::Chat => (self.waiting().is_none()).then(|| UploadTarget::Composer {
                channel_id: self.channel_id.clone(),
            }),
            DropOver::Files => Some(UploadTarget::ChannelFiles {
                channel_id: self.channel_id.clone(),
                folder: self.files_folder.clone(),
            }),
            DropOver::Reply => match &self.panel {
                crate::app::Panel::Thread(task_id) => Some(UploadTarget::Reply {
                    task_id: task_id.clone(),
                }),
                crate::app::Panel::MessageThread(root_id) => Some(UploadTarget::Thread {
                    root_id: root_id.clone(),
                }),
                _ => None,
            },
        }
    }

    /// Files moved over a drop area: remember where they would go, so the
    /// area can show its drop state.
    pub fn drag_files_over(
        &mut self,
        over: DropOver,
        inside: bool,
        count: usize,
        cx: &mut Context<Self>,
    ) {
        let next = if inside {
            Some((over, count))
        } else if self.drop_hover.is_some_and(|(o, _)| o == over) {
            None
        } else {
            self.drop_hover
        };
        if next != self.drop_hover {
            self.drop_hover = next;
            cx.notify();
        }
    }

    /// Files dropped on a drop area.
    pub fn drop_files(&mut self, over: DropOver, paths: &ExternalPaths, cx: &mut Context<Self>) {
        self.drop_hover = None;
        if let Some(target) = self.drop_target(over) {
            self.attach_paths(target, paths.paths(), cx);
        }
        cx.notify();
    }

    /// Upload from this computer: the system file dialog.
    pub fn choose_files(&mut self, target: UploadTarget, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| this.attach_paths(target, &paths, cx));
        })
        .detach();
    }

    /// A paste in a message box: pictures and copied files attach. Returns
    /// whether it took the paste. Text pastes as usual.
    pub fn paste_files(&mut self, target: UploadTarget, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let stamp = consort_client::format::time_label(consort_client::format::now_ms())
            .replace([':', ' '], "");
        let inputs = paste_inputs(&item, &stamp.to_lowercase());
        if inputs.is_empty() {
            return false;
        }
        self.attach(target, inputs, cx);
        true
    }

    /// Opens the add menu under its button.
    pub fn open_add_menu(
        &mut self,
        trigger: &'static str,
        target: UploadTarget,
        cx: &mut Context<Self>,
    ) {
        if let Some(bounds) = self.triggers.get(trigger) {
            self.open_menu(OpenMenu::under(bounds, add_menu(&target), false), cx);
        }
    }

    /// Asks the client to download a blob, once, so its thumbnail can show.
    pub fn want_blob(&mut self, sha256: &str, size: u64, cx: &mut Context<Self>) {
        if sha256.is_empty() || !self.fetching.insert(sha256.to_owned()) {
            return;
        }
        let client = self.client.clone();
        let sha = sha256.to_owned();
        cx.spawn(async move |_, _| {
            let _ = client.fetch_blob(sha, size, None).await;
        })
        .detach();
    }

    /// Open: downloads the file when needed and opens it with the app of
    /// the system.
    pub fn open_blob(&mut self, sha256: String, size: u64, name: String, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = client.fetch_blob(sha256, size, Some(name)).await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(path) => cx.open_with_system(std::path::Path::new(&path)),
                Err(err) => this.show_error(err.to_string(), cx),
            });
        })
        .detach();
    }

    /// Save: the system save dialog, then a copy of the file there.
    pub fn save_blob(&mut self, sha256: String, size: u64, name: String, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_new_path(&save_folder(), Some(&name));
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(to))) = chosen.await else {
                return;
            };
            let result = client
                .run(consort_client::Command::SaveBlob {
                    sha256,
                    size,
                    to: to.display().to_string(),
                })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.show_notice(format!("Saved {name}."), cx),
                Err(err) => this.show_error(err.to_string(), cx),
            });
        })
        .detach();
    }

    /// Save to channel files: the attachment goes into the top folder of the
    /// channel files with its name.
    pub fn save_to_channel_files(
        &mut self,
        channel_id: String,
        attachment: AttachmentView,
        cx: &mut Context<Self>,
    ) {
        let Some(blob) = attachment.blob() else {
            return;
        };
        let client = self.client.clone();
        let name = attachment.name.clone();
        cx.spawn(async move |this, cx| {
            let result = client
                .run(consort_client::Command::SaveToChannelFiles {
                    channel_id,
                    path: name.clone(),
                    blob,
                })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.show_notice(format!("Saved {name} to the channel files."), cx),
                Err(err) => this.show_error(err.to_string(), cx),
            });
        })
        .detach();
    }

    // Chips.

    /// The chips of the uploads of a composer or a reply box, in one row
    /// that wraps. `None` when there are none.
    pub fn render_upload_chips(
        &mut self,
        target: &UploadTarget,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::Stateful<Div>> {
        let uploads = self.uploads_of(target);
        if uploads.is_empty() {
            return None;
        }
        let mut row = div()
            .id("upload-chips")
            .debug_selector(|| "upload-chips".into())
            .flex()
            .flex_wrap()
            .items_center()
            .gap(t.space.sm)
            .pt(t.space.xs)
            .px(t.space.xs)
            .pb(t.space.xs + t.space.xxs);
        for upload in uploads {
            if upload.image
                && upload.local_path.is_none()
                && let Some(sha) = upload.sha256.clone()
            {
                self.want_blob(&sha, upload.size, cx);
            }
            row = row.child(self.render_chip(&upload, t, cx));
        }
        Some(row)
    }

    fn render_chip(
        &mut self,
        upload: &UploadView,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let f = t.component.files;
        let id = upload.id.clone();
        let menu_upload = upload.clone();
        let remove_id = upload.id.clone();
        let selector = format!("chip-{}", upload.id);
        let remove = move |this: &mut Self, cx: &mut Context<Self>| {
            let client = this.client.clone();
            let id = remove_id.clone();
            this.spawn_command(cx, async move {
                client
                    .run(consort_client::Command::RemoveUpload { id })
                    .await
            });
        };
        let context_menu = cx.listener(move |this, event: &MouseDownEvent, _, cx| {
            this.open_menu(OpenMenu::at(event.position, upload_menu(&menu_upload)), cx);
            cx.stop_propagation();
        });
        let failed = matches!(upload.status, UploadStatus::Failed(_));
        // A picture that is ready or on its way shows as a thumbnail.
        if upload.image
            && !failed
            && let Some(path) = upload.local_path.clone()
        {
            let remove = remove.clone();
            let group: SharedString = format!("chip-group-{id}").into();
            return div()
                .id(SharedString::from(format!("chip-{id}")))
                .debug_selector(move || selector)
                .group(group.clone())
                .relative()
                .flex_none()
                .size(f.chip_image)
                .rounded(t.radius.lg)
                .border(t.border_width)
                .border_color(c.border_strong)
                .overflow_hidden()
                .child(
                    img(PathBuf::from(path))
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
                .when(!matches!(upload.status, UploadStatus::Ready), |d| {
                    d.child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0()
                            .child(progress_bar(upload, f.progress, t)),
                    )
                })
                .child(
                    div()
                        .id(SharedString::from(format!("chip-remove-{id}")))
                        .absolute()
                        .top(t.space.xxs + t.border_width)
                        .right(t.space.xxs + t.border_width)
                        .size(f.chip_badge)
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(t.radius.full)
                        .bg(c.scrim)
                        .cursor_pointer()
                        .opacity(0.)
                        .group_hover(group, |s| s.opacity(1.))
                        .child(icon("x", f.chip_close_icon, c.text_primary))
                        .on_click(cx.listener(move |this, _, _, cx| remove(this, cx))),
                )
                .on_mouse_down(MouseButton::Right, context_menu)
                .into_any_element();
        }
        let tile_bg = if failed {
            c.tint_failed
        } else {
            c.surface_selected
        };
        let tile_icon = if failed {
            "failed"
        } else {
            file_icon(&upload.mime)
        };
        let tile_color = if failed {
            c.status_failed
        } else {
            c.text_secondary
        };
        let status_line: AnyElement = match &upload.status {
            UploadStatus::Uploading { label, .. } => div()
                .flex()
                .items_center()
                .gap(t.space.sm)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(progress_bar(upload, f.progress, t)),
                )
                .child(text(label.clone(), t.text.caption, c.text_tertiary).flex_none())
                .into_any_element(),
            UploadStatus::Preparing => {
                text("Preparing", detail(t), c.text_tertiary).into_any_element()
            }
            UploadStatus::Ready => {
                text(upload.detail.clone(), detail(t), c.text_tertiary).into_any_element()
            }
            // An error detail: the user can select and copy it.
            UploadStatus::Failed(message) => crate::ui::parts::selectable(
                SharedString::from(format!("chip-error-{}", upload.id)),
                message.clone(),
                detail(t),
                c.status_failed,
            )
            .into_any_element(),
        };
        let retry_id = upload.id.clone();
        div()
            .id(SharedString::from(format!("chip-{id}")))
            .debug_selector(move || selector)
            .flex()
            .flex_none()
            .items_center()
            .gap(f.chip_gap)
            .h(f.chip_image)
            .when(!failed, |d| d.w(f.chip_file))
            .p(f.chip_pad)
            .rounded(t.radius.lg)
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(if failed {
                c.status_failed.opacity(t.opacity.failed_border)
            } else {
                c.border_subtle
            })
            .child(
                div()
                    .size(f.chip_tile)
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(t.radius.md)
                    .bg(tile_bg)
                    .child(icon(tile_icon, t.component.sidebar.row_icon, tile_color)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(t.space.xxs)
                    .child(line(upload.name.clone(), t.text.ui_strong, c.text_primary))
                    .child(status_line),
            )
            .when(failed, |d| {
                d.child(
                    button(
                        SharedString::from(format!("chip-retry-{retry_id}")),
                        "Try again",
                        ButtonKind::Secondary,
                        t,
                    )
                    .py(t.space.xxs)
                    .px(t.space.sm + t.space.xxs)
                    .type_style(detail(t))
                    .font_weight(t.text.ui_strong.weight)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let client = this.client.clone();
                        let id = retry_id.clone();
                        this.spawn_command(cx, async move {
                            client
                                .run(consort_client::Command::RetryUpload { id })
                                .await
                        });
                    })),
                )
            })
            .child(
                div()
                    .id(SharedString::from(format!("chip-remove-{id}")))
                    .self_start()
                    .size(f.chip_close)
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(t.radius.sm)
                    .cursor_pointer()
                    .hover(|s| s.bg(c.hover_raised))
                    .child(icon("x", f.chip_close_icon, c.text_tertiary))
                    .on_click(cx.listener(move |this, _, _, cx| remove(this, cx))),
            )
            .on_mouse_down(MouseButton::Right, context_menu)
            .into_any_element()
    }

    /// The drop state inside the composer: the upload icon, how many files
    /// attach, and the limit.
    pub fn render_drop_zone(&self, count: usize, t: &Tokens) -> Div {
        let c = &t.colors;
        let f = t.component.files;
        div()
            .debug_selector(|| "drop-zone".into())
            .flex()
            .flex_col()
            .items_center()
            .gap(t.space.xs + t.space.xxs)
            .pt(t.space.xs + t.space.xxs)
            .pb(t.space.xs)
            .child(
                div()
                    .size(f.drop_icon_tile)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(t.radius.full)
                    .bg(c.tint_accent)
                    .child(icon("upload", f.drop_icon, c.accent_text)),
            )
            .child(
                text(drop_label(count), t.text.body, c.text_primary)
                    .font_weight(t.text.title.weight),
            )
            .child(text("Up to 256 MB each", detail(t), c.text_tertiary))
    }

    // Attachments in the chat.

    /// The attachments of a message as one row of the chat: a grid of
    /// pictures, or one file bubble.
    pub fn render_attachment_row(
        &mut self,
        row: &AttachmentRow,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let message = self
            .state
            .timeline(&self.channel_id)
            .and_then(|tl| tl.items.get(row.item))
            .and_then(|i| match i {
                consort_client::TimelineItem::Message(m) => Some(m.clone()),
                _ => None,
            });
        let Some(message) = message else {
            return div().into_any_element();
        };
        let caption = match message.speaker {
            consort_client::Speaker::You => format!("You sent it at {}.", message.time_label),
            _ => format!("Consort sent it at {}.", message.time_label),
        };
        let lane = row.lane;
        let content = match &row.kind {
            AttachmentPart::Images(indexes) => {
                let images: Vec<AttachmentView> = indexes
                    .iter()
                    .filter_map(|i| message.attachments.get(*i).cloned())
                    .collect();
                self.render_image_tiles(&message.id, images, caption, t, cx)
            }
            AttachmentPart::File(index) => match message.attachments.get(*index).cloned() {
                Some(att) => self.render_file_bubble(&message.id, *index, att, lane, t, cx),
                None => div().into_any_element(),
            },
        };
        div()
            .flex()
            .w_full()
            .when(lane == Lane::You, |d| d.justify_end())
            .child(content)
            .into_any_element()
    }

    /// The pictures of a message, two in a row. A click opens the light box.
    fn render_image_tiles(
        &mut self,
        message_id: &str,
        images: Vec<AttachmentView>,
        caption: String,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let f = t.component.files;
        let mut grid = div()
            .flex()
            .flex_wrap()
            .justify_end()
            .gap(t.space.xs)
            .max_w(f.image_tile_w * 2.0 + t.space.xs);
        for (ix, image) in images.iter().enumerate() {
            if image.local_path.is_none() {
                self.want_blob(&image.sha256, image.size, cx);
            }
            let all = images.clone();
            let caption = caption.clone();
            let menu_att = image.clone();
            let channel = self.channel_id.clone();
            let selector = format!("image-{message_id}-{ix}");
            grid = grid.child(
                focusable(
                    div().id(SharedString::from(format!("image-{message_id}-{ix}"))),
                    t,
                )
                .debug_selector(move || selector)
                .w(f.image_tile_w)
                .h(f.image_tile_h)
                .flex_none()
                .rounded(t.component.chat.bubble_radius)
                .overflow_hidden()
                .bg(c.surface_card)
                .cursor_pointer()
                .child(match image.local_path.clone() {
                    Some(path) => img(PathBuf::from(path))
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .into_any_element(),
                    None => div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("image", t.icon.toolbar, c.text_tertiary))
                        .into_any_element(),
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_lightbox(all.clone(), ix, caption.clone(), window, cx)
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.open_menu(
                            OpenMenu::at(event.position, attachment_menu(&menu_att, &channel)),
                            cx,
                        );
                        cx.stop_propagation();
                    }),
                ),
            );
        }
        grid.into_any_element()
    }

    /// A file in a bubble. Yours is on `accent.surface`. The assistant's is
    /// on `surface.card`, and an artifact names its task. On hover it shows
    /// Save and Open.
    pub fn render_file_bubble(
        &mut self,
        owner_id: &str,
        index: usize,
        att: AttachmentView,
        lane: Lane,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let f = t.component.files;
        let chat = t.component.chat;
        let mine = lane == Lane::You;
        // The card of the open document has the selected ground.
        let selected = self.document_is_open(&att.sha256);
        let id: SharedString = format!("file-{owner_id}-{index}").into();
        let group: SharedString = format!("file-group-{owner_id}-{index}").into();
        let selector = id.to_string();
        let (save, open, menu_att) = (att.clone(), att.clone(), att.clone());
        let channel = self.channel_id.clone();
        let hover_bg = if mine {
            c.accent_surface
        } else {
            c.surface_raised
        };
        let second_line: AnyElement = match &att.artifact {
            Some(task) => div()
                .debug_selector({
                    let selector = format!("{id}-task");
                    move || selector
                })
                .flex()
                .items_center()
                .gap(t.space.xs + t.space.xxs)
                .min_w_0()
                .child(crate::ui::parts::status_glyph(
                    SharedString::from(format!("{id}-status")),
                    task.status,
                    t.icon.dense,
                    t,
                    cx,
                ))
                .child(line(task.task_name.clone(), detail(t), c.text_secondary))
                .child(text(att.size_label.clone(), detail(t), c.text_tertiary).flex_none())
                .into_any_element(),
            None => line(
                att.detail.clone(),
                detail(t),
                if mine {
                    c.text_secondary
                } else {
                    c.text_tertiary
                },
            )
            .into_any_element(),
        };
        let action = |name: &'static str, icon_name: &'static str, cx: &gpui_kit::App| {
            icon_button(
                SharedString::from(format!("{id}-{name}")),
                icon_name,
                f.bubble_action,
                t.icon.size,
                c.text_secondary,
                c.hover_raised,
                t,
                cx,
            )
        };
        focusable(div().id(id.clone()), t)
            .debug_selector(move || selector)
            .group(group.clone())
            .flex()
            .flex_none()
            .items_center()
            .gap(f.chip_gap)
            .w(if mine {
                f.file_bubble_you
            } else {
                f.file_bubble
            })
            .max_w_full()
            .py(f.bubble_pad_y)
            .pl(f.bubble_pad_left)
            .pr(t.space.sm)
            .rounded(chat.bubble_radius)
            .bg(if selected {
                c.surface_selected
            } else if mine {
                c.accent_surface
            } else {
                c.surface_card
            })
            .hover(move |s| s.bg(hover_bg))
            .cursor_pointer()
            .child(
                div()
                    .size(f.chip_tile)
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(t.radius.lg)
                    .bg(if mine {
                        c.scrim.opacity(t.opacity.you_tile)
                    } else {
                        c.surface_selected
                    })
                    .child(icon(
                        file_icon(&att.mime),
                        t.component.sidebar.row_icon,
                        c.text_secondary,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(line(att.name.clone(), t.text.ui_strong, c.text_primary))
                    .child(second_line),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .opacity(0.)
                    .group_hover(group, |s| s.opacity(1.))
                    .child(action("save", "download", cx).on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.save_blob(save.sha256.clone(), save.size, save.name.clone(), cx)
                        },
                    )))
                    .child(action("open", "external-link", cx).on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.open_blob(open.sha256.clone(), open.size, open.name.clone(), cx)
                        },
                    ))),
            )
            .on_click(cx.listener({
                let att = att.clone();
                move |this, _, _, cx| this.click_attachment(&att, cx)
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_menu(
                        OpenMenu::at(event.position, attachment_menu(&menu_att, &channel)),
                        cx,
                    );
                    cx.stop_propagation();
                }),
            )
            .into_any_element()
    }

    /// The attachments of a thread post: one compact row for each file,
    /// with its size. A picture opens the light box.
    pub fn render_post_attachments(
        &mut self,
        post_id: &str,
        list: &[AttachmentView],
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let images: Vec<AttachmentView> = list.iter().filter(|a| a.image).cloned().collect();
        let mut column = div().flex().flex_col().gap(t.space.xs).pt(t.space.sm);
        for (ix, att) in list.iter().enumerate() {
            let selector = format!("post-file-{post_id}-{ix}");
            let menu_att = att.clone();
            let open_att = att.clone();
            let channel = self.channel_id.clone();
            let images = images.clone();
            let image_index = images.iter().position(|a| a.sha256 == att.sha256);
            column = column.child(
                focusable(
                    div().id(SharedString::from(format!("post-file-{post_id}-{ix}"))),
                    t,
                )
                .debug_selector(move || selector)
                .flex()
                .items_center()
                .gap(t.space.sm)
                .py(t.space.xs + t.space.xxs)
                .px(t.space.sm + t.space.xxs)
                .rounded(t.radius.md)
                .bg(c.surface_raised)
                .cursor_pointer()
                .hover(|s| s.bg(c.hover_raised))
                .child(icon(file_icon(&att.mime), t.icon.dense, c.text_secondary))
                .child(
                    line(att.name.clone(), t.text.ui, c.text_primary)
                        .flex_1()
                        .min_w_0(),
                )
                .child(text(att.size_label.clone(), detail(t), c.text_tertiary).flex_none())
                .on_click(cx.listener(move |this, _, window, cx| match image_index {
                    Some(index) => {
                        this.open_lightbox(images.clone(), index, String::new(), window, cx)
                    }
                    None => this.open_blob(
                        open_att.sha256.clone(),
                        open_att.size,
                        open_att.name.clone(),
                        cx,
                    ),
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.open_menu(
                            OpenMenu::at(event.position, attachment_menu(&menu_att, &channel)),
                            cx,
                        );
                        cx.stop_propagation();
                    }),
                ),
            );
        }
        column.into_any_element()
    }
}

/// The progress bar of an upload: `status.working` on a faint track.
fn progress_bar(upload: &UploadView, height: gpui_kit::Pixels, t: &Tokens) -> Div {
    let c = &t.colors;
    let fraction = match &upload.status {
        UploadStatus::Uploading { fraction, .. } => *fraction,
        UploadStatus::Ready => 1.0,
        _ => 0.0,
    };
    div()
        .w_full()
        .h(height)
        .rounded(t.radius.full)
        .bg(c.status_working.opacity(t.opacity.progress_track))
        .child(
            div()
                .h_full()
                .w(gpui_kit::relative(fraction))
                .rounded(t.radius.full)
                .bg(c.status_working),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paste_takes_pictures_and_files_and_leaves_text_to_the_box() {
        let image = gpui_kit::ClipboardItem {
            entries: vec![ClipboardEntry::Image(gpui_kit::Image::from_bytes(
                ImageFormat::Png,
                vec![1, 2, 3],
            ))],
        };
        let inputs = paste_inputs(&image, "740pm");
        assert_eq!(inputs.len(), 1);
        assert!(
            matches!(&inputs[0], AttachInput::Pasted { name, mime, bytes }
            if name == "pasted-image-740pm.png" && mime == "image/png" && bytes.0.len() == 3)
        );
        let words = gpui_kit::ClipboardItem::new_string("hello".into());
        assert!(paste_inputs(&words, "x").is_empty());
    }

    #[test]
    fn the_drop_label_counts_the_files() {
        assert_eq!(drop_label(1), "Drop to attach 1 file");
        assert_eq!(drop_label(3), "Drop to attach 3 files");
    }
}
