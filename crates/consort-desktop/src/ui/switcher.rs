//! The search palette (Command K): go to a channel or a task at once, from
//! the view state on this client, and find messages, memory notes and tasks
//! on the hub (`message.search`). The view is in `ui/convo_search.rs`.

use consort_client::{ChannelKind, TaskStatus, ViewState};
use consort_theme::Tokens;
use gpui_kit::{AnyElement, Context, KeyDownEvent, Window};

use consort_client::convo::{SearchHitView, SearchSection, SearchTarget};

use crate::app::ConsortApp;

/// The most rows the switcher shows.
const MAX_ROWS: usize = 10;

/// One result.
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Channel {
        id: String,
        name: String,
        icon: String,
        tutti: bool,
        detail: String,
    },
    Task {
        id: String,
        channel_id: String,
        title: String,
        status: TaskStatus,
        detail: String,
    },
}

/// How well `text` matches `query`: 3 for a start, 2 for a word start, 1
/// anywhere, 0 for no match. Both are lowercase.
fn score(text: &str, query: &str) -> u8 {
    let text = text.to_lowercase();
    if text.starts_with(query) {
        3
    } else if text
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word.starts_with(query))
    {
        2
    } else if text.contains(query) {
        1
    } else {
        0
    }
}

/// How well `text` matches any word of `query`: the best score of the
/// words. One word that matches is enough, so "windows engine" finds the
/// channel whose purpose says "Windows".
fn score_words(text: &str, query: &str) -> u8 {
    let mut best = score(text, query);
    for word in query.split_whitespace().filter(|w| w.chars().count() > 1) {
        best = best.max(score(text, word));
    }
    best
}

/// The results for `query`: channels first, then tasks, best match first.
/// With no query: every channel, then the tasks that need the user.
pub fn search(state: &ViewState, query: &str) -> Vec<Hit> {
    let query = query.trim().to_lowercase();
    let score = score_words;
    let mut channels: Vec<(u8, usize, Hit)> = state
        .channels
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let s = if query.is_empty() {
                1
            } else {
                score(&c.name, &query).max(score(&c.purpose, &query).saturating_sub(1))
            };
            (s > 0).then(|| {
                (
                    s,
                    i,
                    Hit::Channel {
                        id: c.id.clone(),
                        name: c.name.clone(),
                        icon: c.icon.clone(),
                        tutti: c.kind == ChannelKind::Tutti,
                        detail: c.purpose.clone(),
                    },
                )
            })
        })
        .collect();
    let mut tasks: Vec<(u8, usize, Hit)> = state
        .tasks
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let s = if query.is_empty() {
                u8::from(t.status.is_your_turn())
            } else {
                score(&t.title, &query).max(score(&t.channel_name, &query).saturating_sub(1))
            };
            // A resolved task ranks below an open one with the same match.
            let s = if t.status == TaskStatus::Resolved {
                s.min(1)
            } else {
                s
            };
            (s > 0).then(|| {
                (
                    s,
                    i,
                    Hit::Task {
                        id: t.id.clone(),
                        channel_id: t.channel_id.clone(),
                        title: t.title.clone(),
                        status: t.status,
                        detail: t.channel_name.clone(),
                    },
                )
            })
        })
        .collect();
    channels.sort_by_key(|(s, i, _)| (std::cmp::Reverse(*s), *i));
    tasks.sort_by_key(|(s, i, _)| (std::cmp::Reverse(*s), *i));
    let mut out: Vec<Hit> = channels.into_iter().map(|(_, _, h)| h).collect();
    out.truncate(if query.is_empty() {
        MAX_ROWS
    } else {
        MAX_ROWS / 2 + 1
    });
    let room = MAX_ROWS.saturating_sub(out.len());
    out.extend(tasks.into_iter().take(room).map(|(_, _, h)| h));
    out
}

impl ConsortApp {
    pub fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.switcher = Some(0);
        self.convo.search_scope = consort_client::convo::SearchScope::All;
        self.inputs.search.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        let client = self.client.clone();
        self.spawn_command(cx, async move {
            client.run(consort_client::Command::ClearSearch).await
        });
        cx.notify();
    }

    pub fn close_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.switcher = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn switcher_hits(&self, cx: &gpui_kit::App) -> Vec<Hit> {
        if !self.convo.search_scope.shows_go_to() {
            return Vec::new();
        }
        let query = self.inputs.search.read(cx).value().to_string();
        let mut hits = search(&self.state, &query);
        if self.convo.search_scope == consort_client::convo::SearchScope::Tasks {
            hits.retain(|h| matches!(h, Hit::Task { .. }));
        }
        hits
    }

    /// The hub results that the palette shows now, in the order of their
    /// sections: messages, memory, tasks.
    pub(crate) fn hub_hits(&self, cx: &gpui_kit::App) -> Vec<SearchHitView> {
        let query = self.inputs.search.read(cx).value().trim().to_owned();
        let search = &self.state.convo.search;
        if search.query.trim() != query || search.scope != self.convo.search_scope {
            return Vec::new();
        }
        let mut hits = search.hits.clone();
        let rank = |s: SearchSection| match s {
            SearchSection::Messages => 0,
            SearchSection::Memory => 1,
            SearchSection::Tasks => 2,
        };
        hits.sort_by_key(|h| rank(h.section));
        hits
    }

    pub(crate) fn go_to(&mut self, hit: Hit, window: &mut Window, cx: &mut Context<Self>) {
        self.close_switcher(window, cx);
        match hit {
            Hit::Channel { id, .. } => self.open_channel(id, window, cx),
            Hit::Task { id, channel_id, .. } => {
                self.open_channel(channel_id, window, cx);
                self.open_thread(id, window, cx);
            }
        }
    }

    /// Opens a hub result in place: a message in its channel, a reply in
    /// its thread, a task in its thread, a memory note in the Memory tab or in
    /// Settings > Memory.
    pub fn open_hit(&mut self, hit: SearchHitView, window: &mut Window, cx: &mut Context<Self>) {
        self.close_switcher(window, cx);
        let client = self.client.clone();
        match hit.target {
            SearchTarget::Message {
                channel_id,
                message_id,
                thread_root_id,
                task_id,
            } => {
                if self.state.channel(&channel_id).is_some() {
                    self.open_channel(channel_id.clone(), window, cx);
                }
                match (thread_root_id, task_id) {
                    (Some(_), Some(task)) => self.open_thread(task, window, cx),
                    (Some(root), None) => {
                        self.convo.scroll_to = Some(root.clone());
                        let (c2, r2) = (channel_id.clone(), root.clone());
                        let find = client.clone();
                        self.spawn_command(cx, async move {
                            find.run(consort_client::Command::FindMessage {
                                channel_id: c2,
                                message_id: r2,
                            })
                            .await
                        });
                        self.open_message_thread(root, window, cx);
                    }
                    (None, _) => {
                        self.convo.scroll_to = Some(message_id.clone());
                        self.spawn_command(cx, async move {
                            client
                                .run(consort_client::Command::FindMessage {
                                    channel_id,
                                    message_id,
                                })
                                .await
                        });
                    }
                }
            }
            SearchTarget::Task {
                task_id,
                channel_id,
            } => {
                if self.state.channel(&channel_id).is_some() {
                    self.open_channel(channel_id, window, cx);
                }
                if self.state.task(&task_id).is_some() {
                    self.open_thread(task_id, window, cx);
                } else {
                    // An archived task: the Resolved tab lists it with
                    // Restore.
                    self.panel = crate::app::Panel::Tasks;
                    self.tasks_tab = crate::app::TasksTab::Resolved;
                    self.convo.archived_open = true;
                    self.spawn_command(cx, async move {
                        client
                            .run(consort_client::Command::LoadArchivedTasks { more: false })
                            .await
                    });
                }
            }
            // A channel note opens in the Memory tab of the Channel panel,
            // a global note in Settings > Memory (phase 2's views).
            SearchTarget::Memory { channel_id, path } => {
                match channel_id.filter(|id| self.state.channel(id).is_some()) {
                    Some(id) => {
                        let scope = self.memory_scope(&id);
                        if scope == consort_client::memory::GLOBAL {
                            self.open_settings(crate::app::SettingsSection::Memory, cx);
                            self.open_note_in_settings(&scope, &path, cx);
                        } else {
                            self.open_note_in_panel(&scope, &path, window, cx);
                        }
                    }
                    None => {
                        self.open_settings(crate::app::SettingsSection::Memory, cx);
                        self.open_note_in_settings(consort_client::memory::GLOBAL, &path, cx);
                    }
                }
            }
        }
        cx.notify();
    }

    /// "Ask Tutti to search": Tutti gets the question, and the assistant
    /// looks with its own tools.
    pub(crate) fn ask_tutti_to_search(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_switcher(window, cx);
        let Some(tutti) = self.state.tutti().map(|c| c.id.clone()) else {
            return;
        };
        self.open_channel(tutti.clone(), window, cx);
        let client = self.client.clone();
        let body = format!(
            "Look for \u{201C}{query}\u{201D} in my channels, tasks and memory, and tell me what you find."
        );
        self.spawn_command(cx, async move { client.send_message(tutti, body).await });
    }

    pub(crate) fn switcher_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected) = self.switcher else {
            return;
        };
        let local = self.switcher_hits(cx);
        let hub = self.hub_hits(cx);
        let total = local.len() + hub.len();
        match event.keystroke.key.as_str() {
            "escape" => self.close_switcher(window, cx),
            "down" if total > 0 => self.switcher = Some((selected + 1) % total),
            "up" if total > 0 => self.switcher = Some((selected + total - 1) % total),
            "enter" => {
                if let Some(hit) = local.get(selected).cloned() {
                    self.go_to(hit, window, cx);
                } else if let Some(hit) = hub.get(selected.saturating_sub(local.len())).cloned() {
                    self.open_hit(hit, window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub fn render_switcher(&mut self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        crate::ui::convo_search::render_search(self, t, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_and_tasks_are_found_by_word() {
        let state = consort_client::mock::view_state();
        let hits = search(&state, "lucy");
        assert!(matches!(&hits[0], Hit::Channel { name, .. } if name == "miss-lucy-tg"));
        let hits = search(&state, "spam");
        assert!(
            hits.iter()
                .any(|h| matches!(h, Hit::Task { title, .. } if title.contains("spam rules")))
        );
        assert!(search(&state, "zzzz").is_empty());
    }

    #[test]
    fn an_empty_query_lists_channels_then_your_turn() {
        let state = consort_client::mock::view_state();
        let hits = search(&state, "");
        assert!(matches!(&hits[0], Hit::Channel { tutti: true, .. }));
        assert!(hits.len() <= MAX_ROWS);
        assert!(
            hits.iter()
                .filter_map(|h| match h {
                    Hit::Task { status, .. } => Some(*status),
                    _ => None,
                })
                .all(TaskStatus::is_your_turn)
        );
    }

    #[test]
    fn one_word_of_the_query_is_enough() {
        let state = consort_client::mock::view_state();
        let hits = search(&state, "windows engine");
        assert!(
            hits.iter()
                .any(|h| matches!(h, Hit::Channel { name, .. } if name == "sayso")),
            "{hits:?}"
        );
    }

    #[test]
    fn a_start_beats_a_word_start_beats_anywhere() {
        assert_eq!(score("sayso", "say"), 3);
        assert_eq!(score("miss-lucy-tg", "lucy"), 2);
        assert_eq!(score("homelab", "lab"), 1);
        assert_eq!(score("homelab", "x"), 0);
    }
}
