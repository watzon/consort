//! Settings > Dictation (design V5): the engine, the cleanup, the silence
//! limit, the language, the microphone and the shortcut. These are choices for
//! this computer, so they stay here and never go to the hub.

use consort_dictation::{Engine, ModelNeed, Purpose, StopAfter, cleanup_choices, format_size};
use consort_theme::Tokens;
use gpui_kit::{
    AnyElement, Context, Div, InteractiveElement as _, IntoElement as _, ParentElement,
    StatefulInteractiveElement as _, Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::app::ConsortApp;
use crate::dictation::shortcut_caps;
use crate::ui::admin_parts::{column, header, page, row, rows, section_title, select};
use crate::ui::parts::{ButtonKind, button, detail, text};
use crate::ui::settings::switch;

const CLEANUP_SELECT: &str = "dictation-cleanup-model";
const STOP_SELECT: &str = "dictation-stop-after";
const LANGUAGE_SELECT: &str = "dictation-language";
const MIC_SELECT: &str = "dictation-microphone";
/// The option of the default input device.
const DEFAULT_DEVICE: &str = "default";

/// The name of a language code in the list of the final model.
pub fn language_name(code: &str) -> &str {
    match code {
        "en" => "English (US)",
        "bg" => "Bulgarian",
        "cs" => "Czech",
        "da" => "Danish",
        "de" => "German",
        "el" => "Greek",
        "es" => "Spanish",
        "et" => "Estonian",
        "fi" => "Finnish",
        "fr" => "French",
        "hr" => "Croatian",
        "hu" => "Hungarian",
        "it" => "Italian",
        "lt" => "Lithuanian",
        "lv" => "Latvian",
        "mt" => "Maltese",
        "nl" => "Dutch",
        "pl" => "Polish",
        "pt" => "Portuguese",
        "ro" => "Romanian",
        "ru" => "Russian",
        "sk" => "Slovak",
        "sl" => "Slovenian",
        "sv" => "Swedish",
        "uk" => "Ukrainian",
        other => other,
    }
}

impl ConsortApp {
    /// A pick from a select of this page. True when the select is one of them.
    pub(crate) fn pick_dictation_select(
        &mut self,
        select: &str,
        option: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let option = option.to_owned();
        match select {
            CLEANUP_SELECT => self.change_dictation_settings(|s| s.cleanup_model = option, cx),
            LANGUAGE_SELECT => self.change_dictation_settings(|s| s.language = option, cx),
            STOP_SELECT => {
                if let Some(stop) = StopAfter::ALL.iter().find(|s| s.label() == option) {
                    let stop = *stop;
                    self.change_dictation_settings(|s| s.stop_after = stop, cx);
                }
            }
            MIC_SELECT => {
                let device = (option != DEFAULT_DEVICE).then_some(option);
                self.change_dictation_settings(|s| s.microphone = device, cx);
            }
            _ => return false,
        }
        true
    }

    pub fn render_dictation_settings(
        &mut self,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.dictation_page_rendered(window, cx);
        let c = &t.colors;
        let line = text(
            "Speak instead of typing. Your words appear in gray and turn solid when you stop."
                .to_owned(),
            t.text.ui,
            c.text_secondary,
        );
        page("dictation-page", t)
            .child(
                column(t)
                    .gap(t.space.xl)
                    .child(header("Dictation", line, None, t))
                    .child(block("Engine", self.engine_rows(t, cx), t))
                    .child(block(
                        "Fix the text after you stop",
                        self.cleanup_rows(t, cx),
                        t,
                    ))
                    .child(block(
                        "Stop when you stop talking",
                        self.silence_rows(t, cx),
                        t,
                    ))
                    .child(block("Language", self.language_rows(t, cx), t))
                    .child(block("Microphone and shortcut", self.device_rows(t, cx), t)),
            )
            .into_any_element()
    }

    fn engine_rows(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let engine = self.dictation.settings.engine;
        let needs = self.dictation.needs.clone();
        let final_model = needs.iter().find(|n| n.purpose == Purpose::Final);
        let preview = needs.iter().find(|n| n.purpose == Purpose::Preview);
        let on_disk = final_model.is_some_and(|m| m.on_disk);
        // The default model line, until the engine answers.
        let model_line = final_model.map_or_else(
            || "Parakeet TDT v3, 483 MB".to_owned(),
            |m| format!("{}, {}", m.name, format_size(m.size_bytes)),
        );
        let local_button = if on_disk {
            button(
                "dictation-remove-models",
                "Remove",
                ButtonKind::Secondary,
                t,
            )
            .debug_selector(|| "dictation-remove-models".into())
            .on_click(cx.listener(|this, _, _, cx| this.remove_dictation_models(cx)))
        } else {
            let missing: Vec<ModelNeed> = needs.iter().filter(|n| !n.on_disk).cloned().collect();
            // What the click downloads. The engine has not answered in a demo.
            let size: u64 = missing.iter().map(|n| n.size_bytes).sum();
            let label = format!(
                "Download {}",
                format_size(if size == 0 { 483_000_000 } else { size })
            );
            button(
                "dictation-download-models",
                &label,
                ButtonKind::Secondary,
                t,
            )
            .debug_selector(|| "dictation-download-models".into())
            .on_click(cx.listener(move |this, _, window, cx| {
                if missing.is_empty() {
                    this.show_error(
                        "Consort could not reach its speech engine. Install Consort again, then try again."
                            .to_owned(),
                        cx,
                    );
                } else {
                    this.download_dictation_models(missing.clone(), window, cx)
                }
            }))
        };
        // The line of the second model shows where the engine needs one.
        let second = preview
            .filter(|p| final_model.is_none_or(|f| f.id != p.id))
            .map(|p| {
                format!(
                    "The live preview on this Mac uses a second model, {}.",
                    format_size(p.size_bytes)
                )
            });
        let local = row(false, t)
            .items_start()
            .child(radio(
                "dictation-engine-local",
                engine == Engine::Local,
                t,
                cx,
                |this, _, cx| this.change_dictation_settings(|s| s.engine = Engine::Local, cx),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.sm)
                            .child(text("On this device".to_owned(), t.text.ui_strong, c.text_primary))
                            .child(
                                div()
                                    .debug_selector(|| "dictation-recommended".into())
                                    .px(t.space.sm)
                                    .rounded(t.radius.full)
                                    .bg(c.accent_fill.opacity(t.opacity.tint))
                                    .child(text("Recommended".to_owned(), detail(t), c.accent_text)),
                            ),
                    )
                    .child(text(
                        "Private and free. Your voice stays on this computer and works with no connection.".to_owned(),
                        detail(t),
                        c.text_tertiary,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(t.space.sm)
                            .pt(t.space.xs)
                            .child(text(model_line, detail(t), c.text_secondary))
                            .when(on_disk, |d| {
                                d.child(
                                    div()
                                        .size(t.component.chat.mic_dot)
                                        .rounded_full()
                                        .bg(c.status_done),
                                )
                                .child(text("Downloaded".to_owned(), detail(t), c.text_secondary))
                            }),
                    )
                    .children(second.map(|line| text(line, detail(t), c.text_tertiary))),
            )
            .child(local_button);
        let has_key = self.dictation.has_key;
        let key_button = if has_key {
            button(
                "dictation-remove-key",
                "Remove key",
                ButtonKind::Secondary,
                t,
            )
            .debug_selector(|| "dictation-remove-key".into())
            .on_click(cx.listener(|this, _, _, cx| this.remove_deepgram_key(cx)))
        } else {
            button("dictation-add-key", "Add key", ButtonKind::Secondary, t)
                .debug_selector(|| "dictation-add-key".into())
                .on_click(
                    cx.listener(|this, _, window, cx| this.open_deepgram_key_dialog(window, cx)),
                )
        };
        let cloud = row(true, t)
            .items_start()
            .child(radio(
                "dictation-engine-deepgram",
                engine == Engine::Deepgram,
                t,
                cx,
                move |this, window, cx| {
                    if has_key {
                        this.change_dictation_settings(|s| s.engine = Engine::Deepgram, cx);
                    } else {
                        this.open_deepgram_key_dialog(window, cx);
                    }
                },
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(text("Deepgram".to_owned(), t.text.ui_strong, c.text_primary))
                    .child(text(
                        "Deepgram writes the final text only. The live preview stays on this computer.".to_owned(),
                        detail(t),
                        c.text_tertiary,
                    )),
            )
            .child(key_button);
        rows(t).child(local).child(cloud)
    }

    fn cleanup_rows(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let on = self.dictation.settings.cleanup_on;
        let current = self.dictation.settings.cleanup_model.clone();
        let choices = cleanup_choices();
        let label = choices
            .iter()
            .find(|(id, _)| *id == current)
            .map_or_else(|| current.clone(), |(_, label)| label.clone());
        let other = choices
            .iter()
            .find(|(id, _)| *id != current)
            .map_or_else(String::new, |(_, label)| {
                format!("The other choice is {label}.")
            });
        let options = choices
            .iter()
            .map(|(id, label)| (label.clone(), id.clone(), *id == current))
            .collect();
        let select_box = select(
            CLEANUP_SELECT,
            label,
            t.component.dictation_cleanup_select,
            options,
            self,
            t,
            cx,
        );
        rows(t)
            .child(
                labeled(
                    "Clean up the text",
                    "Cleanup runs on this computer when you stop. If it fails, Consort keeps your words as heard.",
                    false,
                    t,
                )
                .child(
                    switch("dictation-cleanup", on, t)
                        .debug_selector(|| "dictation-cleanup".into())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_dictation_settings(|s| s.cleanup_on = !on, cx)
                        })),
                ),
            )
            .child(labeled("Cleanup model", &other, true, t).child(select_box))
    }

    fn silence_rows(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let current = self.dictation.settings.stop_after;
        let options = StopAfter::ALL
            .iter()
            .map(|s| (s.label().to_owned(), s.label().to_owned(), *s == current))
            .collect();
        let select_box = select(
            STOP_SELECT,
            current.label().to_owned(),
            t.component.dictation_select,
            options,
            self,
            t,
            cx,
        );
        rows(t).child(
            labeled(
                "Stop after silence",
                "Works with both engines and runs on this computer. It never sends your message.",
                true,
                t,
            )
            .child(select_box),
        )
    }

    fn language_rows(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let current = self.dictation.settings.language.clone();
        let mut languages = self.dictation.languages.clone();
        if !languages.contains(&current) {
            languages.insert(0, current.clone());
        }
        let options = languages
            .iter()
            .map(|code| {
                (
                    language_name(code).to_owned(),
                    code.clone(),
                    *code == current,
                )
            })
            .collect();
        let select_box = select(
            LANGUAGE_SELECT,
            language_name(&current).to_owned(),
            t.component.dictation_select,
            options,
            self,
            t,
            cx,
        );
        rows(t).child(
            labeled(
                "Spoken language",
                "Choose the language you speak. Parakeet TDT v3 hears 25 European languages.",
                true,
                t,
            )
            .child(select_box),
        )
    }

    fn device_rows(&mut self, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let chosen = self.dictation.settings.microphone.clone();
        let devices = self.dictation.devices.clone();
        let default_name = devices
            .iter()
            .find(|d| d.is_default)
            .map_or("Default microphone", |d| d.name.as_str())
            .to_owned();
        let label = chosen
            .as_ref()
            .and_then(|id| devices.iter().find(|d| d.id == *id))
            .map_or(default_name.clone(), |d| d.name.clone());
        let mut options = vec![(default_name, DEFAULT_DEVICE.to_owned(), chosen.is_none())];
        options.extend(devices.iter().filter(|d| !d.is_default).map(|d| {
            (
                d.name.clone(),
                d.id.clone(),
                chosen.as_deref() == Some(d.id.as_str()),
            )
        }));
        let select_box = select(
            MIC_SELECT,
            label,
            t.component.dictation_select,
            options,
            self,
            t,
            cx,
        );
        let capturing = self.dictation.capturing_shortcut;
        let caps = div().flex().items_center().gap(t.space.xs).children(
            shortcut_caps(&self.dictation.settings.shortcut)
                .into_iter()
                .map(|cap| {
                    div()
                        .px(t.space.sm)
                        .py(t.space.xxs)
                        .rounded(t.radius.sm)
                        .border(t.border_width)
                        .border_color(c.border_strong)
                        .font_family(t.font_mono.clone())
                        .child(text(cap, t.text.mono, c.text_secondary))
                }),
        );
        let change = button(
            "dictation-change-shortcut",
            if capturing { "Press keys" } else { "Change" },
            ButtonKind::Secondary,
            t,
        )
        .debug_selector(|| "dictation-change-shortcut".into())
        .on_click(cx.listener(|this, _, _, cx| {
            this.dictation.capturing_shortcut = !this.dictation.capturing_shortcut;
            cx.notify();
        }));
        rows(t)
            .child(
                labeled(
                    "Input device",
                    "Consort listens with this microphone when you dictate.",
                    false,
                    t,
                )
                .child(select_box),
            )
            .child(
                labeled(
                    "Start and stop dictation",
                    "Press it in any message box. Esc cancels while you dictate.",
                    true,
                    t,
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.md)
                        .child(caps)
                        .child(change),
                ),
            )
    }
}

fn block(title: &str, body: Div, t: &Tokens) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(t.space.sm)
        .child(section_title(title, t))
        .child(body)
}

/// A row with a title and a line at the left.
fn labeled(title: &str, hint: &str, last: bool, t: &Tokens) -> Div {
    let c = &t.colors;
    row(last, t).child(
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(text(title.to_owned(), t.text.ui_strong, c.text_primary))
            .child(text(hint.to_owned(), detail(t), c.text_tertiary)),
    )
}

/// A radio button: a ring, with a dot when it is on.
fn radio(
    id: &'static str,
    on: bool,
    t: &Tokens,
    cx: &mut Context<ConsortApp>,
    pick: impl Fn(&mut ConsortApp, &mut Window, &mut Context<ConsortApp>) + 'static,
) -> gpui_kit::Stateful<Div> {
    let c = &t.colors;
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(t.icon.size)
        .mt(t.space.xxs)
        .rounded_full()
        .border(t.border_width)
        .border_color(if on { c.accent_text } else { c.border_strong })
        .cursor_pointer()
        .when(on, |d| {
            d.child(
                div()
                    .size(t.component.chat.mic_dot)
                    .rounded_full()
                    .bg(c.accent_text),
            )
        })
        .on_click(cx.listener(move |this, _, window, cx| pick(this, window, cx)))
}
