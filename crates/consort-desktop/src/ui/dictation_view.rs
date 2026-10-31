//! How dictation looks in a message box (design V1 to V4): the live mic, the
//! line at the right of the tool row, and the notices.

use consort_dictation::{Phase, format_size};
use consort_theme::Tokens;
use gpui_kit::{
    AnyElement, App, Context, Div, ElementId, Focusable as _, InteractiveElement as _,
    IntoElement as _, ParentElement, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::app::ConsortApp;
use crate::dictation::{DictationNotice, DownloadRow};
use crate::ui::dialogs::{field_block, input_box};
use crate::ui::menu::TextTarget;
use crate::ui::parts::{ButtonKind, button, detail, icon, icon_button, selectable, text};

/// The line of a dictation at the right of the tool row.
pub fn side_line(phase: Phase) -> Option<&'static str> {
    match phase {
        Phase::Idle => None,
        Phase::Starting => Some("Getting the speech model ready"),
        Phase::Listening => Some("Esc cancels"),
        Phase::Finishing => Some("Fixing the text"),
    }
}

/// The heights of the three level bars. The middle bar is the tallest.
fn bar_heights(level: f32, max: f32, min: f32) -> [f32; 3] {
    let level = level.clamp(0.0, 1.0);
    let bar = |weight: f32| min + (max - min) * level * weight;
    [bar(0.6), bar(1.0), bar(0.75)]
}

impl ConsortApp {
    /// True when a dictation runs in this box.
    pub(crate) fn dictating_in(&self, target: TextTarget) -> bool {
        self.dictation.phase.is_active() && self.dictation.target == Some(target)
    }

    /// The mic of a message box. At rest it is a tool button. While dictation
    /// runs it keeps its size and shows what a click does.
    pub(crate) fn render_mic(
        &self,
        id: &'static str,
        target: TextTarget,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let chat = t.component.chat;
        let here = self.dictating_in(target);
        let phase = if here {
            self.dictation.phase
        } else {
            Phase::Idle
        };
        let downloading = matches!(self.dictation.notice, Some(DictationNotice::Downloading(_)));
        let failed = self.dictation.failed_dot == Some(target);
        let tip = match phase {
            Phase::Idle => "Dictate",
            _ => "Stop dictation",
        };
        let shell = icon_button(
            id,
            "voice",
            t.component.composer_chip,
            t.icon.size,
            if downloading {
                c.text_tertiary
            } else {
                c.text_secondary
            },
            c.hover_canvas,
            t,
            cx,
        );
        let mic = match phase {
            Phase::Idle => shell,
            Phase::Finishing => live_mic(id, t).child(crate::ui::parts::status_glyph(
                id,
                consort_client::TaskStatus::Working,
                t.icon.size,
                t,
                cx,
            )),
            Phase::Starting | Phase::Listening => {
                let [a, b, d] = bar_heights(
                    self.dictation.level,
                    f32::from(chat.mic_bar_max),
                    f32::from(chat.mic_bar),
                );
                let bar = |height: f32| {
                    div()
                        .w(chat.mic_bar)
                        .h(gpui_kit::px(height))
                        .rounded(t.radius.full)
                        .bg(c.accent_text)
                };
                live_mic(id, t)
                    .gap(t.space.xxs)
                    .child(
                        div()
                            .size(chat.mic_stop)
                            .rounded(t.radius.sm)
                            .bg(c.accent_text),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(chat.mic_bar_gap)
                            .child(bar(a))
                            .child(bar(b))
                            .child(bar(d)),
                    )
            }
        };
        let tip_view = move |_: &mut Window, cx: &mut App| {
            gpui_kit::AppContext::new(cx, |_| crate::ui::convo_sidebar::Tip(tip.into())).into()
        };
        div()
            .relative()
            .flex_none()
            .child(
                mic.debug_selector(move || id.into())
                    .tooltip(tip_view)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_dictation(target, window, cx)
                    })),
            )
            .when(failed, |d| {
                let selector = format!("{id}-failed");
                d.child(
                    div()
                        .debug_selector(move || selector)
                        .absolute()
                        .top(t.space.xxs)
                        .right(t.space.xxs)
                        .size(chat.mic_dot)
                        .rounded_full()
                        .bg(c.status_failed)
                        .border(t.border_width)
                        .border_color(c.surface_canvas),
                )
            })
            .into_any_element()
    }

    /// The notice of the dictation, in the corner stack.
    pub(crate) fn render_dictation_notice(
        &self,
        notice: &DictationNotice,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let is_error = notice.is_error();
        let glyph = if is_error {
            icon("failed", t.icon.size, c.status_failed)
        } else {
            icon("info", t.icon.size, c.status_working)
        };
        let rows = notice_rows(notice, t);
        let buttons = notice.buttons();
        let action_row = (!buttons.is_empty()).then(|| {
            div().flex().gap(t.space.sm).pt(t.space.sm).children(
                buttons
                    .into_iter()
                    .enumerate()
                    .map(|(index, (action, label))| {
                        let selector = format!("dictation-notice-button-{index}");
                        button(
                            ElementId::Name(SharedString::from(selector.clone())),
                            label,
                            ButtonKind::Secondary,
                            t,
                        )
                        .debug_selector(move || selector)
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.run_dictation_notice(action.clone(), window, cx)
                            },
                        ))
                    }),
            )
        });
        div()
            .id("dictation-notice")
            .debug_selector(|| "dictation-notice".into())
            .flex()
            .items_start()
            .gap(t.space.sm + t.space.xxs)
            .py(t.space.sm + t.space.xxs)
            .pl(t.space.md)
            .pr(t.space.sm)
            .bg(c.surface_raised)
            .border(t.border_width)
            .border_color(if is_error {
                gpui_kit::transparent_black()
            } else {
                c.border_strong
            })
            .rounded(t.radius.lg)
            .shadow(t.shadow_float())
            .child(div().flex_none().pt(t.space.xs).child(glyph))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .pt(t.space.xxs)
                    .gap(t.space.xxs)
                    .child(selectable(
                        "dictation-notice-title",
                        notice.title(),
                        t.text.ui_strong,
                        c.text_primary,
                    ))
                    .child(selectable(
                        "dictation-notice-text",
                        notice.text(),
                        t.text.ui,
                        c.text_secondary,
                    ))
                    .children(rows)
                    .children(action_row),
            )
            .child(
                icon_button(
                    "dictation-notice-close",
                    "close",
                    t.space.xl,
                    t.icon.dense,
                    c.text_tertiary,
                    c.hover_raised,
                    t,
                    cx,
                )
                .debug_selector(|| "dictation-notice-close".into())
                .on_click(cx.listener(|this, _, _, cx| this.close_dictation_notice(cx))),
            )
            .into_any_element()
    }

    /// The dialog of the Deepgram key.
    pub(crate) fn deepgram_key_dialog(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (String, String, gpui_kit::Pixels, Div, Div) {
        let focused = self
            .inputs
            .api_key
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        (
            "Add your Deepgram key".to_owned(),
            "Consort checks the key with Deepgram, then keeps it on this computer. It never goes to the hub.".to_owned(),
            t.layout.dialog_width,
            field_block("Key", input_box(&self.inputs.api_key, focused, t), None, t),
            self.dialog_actions("Add key", t, cx, move |this, window, cx| {
                let key = this.field_text(&this.inputs.api_key, cx);
                this.save_deepgram_key(key, window, cx);
            }),
        )
    }

    /// Clears the key field and opens the dialog of the Deepgram key.
    pub(crate) fn clear_deepgram_key_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.inputs
            .api_key
            .update(cx, |input, cx| input.set_value("", window, cx));
    }
}

/// The base of the live mic: the size of the tool button, with an accent tint.
fn live_mic(id: &'static str, t: &Tokens) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(t.component.composer_chip)
        .rounded(t.radius.md)
        .bg(t.colors.accent_fill.opacity(t.opacity.tint))
        .cursor_pointer()
}

/// The rows of a notice with models.
fn notice_rows(notice: &DictationNotice, t: &Tokens) -> Option<Div> {
    let c = &t.colors;
    match notice {
        DictationNotice::ModelsMissing(needs) => Some(
            div()
                .flex()
                .flex_col()
                .gap(t.space.xs)
                .pt(t.space.sm)
                .children(
                    needs
                        .iter()
                        .filter_map(|need| need.line())
                        .map(|line| text(line, detail(t), c.text_secondary)),
                ),
        ),
        DictationNotice::Downloading(rows) => Some(
            div()
                .flex()
                .flex_col()
                .gap(t.space.sm)
                .pt(t.space.sm)
                .children(
                    rows.iter()
                        .filter(|row| row.need.line().is_some())
                        .map(|row| download_row(row, t)),
                ),
        ),
        _ => None,
    }
}

fn download_row(row: &DownloadRow, t: &Tokens) -> Div {
    let c = &t.colors;
    let fraction = if row.finished {
        1.0
    } else if row.total == 0 {
        0.0
    } else {
        (row.done as f32 / row.total as f32).clamp(0.0, 1.0)
    };
    div()
        .flex()
        .flex_col()
        .gap(t.space.xxs)
        .child(
            div()
                .flex()
                .justify_between()
                .gap(t.space.sm)
                .child(text(row.need.name.clone(), detail(t), c.text_secondary))
                .child(text(progress_line(row), detail(t), c.text_tertiary)),
        )
        .child(
            div()
                .w_full()
                .h(t.component.chat.mic_bar)
                .rounded(t.radius.full)
                .bg(c.border_strong)
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(gpui_kit::relative(fraction))
                        .rounded(t.radius.full)
                        .bg(if row.finished {
                            c.status_done
                        } else {
                            c.status_working
                        }),
                ),
        )
}

/// "483 MB, done", "130 of 222 MB" or "Waiting, 639 MB".
pub fn progress_line(row: &DownloadRow) -> String {
    if row.finished {
        format!("{}, done", format_size(row.need.size_bytes))
    } else if row.done == 0 {
        format!("Waiting, {}", format_size(row.need.size_bytes))
    } else {
        let total = if row.total == 0 {
            row.need.size_bytes
        } else {
            row.total
        };
        let of = format_size(total);
        let unit = of.rsplit(' ').next().unwrap_or_default();
        let done = format_size(row.done);
        let number = done
            .strip_suffix(unit)
            .map_or(done.as_str(), str::trim_end)
            .to_owned();
        format!("{number} of {of}")
    }
}
