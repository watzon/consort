//! Access requests and temporary access (ADR 0017): the card that asks for
//! a connection or for Monid, and the grants that the Channel panel lists.

use consort_proto::model::{
    ACCESS_CARD_ALWAYS, ACCESS_CARD_TEMPORARY, AccessGrant, AccessRequestCard, AccessTarget,
    RequestOption,
};

use crate::{
    format,
    view::{TaskStatus, ViewState},
};

/// One button of an access card, in the order of the approval's options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessCardOption {
    /// The option ID that the answer sends.
    pub id: String,
    pub label: String,
}

/// A session asks for access that its channel does not have.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccessCardView {
    /// The name of what it asks for, such as "Gmail Work" or "Monid".
    pub name: String,
    /// "Give this task access to Gmail Work".
    pub title: String,
    /// Why the session asks. Shown only when set.
    pub reason: Option<String>,
    /// What each answer does.
    pub effect: String,
    /// The buttons. The first is the accent button.
    pub options: Vec<AccessCardOption>,
    /// The temporary option lasts as long as a task.
    pub temporary_for_task: bool,
    /// The option that the user chose, once answered.
    pub answered: Option<String>,
}

impl AccessCardView {
    /// The line that the card shrinks to after the answer: "Gmail Work:
    /// access for this task", "Gmail Work: access in this channel", or
    /// "Gmail Work: access" after Deny.
    pub fn answered_line(&self) -> String {
        let what = match self.answered.as_deref() {
            Some(ACCESS_CARD_TEMPORARY) if self.temporary_for_task => " for this task",
            Some(ACCESS_CARD_TEMPORARY) => " for 24 hours",
            Some(ACCESS_CARD_ALWAYS) => " in this channel",
            _ => "",
        };
        format!("{}: access{what}", self.name)
    }

    /// The card was answered with an allow.
    pub fn allowed(&self) -> bool {
        matches!(
            self.answered.as_deref(),
            Some(ACCESS_CARD_TEMPORARY | ACCESS_CARD_ALWAYS)
        )
    }
}

/// The view of an access card. The buttons are the options of the approval,
/// in the order the hub sent them.
pub fn card_view(card: &AccessRequestCard, options: &[RequestOption]) -> AccessCardView {
    AccessCardView {
        name: card.target.label().to_owned(),
        title: card.title.clone(),
        reason: card
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_owned),
        effect: card.effect.clone(),
        options: options
            .iter()
            .map(|o| AccessCardOption {
                id: o.id.clone(),
                label: o.label.clone(),
            })
            .collect(),
        temporary_for_task: card.temporary_label.to_lowercase().contains("task"),
        answered: None,
    }
}

/// Temporary access on a channel, as the hub sent it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessGrantView {
    pub id: String,
    /// What it gives, such as "Gmail Work".
    pub name: String,
    /// The target is Monid or a Monid tool.
    pub monid: bool,
    /// Set for access that is for one task.
    pub task_id: Option<String>,
    /// When it ends, in Unix milliseconds.
    pub expires_at: Option<i64>,
}

pub fn grant_view(grant: &AccessGrant) -> AccessGrantView {
    AccessGrantView {
        id: grant.id.to_string(),
        name: grant.target.label().to_owned(),
        monid: matches!(
            grant.target,
            AccessTarget::Monid | AccessTarget::MonidTool { .. }
        ),
        task_id: grant.task_id.map(|t| t.to_string()),
        expires_at: grant.expires_at.map(|t| t.as_millis()),
    }
}

/// One row of the Temporary access section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemporaryAccessRow {
    pub grant_id: String,
    pub name: String,
    /// 'Until "Price out roofline lighting" ends' or "Until Oct 31, 18:00".
    /// A Monid target starts with "Monid. ".
    pub detail: String,
}

fn task_ended(status: TaskStatus, session_ended: bool) -> bool {
    matches!(status, TaskStatus::Resolved | TaskStatus::Failed) || session_ended
}

/// The rows of the Temporary access section of a channel. A grant that has
/// expired, or whose task has ended, is left out. Empty when no grant is
/// live, and then the section does not show.
pub fn temporary_access_rows(
    state: &ViewState,
    channel_id: &str,
    now_ms: i64,
) -> Vec<TemporaryAccessRow> {
    let Some(channel) = state.channel(channel_id) else {
        return Vec::new();
    };
    channel
        .access_grants
        .iter()
        .filter_map(|grant| {
            if grant.expires_at.is_some_and(|end| end <= now_ms) {
                return None;
            }
            let until = match &grant.task_id {
                Some(task_id) => {
                    let task = state.tasks.iter().find(|t| &t.id == task_id)?;
                    if task_ended(task.status, task.session_ended) {
                        return None;
                    }
                    format!("Until \"{}\" ends", task.title)
                }
                None => format!("Until {}", format::when_label(grant.expires_at?, now_ms)),
            };
            let detail = if grant.monid {
                format!("Monid. {until}")
            } else {
                until
            };
            Some(TemporaryAccessRow {
                grant_id: grant.id.clone(),
                name: grant.name.clone(),
                detail,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{ChannelView, TaskView};

    fn grant(id: &str, monid: bool, task: Option<&str>, expires: Option<i64>) -> AccessGrantView {
        AccessGrantView {
            id: id.into(),
            name: if monid { "Monid" } else { "Gmail Work" }.into(),
            monid,
            task_id: task.map(Into::into),
            expires_at: expires,
        }
    }

    fn state(grants: Vec<AccessGrantView>, tasks: Vec<TaskView>) -> ViewState {
        let mut state = ViewState::default();
        state.channels.push(ChannelView {
            id: "chn_1".into(),
            access_grants: grants,
            ..ChannelView::default()
        });
        state.tasks = tasks;
        state
    }

    fn task(id: &str, status: TaskStatus) -> TaskView {
        TaskView {
            id: id.into(),
            channel_id: "chn_1".into(),
            title: "Price out roofline lighting".into(),
            status,
            ..TaskView::default()
        }
    }

    #[test]
    fn a_live_grant_has_a_row_and_a_monid_grant_says_so() {
        let now = 1_000_000;
        let state = state(
            vec![
                grant("a", false, Some("t1"), None),
                grant("b", true, None, Some(now + 3_600_000)),
            ],
            vec![task("t1", TaskStatus::Working)],
        );
        let rows = temporary_access_rows(&state, "chn_1", now);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "Gmail Work");
        assert_eq!(rows[0].detail, "Until \"Price out roofline lighting\" ends");
        assert_eq!(rows[1].name, "Monid");
        assert!(
            rows[1].detail.starts_with("Monid. Until "),
            "{}",
            rows[1].detail
        );
    }

    #[test]
    fn an_expired_grant_or_an_ended_task_has_no_row() {
        let now = 1_000_000;
        let state = state(
            vec![
                grant("old", true, None, Some(now - 1)),
                grant("done", false, Some("t1"), None),
                grant("gone", false, Some("t_missing"), None),
                grant("ended", false, Some("t2"), None),
            ],
            vec![
                task("t1", TaskStatus::Resolved),
                TaskView {
                    session_ended: true,
                    ..task("t2", TaskStatus::Review)
                },
            ],
        );
        assert!(temporary_access_rows(&state, "chn_1", now).is_empty());
        assert!(temporary_access_rows(&state, "chn_none", now).is_empty());
    }

    #[test]
    fn the_answered_card_is_one_line() {
        let mut card = AccessCardView {
            name: "Gmail Work".into(),
            temporary_for_task: true,
            ..AccessCardView::default()
        };
        card.answered = Some(ACCESS_CARD_TEMPORARY.into());
        assert_eq!(card.answered_line(), "Gmail Work: access for this task");
        assert!(card.allowed());
        card.answered = Some(ACCESS_CARD_ALWAYS.into());
        assert_eq!(card.answered_line(), "Gmail Work: access in this channel");
        card.answered = Some("deny".into());
        assert_eq!(card.answered_line(), "Gmail Work: access");
        assert!(!card.allowed());
    }

    #[test]
    fn the_buttons_follow_the_options_of_the_hub() {
        use consort_proto::model::{AccessDuration, HUB_CARD_DENY, RequestOptionKind};
        let wire = AccessRequestCard {
            target: AccessTarget::Monid,
            requested: AccessDuration::Permanent,
            title: "Give this channel access to Monid".into(),
            reason: Some("  ".into()),
            effect: "Always allow here gives the channel Monid.".into(),
            temporary_label: "Allow for 24 hours".into(),
        };
        let opt = |id: &str, label: &str| RequestOption {
            id: id.into(),
            label: label.into(),
            kind: RequestOptionKind::AllowOnce,
            description: None,
        };
        let options = vec![
            opt(ACCESS_CARD_ALWAYS, "Always allow here"),
            opt(ACCESS_CARD_TEMPORARY, "Allow for 24 hours"),
            opt(HUB_CARD_DENY, "Deny"),
        ];
        let view = card_view(&wire, &options);
        assert_eq!(view.options[0].id, ACCESS_CARD_ALWAYS);
        assert_eq!(view.options.len(), 3);
        assert_eq!(view.reason, None);
        assert!(!view.temporary_for_task);
        assert_eq!(view.name, "Monid");
    }
}
