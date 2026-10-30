//! The composer: one component for the chat and the task thread.
//!
//! It is the rounded field with Send inside at the right, and under it one
//! row of tools: add, the model chip, the effort chip, a spacer and where the
//! assistant or the task runs. The target (a channel or a task) decides only
//! the text box, the files, the model that the chips change and the words
//! that go with them. The sizes and the padding are the same, so with the
//! panel open the two fields and the two tool rows line up at the bottom of
//! the window.
//!
//! In a narrow column the tool row drops "Runs on" first and then the account
//! name. It never overflows and never wraps.

use consort_client::{ChannelKind, UploadTarget};
use consort_theme::{Tokens, Type, TypeExt as _};
use gpui_kit::{
    AnyElement, App, Context, Entity, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, Pixels, SharedString, StatefulInteractiveElement as _, Styled, TextRun, Window,
    component::input::TextareaState, div, font, prelude::FluentBuilder as _, px,
};

use crate::{
    app::{ConsortApp, DropOver},
    ui::{
        menu::{OpenMenu, TextTarget, text_edit_menu},
        model_picker::{PickerTarget, TriggerStyle},
        parts::{detail, icon_button, text},
    },
};

/// What a composer writes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerTarget {
    /// The main composer: the open channel.
    Channel,
    /// The composer of a task thread, by task ID.
    Task(String),
}

/// The element IDs of one composer, so that two composers can be on screen
/// at once and each keeps its own bounds.
#[derive(Clone, Copy)]
struct Ids {
    field: &'static str,
    text: &'static str,
    send: &'static str,
    attach: &'static str,
    mic: &'static str,
    divider: &'static str,
    tools: &'static str,
    model: &'static str,
    effort: &'static str,
    side: &'static str,
}

const CHANNEL_IDS: Ids = Ids {
    field: "composer",
    text: "composer-text",
    send: "send",
    attach: "composer-attach",
    mic: "voice",
    divider: "composer-divider",
    tools: "composer-tools",
    model: "composer-model",
    effort: "composer-effort",
    side: "runs-on",
};

const TASK_IDS: Ids = Ids {
    field: "reply-box",
    text: "reply-text",
    send: "reply-send",
    attach: "reply-attach",
    mic: "reply-voice",
    divider: "reply-divider",
    tools: "reply-tools",
    model: "reply-model",
    effort: "reply-effort",
    side: "reply-runs-on",
};

/// What differs between the two composers.
struct Spec {
    ids: Ids,
    input: Entity<TextareaState>,
    upload: UploadTarget,
    picker: PickerTarget,
    text_target: TextTarget,
    drop: DropOver,
    /// "Runs on hub", or `None` while nothing is known.
    runs_on: Option<String>,
    /// The channel uses the model of Tutti.
    inherited: bool,
    /// The model chips show. A task with no model has none.
    chips: bool,
}

/// What the tool row can show in the room that it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolFit {
    /// The account name in the model chip.
    pub account: bool,
    /// "Runs on <worker>" at the right end.
    pub side: bool,
}

/// The widths that decide the fit, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolWidths {
    /// The row with every part but the account name and the side label.
    pub fixed: f32,
    /// The account name and its gap.
    pub account: f32,
    /// The side label and its gap.
    pub side: f32,
}

/// The tool row drops "Runs on" first and then the account name. What is
/// left is never wider than `room`, unless even the fixed part is.
pub fn fit_tools(room: f32, widths: ToolWidths) -> ToolFit {
    let with_account = widths.fixed + widths.account;
    if with_account + widths.side <= room {
        ToolFit {
            account: true,
            side: true,
        }
    } else if with_account <= room {
        ToolFit {
            account: true,
            side: false,
        }
    } else {
        ToolFit {
            account: false,
            side: false,
        }
    }
}

/// The width of one line of text in a type style.
fn text_width(window: &Window, t: &Tokens, content: &str, style: Type) -> Pixels {
    if content.is_empty() {
        return px(0.);
    }
    let run = TextRun {
        len: content.len(),
        font: gpui_kit::Font {
            weight: style.weight,
            ..font(t.font_sans.clone())
        },
        color: t.colors.text_primary,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(
            SharedString::from(content.to_owned()),
            style.size,
            &[run],
            None,
        )
        .width
}

impl ConsortApp {
    /// A click on the mic button. The box takes the focus, so dictation types
    /// into it. `started` is the answer of the platform call. When the system
    /// did not start dictation, the hint shows as an information notice.
    pub fn click_mic(
        &mut self,
        input: Entity<TextareaState>,
        started: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        input.update(cx, |input, cx| input.focus(window, cx));
        if let crate::dictation::MicOutcome::Hint(hint) =
            crate::dictation::outcome(started, crate::convo::dictation_hint())
        {
            self.show_notice(hint, cx);
        }
    }

    /// The width of the column that the chat has now: the window less the
    /// sidebar and the panel beside it.
    pub fn chat_column_width(&self, t: &Tokens) -> Pixels {
        let frame = self.layout_frame(t);
        let window: Pixels = self.layout.window_width;
        let sidebar = if frame.sidebar {
            t.layout.sidebar_width
        } else {
            px(0.)
        };
        let panel = if frame.panel_over {
            px(0.)
        } else {
            px(frame.panel)
        };
        (window - sidebar - panel).max(px(0.))
    }

    fn composer_spec(&self, target: &ComposerTarget) -> Spec {
        match target {
            ComposerTarget::Channel => {
                let channel = self.state.channel(&self.channel_id);
                // A channel without its own model says that it uses Tutti's.
                let inherited = channel.is_some_and(|ch| {
                    ch.kind != ChannelKind::Tutti
                        && ch
                            .coordinator_selection
                            .as_ref()
                            .is_some_and(|e| e.source == consort_client::SelectionSource::Tutti)
                });
                let runs_on = channel
                    .and_then(|ch| ch.coordinator_worker.clone())
                    .filter(|w| !w.trim().is_empty())
                    .map(|w| format!("Runs on {w}"));
                Spec {
                    ids: CHANNEL_IDS,
                    input: self.inputs.composer.clone(),
                    upload: UploadTarget::Composer {
                        channel_id: self.channel_id.clone(),
                    },
                    picker: PickerTarget::Assistant(self.channel_id.clone()),
                    text_target: TextTarget::Composer,
                    drop: DropOver::Chat,
                    runs_on,
                    inherited,
                    chips: channel.is_some(),
                }
            }
            ComposerTarget::Task(task_id) => {
                let thread = self.state.thread(task_id);
                Spec {
                    ids: TASK_IDS,
                    input: self.inputs.reply.clone(),
                    upload: UploadTarget::Reply {
                        task_id: task_id.clone(),
                    },
                    picker: PickerTarget::Task(task_id.clone()),
                    text_target: TextTarget::Reply,
                    drop: DropOver::Reply,
                    runs_on: thread.and_then(|th| th.runs_on.clone()),
                    inherited: false,
                    chips: thread.is_some_and(|th| th.selection.is_some()),
                }
            }
        }
    }

    /// What the tool row has room for, from the width of the column that
    /// holds the composer.
    fn tool_fit(
        &self,
        spec: &Spec,
        side: Option<&str>,
        column: Pixels,
        t: &Tokens,
        window: &Window,
    ) -> ToolFit {
        let room = (column - t.space.xl * 2.0).min(t.layout.timeline_max_width) - t.space.xs * 2.0;
        let gap = t.space.xs + t.space.xxs;
        let row_gap = t.space.xxs;
        let locked = self.picker_locked(&spec.picker);
        let chevron = if locked { px(0.) } else { gap + t.space.md };
        // The attach button and the divider.
        let mut fixed = t.component.composer_chip
            + row_gap
            + t.border_width
            + gap * 2.0
            + row_gap
            // The spacer is a child of the row too.
            + row_gap;
        let mut account = px(0.);
        if spec.inherited {
            let same = format!("Same as {}", crate::ui::TUTTI);
            fixed += text_width(window, t, &same, detail(t)) + t.space.xs + row_gap;
        }
        if spec.chips {
            let (name, account_name, logo) = self.chip_texts(&spec.picker);
            let strong = Type {
                weight: t.text.title.weight,
                ..t.text.label
            };
            fixed += t.space.sm * 2.0
                + if logo { t.icon.dense + gap } else { px(0.) }
                + text_width(window, t, &name, strong)
                + chevron
                + row_gap;
            if !account_name.is_empty() {
                account = gap + text_width(window, t, &account_name, detail(t));
            }
            if let Some(label) = self.effort_chip_label(&spec.picker) {
                fixed += t.space.sm * 2.0
                    + t.icon.dense
                    + gap
                    + text_width(window, t, &label, detail(t))
                    + chevron
                    + row_gap;
            }
        }
        let side = side
            .map(|s| text_width(window, t, s, detail(t)) + row_gap)
            .unwrap_or_default();
        fit_tools(
            room.into(),
            ToolWidths {
                fixed: fixed.into(),
                account: account.into(),
                side: side.into(),
            },
        )
    }

    /// The composer of the chat or of a task thread. `column` is the width of
    /// the column that holds it, and `above` is a line over the field, such
    /// as the state of the link.
    pub fn render_composer_box(
        &mut self,
        target: ComposerTarget,
        column: Pixels,
        above: Option<AnyElement>,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &t.colors;
        let chat = t.component.chat;
        let spec = self.composer_spec(&target);
        let ids = spec.ids;
        let is_channel = target == ComposerTarget::Channel;
        let hub_offline = self.state.saved_data.is_some();
        // Nothing runs while the hub is down. A message waits.
        let side: Option<(String, &'static str)> = if hub_offline {
            is_channel.then(|| {
                (
                    "Messages wait here until the hub is back".to_owned(),
                    "queue-hint",
                )
            })
        } else {
            spec.runs_on.clone().map(|s| (s, ids.side))
        };
        let fit = self.tool_fit(
            &spec,
            side.as_ref().map(|(s, _)| s.as_str()),
            column,
            t,
            window,
        );
        let model_style = if fit.account {
            TriggerStyle::Chip
        } else {
            TriggerStyle::ChipShort
        };
        let model_chip = spec
            .chips
            .then(|| self.render_model_trigger(ids.model, spec.picker.clone(), model_style, t, cx));
        let effort_chip = spec
            .chips
            .then(|| {
                self.render_effort_trigger(
                    ids.effort,
                    spec.picker.clone(),
                    TriggerStyle::Chip,
                    t,
                    cx,
                )
            })
            .flatten();
        let input = crate::ui::parts::textarea(&spec.input, t.text.body);
        let chips = self.render_upload_chips(&spec.upload, t, cx);
        let waiting = is_channel && self.waiting().is_some();
        let dropping = self
            .drop_hover
            .filter(|(over, _)| *over == spec.drop)
            .map(|(_, count)| count)
            .filter(|_| !waiting);
        let drop_zone = dropping.map(|count| self.render_drop_zone(count, t));
        let paste_target = spec.upload.clone();
        let add_target = spec.upload.clone();
        let focus_input = spec.input.clone();
        let mic_input = spec.input.clone();
        let send_target = target.clone();
        let send = icon_button(
            ids.send,
            "send",
            chat.send_button,
            t.icon.size,
            c.text_tertiary,
            c.hover_card,
            t,
            cx,
        )
        .rounded(t.radius.lg)
        .debug_selector(|| ids.send.into())
        .on_click(cx.listener(move |this, _, window, cx| {
            match &send_target {
                ComposerTarget::Channel => this.send_message(window, cx),
                ComposerTarget::Task(_) => this.send_reply(window, cx),
            }
            // The button takes the focus. The box gets it back, so the user
            // can go on typing.
            focus_input.update(cx, |input, cx| input.focus(window, cx));
        }));
        let tool = |id: &'static str, name: &str, cx: &App| {
            icon_button(
                id,
                name,
                t.component.composer_chip,
                t.icon.size,
                c.text_secondary,
                c.hover_canvas,
                t,
                cx,
            )
        };
        let inherited = spec.inherited;
        let text_target = spec.text_target;
        let field = div()
            .id(ids.field)
            .debug_selector(|| ids.field.into())
            .flex()
            .flex_col()
            .gap(t.space.xs)
            .w_full()
            .max_w(t.layout.timeline_max_width)
            // No channel has loaded: the box takes no text.
            .when(waiting, |d| d.opacity(t.opacity.busy))
            .p(t.space.sm)
            .bg(c.surface_card)
            .border(t.border_width)
            .border_color(c.border_strong)
            .rounded(chat.composer_radius)
            // Files dragged over it: the field is the drop zone, with a
            // dashed accent border and a soft fill.
            .when(dropping.is_some(), |d| {
                d.border(t.stroke)
                    .border_dashed()
                    .border_color(c.accent_text)
                    .bg(c.accent_fill.opacity(t.opacity.drop_fill))
                    .pb(t.space.md + t.space.xxs)
            })
            // A picture or a copied file on the clipboard attaches. Text
            // pastes as usual.
            .capture_action(
                cx.listener(move |this, _: &gpui_kit::base::input::Paste, _, cx| {
                    if this.paste_files(paste_target.clone(), cx) {
                        cx.stop_propagation();
                    }
                }),
            )
            .children(chips)
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(t.space.sm)
                    .pl(t.space.sm)
                    .child(
                        div()
                            .id(ids.text)
                            .flex_1()
                            .min_w_0()
                            .py(t.space.xs)
                            .type_style(t.text.body)
                            .text_color(c.text_primary)
                            .child(input),
                    )
                    .child(send),
            )
            .children(drop_zone)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    let items = text_edit_menu(text_target);
                    this.open_menu(OpenMenu::at(event.position, items), cx);
                    cx.stop_propagation();
                }),
            );
        let tools = div()
            .id(ids.tools)
            .debug_selector(|| ids.tools.into())
            .flex()
            .items_center()
            .gap(t.space.xxs)
            .w_full()
            .max_w(t.layout.timeline_max_width)
            .px(t.space.xs)
            // The row never wraps and never runs past its column.
            .overflow_hidden()
            .child(
                tool(ids.attach, "plus", cx)
                    .debug_selector(|| ids.attach.into())
                    .relative()
                    .child(self.triggers.track(ids.attach))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_add_menu(ids.attach, add_target.clone(), cx)
                    })),
            )
            // Voice: focus the box and ask the system to start dictation.
            // When the system does not, the hint shows (DECISIONS 738).
            .child(
                tool(ids.mic, "voice", cx)
                    .debug_selector(|| ids.mic.into())
                    .tooltip(|_, cx| {
                        gpui_kit::AppContext::new(cx, |_| {
                            crate::ui::convo_sidebar::Tip("Dictate".into())
                        })
                        .into()
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.click_mic(
                            mic_input.clone(),
                            crate::dictation::start_system_dictation(),
                            window,
                            cx,
                        )
                    })),
            )
            // The divider has nothing to divide without the model chips.
            .when(spec.chips || inherited, |d| {
                d.child(
                    div()
                        .debug_selector(|| ids.divider.into())
                        .flex_none()
                        .w(t.border_width)
                        .h(t.icon.size)
                        .mx(t.space.xs + t.space.xxs)
                        .bg(c.border_strong),
                )
            })
            .when(inherited, |d| {
                d.child(
                    text(
                        format!("Same as {}", crate::ui::TUTTI),
                        detail(t),
                        c.text_tertiary,
                    )
                    .flex_none()
                    .whitespace_nowrap()
                    .pr(t.space.xs),
                )
            })
            .children(model_chip)
            .children(effort_chip)
            .child(div().flex_1())
            .when(fit.side, |d| {
                d.children(side.map(|(label, id)| {
                    text(label, detail(t), c.text_tertiary)
                        .flex_none()
                        .whitespace_nowrap()
                        .id(id)
                        .debug_selector(move || id.into())
                }))
            });
        div()
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap(t.space.xs + t.space.xxs)
            .px(t.space.xl)
            .pt(t.space.md)
            .pb(t.space.lg)
            .children(above)
            .child(field)
            .child(tools)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTHS: ToolWidths = ToolWidths {
        fixed: 200.0,
        account: 80.0,
        side: 70.0,
    };

    #[test]
    fn a_wide_row_shows_everything() {
        let fit = fit_tools(400.0, WIDTHS);
        assert!(fit.account && fit.side);
        // It fits exactly.
        let fit = fit_tools(350.0, WIDTHS);
        assert!(fit.account && fit.side);
    }

    #[test]
    fn the_row_drops_runs_on_first() {
        let fit = fit_tools(349.0, WIDTHS);
        assert!(fit.account && !fit.side);
        let fit = fit_tools(280.0, WIDTHS);
        assert!(fit.account && !fit.side);
    }

    #[test]
    fn the_row_drops_the_account_name_second() {
        let fit = fit_tools(279.0, WIDTHS);
        assert!(!fit.account && !fit.side);
        let fit = fit_tools(100.0, WIDTHS);
        assert!(!fit.account && !fit.side);
    }
}
