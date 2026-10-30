//! The Add worker dialog, in three steps (Paper `E3F-0` and `EI4-0`):
//!
//! 1. **System.** Choose macOS, Windows or Linux. Each choice says how the
//!    worker runs there.
//! 2. **Install.** One command that downloads the worker, installs it and
//!    pairs it with the code. The code expires, and New code makes another.
//! 3. **Connect.** "<name> is online", a short checklist and Add an account.
//!
//! The dialog moves to step 3 by itself when the worker that it paired is
//! online. The label of step 1 turns into the chosen system after step 1.

use consort_client::{Os, PairingCodeView, WorkerView, update::worker_install_command};
use consort_theme::{Tokens, TypeExt as _};
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Div, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _,
};

use crate::{
    app::{ConsortApp, Dialog},
    ui::parts::{ButtonKind, button, focusable, icon, selectable, status_glyph, text},
};

/// The step that the user chose. Step 3 is not here: the dialog shows it
/// when the worker is online.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AddWorkerStep {
    #[default]
    System,
    Install,
}

/// The system of this computer, which the dialog preselects.
pub fn default_os() -> Os {
    match std::env::consts::OS {
        "macos" => Os::MacOs,
        "windows" => Os::Windows,
        _ => Os::Linux,
    }
}

/// The name and the description on a card of step 1.
pub fn system_card(os: Os) -> (&'static str, &'static str) {
    match os {
        Os::MacOs => ("macOS", "Apple silicon or Intel. Runs as a launchd agent."),
        Os::Windows => (
            "Windows",
            "Windows 10 or 11, x64. Runs as a Task Scheduler task.",
        ),
        Os::Linux => ("Linux", "x64 or arm64. Runs as a systemd user unit."),
    }
}

/// The program that the user runs the command in.
pub fn shell_name(os: Os) -> &'static str {
    match os {
        Os::Windows => "PowerShell",
        Os::MacOs | Os::Linux => "a terminal",
    }
}

/// What the command does, in the words of the system.
pub fn what_the_command_does(os: Os) -> &'static str {
    match os {
        Os::MacOs => {
            "It downloads the worker, installs it as a launchd agent for your user, and pairs it with the hub. It does not need admin rights."
        }
        Os::Windows => {
            "It downloads the worker, installs it as a Task Scheduler task for your user, and pairs it with the hub. It does not need admin rights."
        }
        Os::Linux => {
            "It downloads the worker, installs it as a systemd user unit for your user, and pairs it with the hub. It does not need admin rights."
        }
    }
}

/// The time left of a pairing code as `m:ss`, from the end time and the time
/// now, both in Unix milliseconds.
pub fn time_left(expires_at: i64, now: i64) -> String {
    let left = (expires_at - now).max(0) / 1000;
    format!("{}:{:02}", left / 60, left % 60)
}

/// The command for step 2, or `None` while the hub has made no code.
pub fn install_command(os: Os, pairing: Option<&PairingCodeView>) -> Option<String> {
    let pairing = pairing.filter(|p| !p.code.is_empty() && !p.hub_url.is_empty())?;
    Some(worker_install_command(os, &pairing.hub_url, &pairing.code))
}

/// The checklist of step 3.
pub fn connect_checklist(worker: &WorkerView) -> Vec<String> {
    let found: Vec<&str> = worker
        .tools
        .iter()
        .filter(|t| {
            matches!(
                t.state,
                consort_client::ToolState::Ready | consort_client::ToolState::UpdateAvailable
            )
        })
        .map(|t| t.name.as_str())
        .collect();
    let tools = if found.is_empty() {
        "The worker downloads Claude Code and Codex when you add an account.".to_owned()
    } else {
        let names = match found.as_slice() {
            [one] => (*one).to_owned(),
            [first, second] => format!("{first} and {second}"),
            [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
            [] => String::new(),
        };
        match worker.accounts.len() {
            0 if found.len() == 1 => format!("Found {names}. It has no account yet."),
            0 if found.len() == 2 => format!("Found {names}. Neither has an account yet."),
            0 => format!("Found {names}. None has an account yet."),
            1 => format!("Found {names}. One account is set up."),
            n => format!("Found {names}. {n} accounts are set up."),
        }
    };
    vec![
        "Installed and paired with the hub".to_owned(),
        format!("Starts when you sign in to {}", worker.os.label()),
        tools,
    ]
}

/// Where a step of the line at the top is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Done,
    Current,
    Later,
}

fn step_mark(index: usize, current: usize) -> Mark {
    match index.cmp(&current) {
        std::cmp::Ordering::Less => Mark::Done,
        std::cmp::Ordering::Equal => Mark::Current,
        std::cmp::Ordering::Greater => Mark::Later,
    }
}

impl ConsortApp {
    /// The line with the three steps. After step 1, its first label is the
    /// chosen system.
    fn worker_steps(&self, current: usize, t: &Tokens) -> Div {
        let c = &t.colors;
        let labels = [
            if current == 0 {
                "System"
            } else {
                system_card(self.add_worker_os).0
            },
            "Install",
            "Connect",
        ];
        let mut row = div().flex().items_center().gap(t.space.sm + t.space.xxs);
        for (i, label) in labels.iter().enumerate() {
            if i > 0 {
                row = row.child(div().flex_1().h(t.border_width).bg(c.border_strong));
            }
            let mark = step_mark(i, current);
            let circle = div()
                .size(t.space.lg + t.space.sm - t.space.xxs - t.space.xxs)
                .flex_none()
                .rounded(t.radius.full)
                .flex()
                .items_center()
                .justify_center();
            let circle = match mark {
                Mark::Done => circle.bg(c.tint_done).child(icon(
                    "check",
                    t.icon.dense - t.space.xxs / 2.0,
                    c.status_done,
                )),
                Mark::Current => circle
                    .bg(c.surface_selected)
                    .border(t.border_width)
                    .border_color(c.text_secondary)
                    .child(
                        div()
                            .type_style(t.text.label)
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(c.text_primary)
                            .child((i + 1).to_string()),
                    ),
                Mark::Later => circle
                    .border(t.border_width)
                    .border_color(c.border_strong)
                    .child(
                        div()
                            .type_style(t.text.label)
                            .text_color(c.text_tertiary)
                            .child((i + 1).to_string()),
                    ),
            };
            let name = match mark {
                Mark::Done => text(*label, t.text.ui, c.text_secondary),
                Mark::Current => text(*label, t.text.ui_strong, c.text_primary),
                Mark::Later => text(*label, t.text.ui, c.text_tertiary),
            };
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .child(circle)
                    .child(name),
            );
        }
        row
    }

    /// One card of step 1.
    fn system_option(&self, os: Os, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let c = &t.colors;
        let chosen = self.add_worker_os == os;
        let (name, how) = system_card(os);
        let selector = format!("add-worker-os-{}", name.to_lowercase());
        let radius = t.icon.dense;
        let radio = div()
            .size(radius)
            .flex_none()
            .rounded(t.radius.full)
            .border(t.border_width)
            .border_color(if chosen {
                c.accent_text
            } else {
                c.border_strong
            })
            .flex()
            .items_center()
            .justify_center()
            .when(chosen, |d| {
                d.child(
                    div()
                        .size(t.space.sm)
                        .rounded(t.radius.full)
                        .bg(c.accent_text),
                )
            });
        focusable(
            div()
                .id(SharedString::from(format!("worker-os-{name}")))
                .debug_selector(move || selector),
            t,
        )
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(t.space.sm)
        .p(t.space.md)
        .rounded(t.radius.lg)
        .border(t.border_width)
        .border_color(if chosen {
            c.accent_text
        } else {
            c.border_subtle
        })
        .when(chosen, |d| d.bg(c.tint_accent))
        .cursor_pointer()
        .hover(|s| s.bg(c.hover_raised))
        .child(
            div()
                .flex()
                .items_center()
                .gap(t.space.sm)
                .child(radio)
                .child(text(name, t.text.ui_strong, c.text_primary)),
        )
        .child(text(how, t.text.label, c.text_secondary))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.add_worker_os = os;
            cx.notify();
        }))
        .into_any_element()
    }

    /// The command box of step 2, with its Copy button.
    fn command_box(&self, command: String, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let c = &t.colors;
        let copy = command.clone();
        div()
            .flex()
            .items_start()
            .gap(t.space.sm + t.space.xxs)
            .py(t.space.md)
            .pl(t.space.md + t.space.xxs)
            .pr(t.space.sm + t.space.xxs)
            .bg(c.surface_canvas)
            .border(t.border_width)
            .border_color(c.border_strong)
            .rounded(t.radius.lg)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(t.font_mono.clone())
                    .type_style(t.text.mono)
                    .text_color(c.text_primary)
                    .child(selectable(
                        "install-command",
                        command,
                        t.text.mono,
                        c.text_primary,
                    )),
            )
            .child(
                focusable(
                    div()
                        .id("copy-install-command")
                        .debug_selector(|| "copy-install-command".into()),
                    t,
                )
                .flex()
                .flex_none()
                .items_center()
                .gap(t.space.sm - t.space.xxs)
                .px(t.space.sm + t.space.xxs)
                .py(t.space.xs)
                .rounded(t.radius.md)
                .bg(c.surface_raised)
                .border(t.border_width)
                .border_color(c.border_strong)
                .type_style(t.text.ui_strong)
                .text_color(c.text_primary)
                .cursor_pointer()
                .hover(|s| s.bg(c.hover_raised))
                .child(icon("copy", t.icon.dense, c.text_secondary))
                .child("Copy")
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                    this.show_notice("Copied the command.".into(), cx);
                })),
            )
    }

    /// The whole dialog: its title, its description, its width, its body and
    /// its buttons.
    pub fn add_worker_dialog(
        &mut self,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> (String, String, gpui_kit::Pixels, Div, Div) {
        let c = &t.colors;
        let pairing = self.state.worker_pairing.clone();
        let added = self
            .added_worker
            .as_ref()
            .and_then(|id| self.state.workers.iter().find(|w| &w.id == id))
            .cloned();
        let online = added.as_ref().filter(|w| w.online);
        let os = self.add_worker_os;
        let current = if online.is_some() {
            2
        } else if self.add_worker_step == AddWorkerStep::Install {
            1
        } else {
            0
        };
        let steps = self.worker_steps(current, t);
        let footer = || {
            div()
                .flex()
                .items_center()
                .gap(t.space.sm)
                .pt(t.space.md + t.space.xxs)
                .border_t(t.border_width)
                .border_color(c.border_subtle)
        };
        let note = |content: String| {
            text(content, t.text.label, c.text_tertiary)
                .font_weight(t.text.ui.weight)
                .flex_1()
                .min_w_0()
        };

        let (body, actions) = if let Some(worker) = online {
            let checks = connect_checklist(worker);
            let card = div()
                .flex()
                .items_center()
                .gap(t.space.md + t.space.xxs)
                .p(t.space.lg)
                .bg(c.surface_card)
                .rounded(t.radius.lg)
                .child(
                    div()
                        .size(t.space.xl + t.space.lg)
                        .flex_none()
                        .rounded(t.radius.full)
                        .bg(c.tint_done)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .size(t.space.sm + t.space.xxs)
                                .rounded(t.radius.full)
                                .bg(c.status_done),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(text(
                            format!("{} is online", worker.name),
                            t.text.title,
                            c.text_primary,
                        ))
                        .child(text(
                            worker.status_line.clone(),
                            t.text.ui,
                            c.text_secondary,
                        )),
                );
            let mut list = div().flex().flex_col().gap(t.space.sm).px(t.space.xs);
            for check in checks {
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(t.space.sm + t.space.xxs)
                        .child(icon("check", t.icon.dense, c.status_done))
                        .child(text(check, t.text.ui, c.text_secondary).flex_1().min_w_0()),
                );
            }
            let id = worker.id.clone();
            let actions = footer()
                .child(note(format!(
                    "Next, sign in to a harness on {}.",
                    worker.name
                )))
                .child(
                    button("add-worker-done", "Done", ButtonKind::Secondary, t)
                        .debug_selector(|| "add-worker-done".into())
                        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
                )
                .child(
                    button(
                        "add-worker-add-account",
                        "Add an account",
                        ButtonKind::Primary,
                        t,
                    )
                    .debug_selector(|| "add-worker-add-account".into())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_dialog(window, cx);
                        this.open_dialog(Dialog::AddAccount(id.clone()), window, cx);
                    })),
                );
            (
                div()
                    .flex()
                    .flex_col()
                    .gap(t.space.md + t.space.xxs)
                    .child(card)
                    .child(list),
                actions,
            )
        } else if current == 1 {
            let expires = pairing
                .as_ref()
                .map(|p| time_left(p.expires_at, consort_client::format::now_ms()))
                .unwrap_or_default();
            let command = match install_command(os, pairing.as_ref()) {
                Some(command) => self.command_box(command, t, cx).into_any_element(),
                None => text("Making a code", t.text.ui, c.text_secondary).into_any_element(),
            };
            let body = div()
                .flex()
                .flex_col()
                .gap(t.space.sm + t.space.xxs)
                .child(text(
                    format!("Run this in {} on the machine", shell_name(os)),
                    t.text.ui_strong,
                    c.text_primary,
                ))
                .child(command)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(t.space.xs)
                        .child(
                            text(what_the_command_does(os), t.text.label, c.text_tertiary)
                                .font_weight(t.text.ui.weight),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(t.space.sm)
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .items_baseline()
                                        .type_style(t.text.label)
                                        .font_weight(t.text.ui.weight)
                                        .text_color(c.text_tertiary)
                                        .child("The code works once. It expires in\u{a0}")
                                        .child(
                                            div()
                                                .font_family(t.font_mono.clone())
                                                .text_color(c.text_secondary)
                                                .child(expires),
                                        )
                                        .child("."),
                                )
                                .child(
                                    focusable(
                                        div()
                                            .id("add-worker-new-code")
                                            .debug_selector(|| "add-worker-new-code".into()),
                                        t,
                                    )
                                    .type_style(t.text.label)
                                    .font_weight(t.text.ui_strong.weight)
                                    .text_color(c.text_secondary)
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(c.text_primary))
                                    .child("New code")
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            let client = this.client.clone();
                                            this.spawn_command(cx, async move {
                                                client.worker_pairing_code().await
                                            });
                                        },
                                    )),
                                ),
                        ),
                );
            // The worker may be paired but not online yet.
            let (glyph, wait): (AnyElement, String) = match &added {
                Some(w) => (
                    status_glyph(
                        "waiting-worker",
                        consort_client::TaskStatus::Working,
                        t.icon.size,
                        t,
                        cx,
                    ),
                    format!("{} paired. Waiting for it to connect", w.name),
                ),
                None => (
                    status_glyph(
                        "waiting-worker",
                        consort_client::TaskStatus::Working,
                        t.icon.size,
                        t,
                        cx,
                    ),
                    "Waiting for the worker to connect".to_owned(),
                ),
            };
            let actions = footer()
                .child(glyph)
                .child(text(wait, t.text.ui, c.text_secondary).flex_1().min_w_0())
                .child(
                    button("add-worker-back", "Back", ButtonKind::Secondary, t)
                        .debug_selector(|| "add-worker-back".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.add_worker_step = AddWorkerStep::System;
                            cx.notify();
                        })),
                )
                .child(
                    button("dialog-cancel", "Cancel", ButtonKind::Secondary, t)
                        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
                );
            (body, actions)
        } else {
            let body = div()
                .flex()
                .flex_col()
                .gap(t.space.sm + t.space.xxs)
                .child(text(
                    "What system does the machine run?",
                    t.text.ui_strong,
                    c.text_primary,
                ))
                .child(
                    div()
                        .flex()
                        .gap(t.space.md)
                        .child(self.system_option(Os::MacOs, t, cx))
                        .child(self.system_option(Os::Windows, t, cx))
                        .child(self.system_option(Os::Linux, t, cx)),
                );
            let actions = footer()
                .child(note("The worker starts when you sign in.".to_owned()))
                .child(
                    button("dialog-cancel", "Cancel", ButtonKind::Secondary, t)
                        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
                )
                .child(
                    button("add-worker-continue", "Continue", ButtonKind::Primary, t)
                        .debug_selector(|| "add-worker-continue".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.add_worker_step = AddWorkerStep::Install;
                            cx.notify();
                        })),
                );
            (body, actions)
        };

        (
            "Add a worker".to_owned(),
            "A worker runs tasks on its own machine. It connects out to the hub, so the machine needs no open ports.".to_owned(),
            t.layout.dialog_wide,
            div()
                .flex()
                .flex_col()
                .gap(t.space.lg + t.space.xs)
                .child(steps)
                .child(body),
            actions,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_client::{ToolState, ToolView};

    fn tool(name: &str, state: ToolState) -> ToolView {
        ToolView {
            id: name.to_lowercase().replace(' ', "-"),
            name: name.to_owned(),
            state,
            ..ToolView::default()
        }
    }

    fn worker(tools: Vec<ToolView>, accounts: usize) -> WorkerView {
        WorkerView {
            id: "wrk_1".into(),
            name: "bigboi".into(),
            os: Os::Windows,
            online: true,
            tools,
            accounts: (0..accounts)
                .map(|_| consort_client::HarnessAccountView::default())
                .collect(),
            ..WorkerView::default()
        }
    }

    #[test]
    fn each_system_says_how_the_worker_runs_there() {
        assert_eq!(
            system_card(Os::MacOs),
            ("macOS", "Apple silicon or Intel. Runs as a launchd agent.")
        );
        assert_eq!(
            system_card(Os::Windows),
            (
                "Windows",
                "Windows 10 or 11, x64. Runs as a Task Scheduler task."
            )
        );
        assert_eq!(
            system_card(Os::Linux),
            ("Linux", "x64 or arm64. Runs as a systemd user unit.")
        );
    }

    #[test]
    fn the_command_needs_a_code_and_a_hub_address() {
        assert_eq!(install_command(Os::Linux, None), None);
        let no_hub = PairingCodeView {
            code: "KQ7M-4XPD".into(),
            hub_url: String::new(),
            expires_at: 0,
        };
        assert_eq!(install_command(Os::Linux, Some(&no_hub)), None);
        let pairing = PairingCodeView {
            code: "KQ7M-4XPD".into(),
            hub_url: "https://hub.example.com".into(),
            expires_at: 0,
        };
        let linux = install_command(Os::Linux, Some(&pairing)).unwrap();
        assert!(linux.starts_with("curl -fsSL https://raw.githubusercontent.com/"));
        assert!(linux.ends_with("--hub https://hub.example.com --code KQ7M-4XPD"));
        let windows = install_command(Os::Windows, Some(&pairing)).unwrap();
        assert!(windows.starts_with("& ([scriptblock]::Create((irm https://"));
        assert!(windows.ends_with("-Hub https://hub.example.com -Code KQ7M-4XPD"));
        assert_eq!(
            install_command(Os::MacOs, Some(&pairing)).unwrap(),
            linux,
            "macOS and Linux share the script"
        );
    }

    #[test]
    fn the_code_time_counts_down_and_stops_at_zero() {
        assert_eq!(time_left(10 * 60_000, 0), "10:00");
        assert_eq!(time_left(10 * 60_000, 18_000), "9:42");
        assert_eq!(time_left(1_000, 5_000), "0:00");
        assert_eq!(time_left(61_000, 0), "1:01");
    }

    #[test]
    fn the_shell_and_the_service_match_the_system() {
        assert_eq!(shell_name(Os::Windows), "PowerShell");
        assert_eq!(shell_name(Os::MacOs), "a terminal");
        assert!(what_the_command_does(Os::MacOs).contains("launchd agent"));
        assert!(what_the_command_does(Os::Windows).contains("Task Scheduler task"));
        assert!(what_the_command_does(Os::Linux).contains("systemd user unit"));
    }

    #[test]
    fn the_checklist_names_the_tools_and_the_accounts() {
        let two = worker(
            vec![
                tool("Claude Code", ToolState::Ready),
                tool("Codex", ToolState::Ready),
                tool("Gemini CLI", ToolState::Missing),
            ],
            0,
        );
        assert_eq!(
            connect_checklist(&two),
            [
                "Installed and paired with the hub",
                "Starts when you sign in to Windows",
                "Found Claude Code and Codex. Neither has an account yet.",
            ]
        );
        let one = worker(vec![tool("Claude Code", ToolState::UpdateAvailable)], 0);
        assert_eq!(
            connect_checklist(&one)[2],
            "Found Claude Code. It has no account yet."
        );
        let with_accounts = worker(vec![tool("Claude Code", ToolState::Ready)], 1);
        assert_eq!(
            connect_checklist(&with_accounts)[2],
            "Found Claude Code. One account is set up."
        );
        let none = worker(vec![tool("Claude Code", ToolState::Missing)], 0);
        assert_eq!(
            connect_checklist(&none)[2],
            "The worker downloads Claude Code and Codex when you add an account."
        );
    }

    #[test]
    fn the_marks_follow_the_current_step() {
        assert!(step_mark(0, 1) == Mark::Done);
        assert!(step_mark(1, 1) == Mark::Current);
        assert!(step_mark(2, 1) == Mark::Later);
    }
}
