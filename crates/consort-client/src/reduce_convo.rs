//! The phase 4 part of the reducer: message threads, search results, the
//! open task item, the archived tasks and the repos. See [`crate::convo`].

use consort_proto::model::{self, MessageKind};
use consort_proto::tools::SearchHit;

use crate::cache::Cache;
use crate::convo::{
    ArchivedTaskView, ArchivedTasksView, ConvoView, MessageThreadView, SearchHitView,
    SearchSection, SearchTarget, SearchView,
};
use crate::format;
use crate::reduce::{artifact_tasks, attachment_views, shown_name, speaker, task_status};
use crate::view::{MessageView, RepoView, Speaker, TUTTI, ViewState};

/// The git remote URL in one form, so two ways to write it match: no scheme,
/// no user, no `.git`, `host/path`, lower case. The hub uses the same rule.
pub fn repo_url_key(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url,
    };
    let rest = rest.rsplit_once('@').map_or(rest, |(_, after)| after);
    rest.replacen(':', "/", 1).to_lowercase()
}

/// "Checked out on bigboi and MacBook Pro", or "Not checked out yet. The
/// first task clones it."
fn checkouts_label(cache: &Cache, repo: &model::Repo) -> String {
    let key = repo_url_key(&repo.url);
    let mut names: Vec<&str> = cache
        .workers
        .values()
        .filter(|w| w.revoked_at.is_none())
        .filter(|w| {
            w.inventory.as_ref().is_some_and(|inv| {
                inv.checkouts
                    .iter()
                    .any(|c| repo_url_key(&c.repo_url) == key)
            })
        })
        .map(|w| w.name.as_str())
        .collect();
    names.sort_unstable();
    if names.is_empty() {
        "Not checked out yet. The first task clones it.".to_owned()
    } else {
        format!("Checked out on {}", crate::convo::join_names(&names))
    }
}

fn repo_view(cache: &Cache, repo: &model::Repo) -> RepoView {
    RepoView {
        id: repo.id.to_string(),
        name: repo.name.clone(),
        checkouts: checkouts_label(cache, repo),
        url: repo.url.clone(),
    }
}

/// The repos of a channel, in the channel's order.
pub(crate) fn channel_repos(cache: &Cache, channel: &model::Channel) -> Vec<RepoView> {
    channel
        .repo_ids
        .iter()
        .filter_map(|id| cache.repos.get(&id.to_string()))
        .map(|r| repo_view(cache, r))
        .collect()
}

/// "Last reply 9:31 AM" for a message with replies.
pub(crate) fn last_reply(m: &model::Message, now: i64) -> Option<String> {
    if m.reply_count == 0 {
        return None;
    }
    m.last_reply_at
        .map(|at| crate::convo::last_reply_label(at.as_millis(), now))
}

fn message_view(
    cache: &Cache,
    m: &model::Message,
    artifacts: &std::collections::BTreeMap<String, crate::files::ArtifactTask>,
    now: i64,
) -> MessageView {
    let at = m.created_at.as_millis();
    MessageView {
        id: m.id.to_string(),
        channel_id: m.channel_id.to_string(),
        speaker: speaker(m),
        time_label: format::time_label(at),
        created_at: at,
        title: None,
        body: if m.kind == MessageKind::Notice {
            crate::reduce::notice_parts(&m.body).1
        } else {
            m.body.clone()
        },
        items: Vec::new(),
        edited: m.edited_at.is_some(),
        reply_count: m.reply_count,
        pending: false,
        attachments: attachment_views(&m.attachments, artifacts, cache),
        reactions: crate::convo::reaction_views(&m.reactions),
        last_reply_label: last_reply(m, now),
    }
}

fn thread_view(
    cache: &Cache,
    root_id: &str,
    artifacts: &std::collections::BTreeMap<String, crate::files::ArtifactTask>,
    now: i64,
) -> Option<MessageThreadView> {
    let root = cache.messages.get(root_id)?;
    let mut replies: Vec<&model::Message> = cache
        .messages
        .values()
        .filter(|m| m.thread_root_id == Some(root.id) && m.deleted_at.is_none())
        .collect();
    replies.sort_by_key(|m| (m.created_at, m.id));
    let mut reply_views: Vec<MessageView> = replies
        .into_iter()
        .map(|m| message_view(cache, m, artifacts, now))
        .collect();
    for p in cache
        .pending
        .iter()
        .filter(|p| p.thread_root_id.as_deref() == Some(root_id))
    {
        reply_views.push(MessageView {
            id: format!("pending-{}", p.client_message_id),
            channel_id: p.channel_id.clone(),
            speaker: Speaker::You,
            time_label: format::time_label(p.created_at),
            created_at: p.created_at,
            body: p.body.clone(),
            pending: true,
            attachments: attachment_views(&p.attachments, artifacts, cache),
            ..MessageView::default()
        });
    }
    let channel_name = cache
        .channels
        .get(&root.channel_id.to_string())
        .map(shown_name)
        .unwrap_or_default();
    let count = root
        .reply_count
        .max(u32::try_from(reply_views.len()).unwrap_or(u32::MAX));
    Some(MessageThreadView {
        root_id: root_id.to_owned(),
        channel_id: root.channel_id.to_string(),
        subtitle: format!(
            "In {channel_name}, {}",
            crate::convo::reply_count_label(count)
        ),
        root: message_view(cache, root, artifacts, now),
        replies: reply_views,
        loaded: true,
    })
}

fn channel_bits(cache: &Cache, id: &consort_proto::ids::ChannelId) -> (String, String, bool) {
    match cache.channels.get(&id.to_string()) {
        Some(c) if c.kind == model::ChannelKind::Tutti => {
            (TUTTI.to_owned(), "brace".to_owned(), true)
        }
        Some(c) => (
            shown_name(c),
            c.icon.clone().unwrap_or_else(|| "hash".to_owned()),
            false,
        ),
        None => ("A channel".to_owned(), "hash".to_owned(), false),
    }
}

/// "Today 9:14 AM", "Yesterday 6:28 PM", "October 24".
fn when_label(at: i64, now: i64) -> String {
    let day = format::day_label(at, now);
    match day.as_str() {
        "Today" | "Yesterday" => format!("{day} {}", format::time_label(at)),
        _ => day,
    }
}

fn hit_view(cache: &Cache, hit: &SearchHit, now: i64) -> SearchHitView {
    match hit {
        SearchHit::Message {
            message_id,
            channel_id,
            thread_root_id,
            snippet,
            created_at,
        } => {
            let (name, icon, tutti) = channel_bits(cache, channel_id);
            let task_id = thread_root_id.and_then(|root| {
                cache
                    .tasks
                    .values()
                    .find(|t| t.card_message_id == root)
                    .map(|t| t.id.to_string())
            });
            SearchHitView {
                section: SearchSection::Messages,
                target: SearchTarget::Message {
                    channel_id: channel_id.to_string(),
                    message_id: message_id.to_string(),
                    thread_root_id: thread_root_id.map(|r| r.to_string()),
                    task_id,
                },
                title: name,
                title_note: thread_root_id.map(|_| "in a thread".to_owned()),
                icon: Some(icon),
                tutti,
                status: None,
                snippet: snippet.clone(),
                detail: when_label(created_at.as_millis(), now),
            }
        }
        SearchHit::MemoryNote {
            scope,
            path,
            snippet,
            updated_at,
        } => SearchHitView {
            section: SearchSection::Memory,
            target: SearchTarget::Memory {
                channel_id: match scope {
                    model::MemoryScope::Channel(id) => Some(id.to_string()),
                    model::MemoryScope::Global => None,
                },
                path: path.clone(),
            },
            title: path.clone(),
            title_note: None,
            icon: Some("notebook".to_owned()),
            tutti: false,
            status: None,
            snippet: snippet.clone(),
            detail: updated_at
                .map(|at| {
                    let day = format::day_label(at.as_millis(), now);
                    match day.as_str() {
                        "Today" | "Yesterday" => format!("Changed {}", day.to_lowercase()),
                        _ => format!("Changed {day}"),
                    }
                })
                .unwrap_or_default(),
        },
        SearchHit::Task {
            task_id,
            channel_id,
            title,
            status,
            updated_at,
        } => {
            let (name, _, _) = channel_bits(cache, channel_id);
            let status = task_status(*status);
            let when = updated_at
                .map(|at| format::day_label(at.as_millis(), now).to_lowercase())
                .map(|d| match d.as_str() {
                    "today" | "yesterday" => d,
                    _ => {
                        let mut c = d.chars();
                        match c.next() {
                            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                            None => d,
                        }
                    }
                });
            let state = status.label().to_lowercase();
            SearchHitView {
                section: SearchSection::Tasks,
                target: SearchTarget::Task {
                    task_id: task_id.to_string(),
                    channel_id: channel_id.to_string(),
                },
                title: title.clone(),
                title_note: None,
                icon: None,
                tutti: false,
                status: Some(status),
                snippet: String::new(),
                detail: match when {
                    Some(when) => format!("{name}, {state} {when}"),
                    None => format!("{name}, {state}"),
                },
            }
        }
    }
}

fn archived_view(cache: &Cache, now: i64) -> ArchivedTasksView {
    let archived = &cache.convo.archived;
    ArchivedTasksView {
        load: archived.load.clone(),
        has_more: archived.has_more,
        tasks: archived
            .tasks
            // A restore takes the task out of the list at once.
            .iter()
            .filter(|t| {
                cache
                    .tasks
                    .get(&t.id.to_string())
                    .is_none_or(|c| c.archived_at.is_some())
            })
            .map(|t| ArchivedTaskView {
                id: t.id.to_string(),
                title: t.title.clone(),
                channel_name: cache
                    .channels
                    .get(&t.channel_id.to_string())
                    .map(shown_name)
                    .unwrap_or_default(),
                status: task_status(t.status),
                archived_label: t
                    .archived_at
                    .map(|at| {
                        format!(
                            "Archived {}",
                            format::day_label(at.as_millis(), now).to_lowercase_day()
                        )
                    })
                    .unwrap_or_default(),
            })
            .collect(),
    }
}

trait LowerDay {
    fn to_lowercase_day(self) -> String;
}

impl LowerDay for String {
    /// "Today" and "Yesterday" read in lower case after a word; a date or a
    /// weekday keeps its capital.
    fn to_lowercase_day(self) -> String {
        match self.as_str() {
            "Today" | "Yesterday" => self.to_lowercase(),
            _ => self,
        }
    }
}

/// The phase 4 part of the view state.
pub fn convo_view(cache: &Cache, state: &ViewState, now: i64) -> ConvoView {
    let artifacts = artifact_tasks(cache);
    let threads = cache
        .convo
        .threads
        .iter()
        .filter_map(|root| thread_view(cache, root, &artifacts, now))
        .collect();
    let search = &cache.convo.search;
    let mut repos: Vec<RepoView> = cache.repos.values().map(|r| repo_view(cache, r)).collect();
    repos.sort_by_key(|a| a.name.to_lowercase());
    let _ = state;
    ConvoView {
        threads,
        search: SearchView {
            query: search.query.clone(),
            scope: search.scope,
            load: search.load.clone(),
            hits: search
                .hits
                .iter()
                .map(|h| hit_view(cache, h, now))
                .collect(),
        },
        item: cache.convo.item.clone(),
        archived: archived_view(cache, now),
        repos,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_urls_match_in_every_form() {
        assert_eq!(
            repo_url_key("git@github.com:watzon/sayso.git"),
            repo_url_key("https://github.com/Watzon/sayso/")
        );
    }
}
