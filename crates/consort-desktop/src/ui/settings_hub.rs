//! Settings > Hub (Paper `7JP-0`): the address, the versions, the data
//! folder, the storage bar, and the backups with their schedule.

use consort_client::LoadState;
use consort_client::admin::{BackupFrequency, HubStatusView, LastBackupView};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Div, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement, StatefulInteractiveElement as _, Styled, Window, div,
    prelude::FluentBuilder as _, relative,
};

use crate::admin::{KEEP_SELECT, TIME_SELECT, WEEKDAY_SELECT, select_options};
use crate::app::ConsortApp;
use crate::ui::admin_parts::{
    column, copy_button, header, hint, mono_value, page, row, row_label, rows, section_title,
    select, status_dot,
};
use crate::ui::dialogs::input_box;
use crate::ui::parts::{ButtonKind, button, detail, error_box, icon, status_glyph, text};
use crate::ui::settings::segmented;

impl ConsortApp {
    pub fn render_hub(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        if self.admin.reach_open {
            return self.render_reach(t, cx);
        }
        let hub = self.state.admin.hub.clone();
        if hub.address.is_empty() {
            return page("hub-page", t)
                .child(column(t).child(self.hub_loading(&hub, t, cx)))
                .into_any_element();
        }
        let is_local = hub.install == Some(consort_client::local_hub::InstallKind::Local);
        let line = if is_local {
            Self::local_hub_line(&hub, t)
        } else {
            div()
                .flex()
                .items_center()
                .gap(t.space.sm)
                .child(status_dot(Some(hub.online).filter(|o| *o), t))
                .child(text(hub.summary.clone(), t.text.ui, c.text_secondary))
        };
        // A hub on this computer has W4 at the top. The address, the storage
        // and the backups stay under it.
        let local_rows = is_local.then(|| self.local_hub_section(t, cx));
        page("hub-page", t)
            .child(
                column(t)
                    .child(header("Hub", line, None, t))
                    .when_some(self.state.boundary_warning(), |d, warning| {
                        d.child(
                            error_box("hub-boundary-warning", warning, t)
                                .debug_selector(|| "hub-boundary-warning".into()),
                        )
                    })
                    .children(self.hub_update_card(&hub, t, cx))
                    .children(local_rows)
                    .child(self.hub_connection(&hub, t, cx))
                    .child(hub_storage(&hub, t))
                    .child(self.hub_backup(&hub, t, window, cx)),
            )
            .into_any_element()
    }

    fn hub_loading(&self, hub: &HubStatusView, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let body = match &hub.load {
            LoadState::Failed(message) | LoadState::Unsupported(message) => div()
                .flex()
                .flex_col()
                .items_start()
                .gap(t.space.md)
                .child(error_box("hub-error", message.clone(), t).w_full())
                .child(
                    button("hub-retry", "Try again", ButtonKind::Secondary, t).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.enter_admin_section(crate::app::SettingsSection::Hub, cx)
                        }),
                    ),
                ),
            _ => text(
                "Getting the status of the hub.",
                t.text.ui,
                c.text_secondary,
            ),
        };
        header("Hub", div(), None, t).child(div()).child(body)
    }

    fn hub_connection(&self, hub: &HubStatusView, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let mono = |s: String| {
            div()
                .font_family(t.font_mono.clone())
                .type_style(t.text.mono)
                .child(s)
        };
        let version = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .type_style(t.text.ui)
                    .text_color(c.text_primary)
                    .child("Hub\u{a0}")
                    .child(mono(hub.hub_version.clone()))
                    .child(", protocol\u{a0}")
                    .child(mono(hub.protocol.clone())),
            )
            .child(hint(hub.protocol_line.clone(), t));
        div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(section_title("Connection", t))
            .child(
                rows(t)
                    .child(
                        row(false, t)
                            .child(row_label("Address", t))
                            .child(
                                mono_value("hub-address", hub.address.clone(), t)
                                    .flex_1()
                                    .min_w_0(),
                            )
                            .child(copy_button(
                                "copy-hub-address",
                                hub.address.clone(),
                                "hub address",
                                t.component.admin.copy_button,
                                t,
                                cx,
                            )),
                    )
                    .child(
                        row(false, t)
                            .child(row_label("Reach", t))
                            .child(
                                text(
                                    self.state.admin.reach.summary.clone(),
                                    t.text.ui,
                                    c.text_primary,
                                )
                                .flex_1()
                                .min_w_0(),
                            )
                            .child(
                                button("hub-reach", "Manage", ButtonKind::Secondary, t)
                                    .flex_none()
                                    .debug_selector(|| "hub-reach".to_owned())
                                    .on_click(cx.listener(|this, _, _, cx| this.open_reach(cx))),
                            ),
                    )
                    .child(row(false, t).child(row_label("Version", t)).child(version))
                    .child(
                        row(true, t).child(row_label("Data folder", t)).child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(mono_value("hub-data-dir", hub.data_dir.clone(), t))
                                .child(hint(
                                    "On the hub machine. It holds the database, the blobs and the skills.",
                                    t,
                                )),
                        ),
                    ),
            )
    }

    fn hub_backup(
        &self,
        hub: &HubStatusView,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = &t.colors;
        let backup = &hub.backup;
        let busy = backup.busy;
        let back_up = button("back-up-now", "Back up now", ButtonKind::Secondary, t)
            .debug_selector(|| "back-up-now".to_owned())
            .when(busy, |b| b.opacity(t.opacity.busy).cursor_default())
            .when(!busy, |b| {
                b.on_click(cx.listener(|this, _, _, cx| this.back_up_now(cx)))
            });
        let title = div()
            .flex()
            .items_end()
            .gap(t.space.lg)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(t.space.xxs)
                    .child(section_title("Backup", t))
                    .child(text(
                        "A backup copies the database and the blobs to a folder on the hub machine.",
                        t.text.ui,
                        c.text_tertiary,
                    )),
            )
            .child(back_up);

        // Last backup.
        let last = match &backup.last {
            LastBackupView::None => div()
                .flex()
                .flex_col()
                .child(text("No backup yet.", t.text.ui, c.text_primary))
                .child(hint("Choose Back up now, or set a schedule.", t)),
            LastBackupView::Running { line, detail, .. } => div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.xs + t.space.xxs)
                        .child(status_glyph(
                            "backup-running",
                            consort_client::TaskStatus::Working,
                            t.icon.dense,
                            t,
                            cx,
                        ))
                        .child(text(line.clone(), t.text.ui, c.text_primary)),
                )
                .when(!detail.is_empty(), |d| d.child(hint(detail.clone(), t))),
            LastBackupView::Done {
                line,
                file_name,
                size,
            } => div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.xs + t.space.xxs)
                        .child(icon("check", t.icon.dense, c.status_done))
                        .child(text(line.clone(), t.text.ui, c.text_primary)),
                )
                .child(
                    div()
                        .flex()
                        .type_style(detail(t))
                        .text_color(c.text_tertiary)
                        .child(
                            div()
                                .font_family(t.font_mono.clone())
                                .text_color(c.text_secondary)
                                .child(gpui_kit::base::SelectableText::new(
                                    "backup-file",
                                    file_name.clone(),
                                )),
                        )
                        .child(format!(", {size}")),
                ),
            LastBackupView::Failed { line, error } => div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.xs + t.space.xxs)
                        .child(icon("failed", t.icon.dense, c.status_failed))
                        .child(text(line.clone(), t.text.ui, c.text_primary)),
                )
                .child(crate::ui::parts::selectable(
                    "backup-error",
                    error.clone(),
                    detail(t),
                    c.text_secondary,
                )),
        };

        // Schedule.
        let frequency = backup.frequency;
        let schedule = row(false, t)
            .child(row_label("Schedule", t))
            .child(div().flex_1().min_w_0())
            .child(
                segmented(
                    "backup-frequency",
                    &[
                        (BackupFrequency::Off, "Off"),
                        (BackupFrequency::Daily, "Daily"),
                        (BackupFrequency::Weekly, "Weekly"),
                    ],
                    frequency,
                    t,
                    cx,
                    |_| t.text.ui,
                    |this, value, _, cx| this.set_backup_frequency(value, cx),
                )
                .flex_none(),
            )
            .when(frequency == BackupFrequency::Weekly, |r| {
                r.child(select(
                    WEEKDAY_SELECT,
                    backup.weekday_label.clone(),
                    t.component.admin.time_select,
                    select_options(WEEKDAY_SELECT, backup),
                    self,
                    t,
                    cx,
                ))
            })
            .when(frequency != BackupFrequency::Off, |r| {
                r.child(select(
                    TIME_SELECT,
                    backup.time_label.clone(),
                    t.component.admin.time_select,
                    select_options(TIME_SELECT, backup),
                    self,
                    t,
                    cx,
                ))
            });

        // Folder.
        let folder = if self.admin.folder_editing {
            let focused = self
                .admin
                .folder_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window);
            row(false, t)
                .child(row_label("Folder", t))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(t.font_mono.clone())
                        .child(input_box(&self.admin.folder_input, focused, t)),
                )
                .child(
                    button("save-backup-folder", "Save", ButtonKind::Secondary, t).on_click(
                        cx.listener(|this, _, window, cx| this.save_backup_folder(window, cx)),
                    ),
                )
                .child(
                    button("cancel-backup-folder", "Cancel", ButtonKind::Quiet, t).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.admin.folder_editing = false;
                            cx.notify();
                        }),
                    ),
                )
        } else {
            row(false, t)
                .child(row_label("Folder", t))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(mono_value("backup-folder", backup.folder.clone(), t))
                        .child(hint(backup.folder_hint.clone(), t)),
                )
                .child(
                    button(
                        "change-backup-folder",
                        "Change folder",
                        ButtonKind::Quiet,
                        t,
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| this.edit_backup_folder(window, cx)),
                    ),
                )
        };

        let keep = row(true, t)
            .child(row_label("Keep", t))
            .child(
                hint("The hub deletes older backups after a new one finishes.", t)
                    .flex_1()
                    .min_w_0(),
            )
            .child(select(
                KEEP_SELECT,
                backup.keep_label.clone(),
                t.component.settings_select,
                select_options(KEEP_SELECT, backup),
                self,
                t,
                cx,
            ));

        let command = backup.restore_command.clone();
        let copy = command.clone();
        let restore = div()
            .flex()
            .items_center()
            .gap(t.space.md)
            .py(t.space.sm + t.space.xxs)
            .pl(t.space.lg)
            .pr(t.space.sm + t.space.xxs)
            .bg(c.surface_card)
            .rounded(t.radius.lg)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .type_style(t.text.ui)
                    .text_color(c.text_secondary)
                    .child("To restore, run\u{a0}")
                    .child(
                        div()
                            .font_family(t.font_mono.clone())
                            .text_color(c.text_primary)
                            .child(gpui_kit::base::SelectableText::new(
                                "restore-command",
                                command,
                            )),
                    )
                    .child("\u{a0}on the hub machine."),
            )
            .child(
                button("copy-restore", "Copy command", ButtonKind::Quiet, t)
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .py(t.space.xs)
                    .text_color(c.text_primary)
                    .child(icon("copy", t.icon.dense, c.text_secondary))
                    .flex_row_reverse()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                        this.show_notice("Copied the restore command.".into(), cx);
                    })),
            );

        div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(title)
            .child(
                rows(t)
                    .child(
                        row(false, t)
                            .child(row_label("Last backup", t))
                            .child(last.flex_1().min_w_0()),
                    )
                    .child(schedule)
                    .child(folder)
                    .child(keep),
            )
            .children(
                backup
                    .error
                    .clone()
                    .map(|err| error_box("backup-settings-error", err, t)),
            )
            .child(restore)
    }
}

/// The storage bar: Consort's own use split by kind, in neutral text
/// colors, and the free disk space as text.
fn hub_storage(hub: &HubStatusView, t: &Tokens) -> Div {
    let c = &t.colors;
    let a = &t.component.admin;
    let colors = [c.text_primary, c.text_tertiary, c.text_secondary];
    let mut bar = div()
        .flex()
        .gap(t.space.xxs)
        .h(a.storage_bar)
        .w_full()
        .bg(c.surface_card)
        .rounded(t.radius.full)
        .overflow_hidden();
    let mut legend = div().flex().gap(a.legend_gap);
    for (part, color) in hub.storage.parts.iter().zip(colors) {
        if part.share > 0.0 {
            bar = bar.child(
                div()
                    .h_full()
                    .flex_shrink(1.0)
                    .w(relative(part.share))
                    .bg(color),
            );
        }
        legend = legend.child(
            div()
                .flex()
                .items_center()
                .gap(t.space.sm)
                .child(
                    div()
                        .size(a.legend_swatch)
                        .flex_none()
                        .rounded(a.swatch_radius)
                        .bg(color),
                )
                .child(
                    div()
                        .flex()
                        .type_style(t.text.ui)
                        .text_color(c.text_secondary)
                        .child(format!("{}\u{a0}", part.label))
                        .child(
                            div()
                                .font_family(t.font_mono.clone())
                                .text_color(c.text_primary)
                                .child(part.size.clone()),
                        ),
                ),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap(t.space.md)
        .child(
            div()
                .flex()
                .items_end()
                .gap(t.space.md)
                .child(section_title("Storage", t).flex_1().min_w_0())
                .child(text(hub.storage.summary.clone(), t.text.ui, c.text_tertiary).min_w_0()),
        )
        .child(bar)
        .child(legend)
}
