//! Mock content for Monid, from the Paper designs (section 13): the card,
//! the call row in each state, the Channel panel list, the Settings page and
//! the dialog.

use crate::{
    ClientError, Command, CommandOutput,
    connectors::{HubCardView, LoadState},
    monid::*,
    view::{
        ApprovalCardView, ApprovalChoice, ApprovalView, NoticeView, PostBody, RequestState,
        Speaker, TimelineItem, ViewState,
    },
};

const LIGHTS: &str = "chn_house_lights";
const TASK_ROOF: &str = "tsk_0192f0c4-roofline";
const ACME: &str = crate::mock_extensions::ACME;
const SAYSO: &str = "chn_sayso";

pub const CALL_RUNNING: &str = "mcl_mock_running";
pub const CALL_ELEVEN: &str = "mcl_mock_elevenlabs";
pub const CALL_PROVIDER_ERROR: &str = "mcl_mock_serpapi";
pub const CALL_DENIED: &str = "mcl_mock_openai";
pub const CALL_BLOCKED: &str = "mcl_mock_maps";
pub const CALL_STOPPED_BY_MONID: &str = "mcl_mock_minimax_stopped";
pub const CALL_MINIMAX: &str = "mcl_mock_minimax";

pub const APR_CARD: &str = "apr_mock_monid_card";
pub const APR_OVER_LIMIT: &str = "apr_mock_monid_over_limit";

const RUN_ID: &str = "01KQ7M4T9XG2ZB8H3VN6RCY5PD";

/// The Monid state of the mock hub: a key that ends in 3f9a, $49.34 in the
/// wallet and $3.87 spent this month.
pub fn monid() -> MonidView {
    let channel = |id: &str, name: &str, calls: u32, cost: i64| MonidSpendView {
        channel_id: id.into(),
        channel_name: name.into(),
        calls,
        cost_micros: cost,
    };
    MonidView {
        load: LoadState::Loaded,
        key_present: true,
        key_hint: Some("3f9a".into()),
        balance_micros: Some(49_340_000),
        error: None,
        settings: MonidSettingsView::default(),
        spent_month_micros: 3_870_000,
        spend: vec![
            channel(LIGHTS, "house-lights", 14, 2_410_000),
            channel(ACME, "acme", 7, 1_000_000),
            channel(SAYSO, "sayso", 4, 460_000),
        ],
        search: MonidSearchView::default(),
        calls: [(
            LIGHTS.to_owned(),
            MonidCallsView {
                load: LoadState::Loaded,
                calls: vec![
                    recent(CALL_ELEVEN, "ElevenLabs text to speech", 49_600, "11:42 AM"),
                    recent(CALL_MINIMAX, "MiniMax image generation", 3_500, "11:38 AM"),
                ],
            },
        )]
        .into_iter()
        .collect(),
    }
}

fn recent(id: &str, name: &str, cost: i64, time: &str) -> MonidRecentCallView {
    MonidRecentCallView {
        call_id: id.into(),
        name: name.into(),
        state: MonidCallStateView::Completed,
        cost_micros: cost,
        time_label: time.into(),
        summary: money_text(cost),
    }
}

/// What the lights channel may do with Monid, as the M5 design shows it.
pub fn lights_access() -> MonidAccessView {
    let tool = |provider: &str, endpoint: Option<&str>, name: &str| MonidToolView {
        provider: provider.into(),
        endpoint: endpoint.map(Into::into),
        name: name.into(),
    };
    MonidAccessView {
        disabled: false,
        approval: Some(crate::connectors::ApprovalRuleView::AskEveryTime),
        only_allowed: true,
        allowed: vec![
            MonidAllowedView {
                tool: tool("apify", None, "Apify"),
                run_without_asking: false,
            },
            MonidAllowedView {
                tool: tool(
                    "minimax",
                    Some("/v1/image_generation"),
                    "MiniMax image generation",
                ),
                run_without_asking: true,
            },
            MonidAllowedView {
                tool: tool(
                    "elevenlabs",
                    Some("/text-to-speech"),
                    "ElevenLabs text to speech",
                ),
                run_without_asking: false,
            },
        ],
        blocked: vec![tool(
            "apify",
            Some("/google-maps"),
            "Apify Google Maps scraper",
        )],
    }
}

fn row(
    id: &str,
    name: &str,
    state: MonidCallStateView,
    cost: i64,
    stoppable: bool,
) -> MonidCallRowView {
    MonidCallRowView {
        call_id: id.into(),
        name: name.into(),
        state,
        cost_micros: cost,
        stoppable,
        summary: call_summary(state, cost),
        attachments: Vec::new(),
        detail: None,
    }
}

/// The rows of the call states of the M2 design.
pub fn rows() -> Vec<MonidCallRowView> {
    use MonidCallStateView::*;
    vec![
        row(
            CALL_RUNNING,
            "Apify Instagram profile scraper",
            Running,
            0,
            true,
        ),
        row(
            CALL_ELEVEN,
            "ElevenLabs text to speech",
            Completed,
            49_600,
            false,
        ),
        row(
            CALL_PROVIDER_ERROR,
            "SerpApi Instagram profile",
            ProviderError,
            0,
            false,
        ),
        row(CALL_DENIED, "OpenAI GPT Image 2", Denied, 0, false),
        row(CALL_BLOCKED, "Apify Google Maps scraper", Blocked, 0, false),
        row(
            CALL_STOPPED_BY_MONID,
            "MiniMax image generation",
            StoppedByMonid,
            0,
            false,
        ),
    ]
}

/// What `monid.call` gives for a mock call. `None` for an unknown call.
pub fn detail(call_id: &str) -> Option<MonidCallDetailView> {
    let loaded = |input: &str, output: &str, cost: Option<String>, run: Option<String>, note| {
        MonidCallDetailView {
            load: LoadState::Loaded,
            input: input.to_owned(),
            output: output.to_owned(),
            output_cut: false,
            output_file: None,
            cost_line: cost,
            run_line: run,
            note,
        }
    };
    Some(match call_id {
        CALL_ELEVEN => loaded(
            "{ \"text\": \"The piano in my grandmother's front room...\",\n  \"model_id\": \"eleven_v4\", \"voice_id\": \"Rachel\" }",
            "{ \"audio\": { \"format\": \"mp3\", \"duration_s\": 41 } }\nintro-reading.mp3, saved to channel files",
            Some(cost_line(49_600, "$0.08 per 1,000 characters")),
            Some(format!("{RUN_ID}, 2.1 s")),
            None,
        ),
        CALL_PROVIDER_ERROR => loaded(
            "{ \"username\": \"consortmusic\" }",
            "",
            None,
            Some("01KQ7M2H5V9R4D8E1TJ0N3QXWB, 0.8 s".to_owned()),
            Some(
                "The provider answered with an error: the profile is private. Try another profile."
                    .to_owned(),
            ),
        ),
        CALL_DENIED => loaded(
            "{ \"prompt\": \"A front room piano at dusk\" }",
            "",
            None,
            None,
            None,
        ),
        CALL_BLOCKED | CALL_STOPPED_BY_MONID => {
            let state = if call_id == CALL_BLOCKED {
                MonidCallStateView::Blocked
            } else {
                MonidCallStateView::StoppedByMonid
            };
            loaded(
                "{ \"prompt\": \"Old piano cover\" }",
                "",
                None,
                None,
                call_note(state, None),
            )
        }
        CALL_MINIMAX => loaded(
            "{ \"prompt\": \"Old piano cover, soft light\" }",
            "{ \"image_urls\": [\"old-piano-cover.jpg\"] }",
            Some(cost_line(3_500, "$0.0035 per call")),
            Some("01KQ7M0A8WZ3Y6C2B9FH4S1DKE, 6.4 s".to_owned()),
            None,
        ),
        _ => return None,
    })
}

fn notice(id: &str, time: &str, row: MonidCallRowView) -> TimelineItem {
    TimelineItem::Notice(NoticeView {
        message_id: format!("msg_{id}"),
        time_label: time.into(),
        created_at: 0,
        kind: MONID_CALL_KIND.into(),
        text: format!("{}: {}", row.name, row.summary),
        monid: Some(Box::new(row)),
        ..NoticeView::default()
    })
}

fn approval(id: &str, card: MonidCardView, task: Option<&str>) -> ApprovalView {
    let mut choices = vec![ApprovalChoice::AllowOnce];
    let mut labels = vec![(ApprovalChoice::AllowOnce, "Allow once".to_owned())];
    if card.allow_task {
        choices.push(ApprovalChoice::AllowForTask);
        labels.push((
            ApprovalChoice::AllowForTask,
            "Allow for this task".to_owned(),
        ));
    }
    choices.push(ApprovalChoice::Deny);
    labels.push((ApprovalChoice::Deny, "Deny".to_owned()));
    ApprovalView {
        id: id.into(),
        task_id: task.map(Into::into),
        summary: card.title(),
        kind: crate::approval_text::ApprovalKind::Other,
        choices,
        state: RequestState::Pending,
        card: Some(Box::new(HubCardView::Monid(Box::new(card)))),
        button_labels: labels,
        ..ApprovalView::default()
    }
}

/// The card of ElevenLabs, with an estimate.
pub fn pending_card() -> MonidCardView {
    MonidCardView {
        provider: "elevenlabs".into(),
        endpoint: "/text-to-speech".into(),
        name: "ElevenLabs text to speech".into(),
        rate: "$0.08 per 1,000 characters".into(),
        estimate_micros: Some(50_000),
        input: "text: The piano in my grandmother's front room had not been tuned since 1971. Three keys stuck, and one did nothing at all...\nmodel_id: eleven_v4\nvoice_id: Rachel".into(),
        over_limit: None,
        allow_task: true,
        more: vec![
            MonidCardOption {
                id: "always".into(),
                label: "Always allow this tool here".into(),
            },
            MonidCardOption {
                id: "never".into(),
                label: "Never use this tool here".into(),
            },
        ],
    }
}

/// The card of a call that goes over the task limit.
pub fn over_limit_card() -> MonidCardView {
    MonidCardView {
        provider: "serpapi".into(),
        endpoint: "/instagram-profile".into(),
        name: "SerpApi Instagram profile".into(),
        rate: "$0.015 per call".into(),
        estimate_micros: Some(15_000),
        input: "username: consortmusic".into(),
        over_limit: Some(
            "This task has spent $4.99 of its $5.00 limit. This call costs $0.015 and goes over it."
                .into(),
        ),
        allow_task: false,
        more: vec![MonidCardOption {
            id: "never".into(),
            label: "Never use this tool here".into(),
        }],
    }
}

pub const APR_ACCESS_TASK: &str = "apr_mock_access_task";
pub const APR_ACCESS_MONID: &str = "apr_mock_access_monid";

fn access_options(labels: [(&str, &str); 3]) -> Vec<crate::access::AccessCardOption> {
    labels
        .into_iter()
        .map(|(id, label)| crate::access::AccessCardOption {
            id: id.into(),
            label: label.into(),
        })
        .collect()
}

/// The access cards of the M9 design: a task asks for a connection, and the
/// chat asks for Monid.
pub fn access_cards() -> Vec<(
    &'static str,
    &'static str,
    crate::access::AccessCardView,
    Option<&'static str>,
)> {
    vec![
        (
            APR_ACCESS_TASK,
            "msg_access_task",
            crate::access::AccessCardView {
                name: "Gmail Work".into(),
                title: "Give this task access to Gmail Work".into(),
                reason: Some("I need the invoice emails from March to finish the expense report.".into()),
                effect: "Calls still follow the approval rules of Gmail Work. Temporary access ends with this task.".into(),
                options: access_options([
                    ("allow_temporary", "Allow for this task"),
                    ("allow_always", "Always allow here"),
                    ("deny", "Deny"),
                ]),
                temporary_for_task: true,
                answered: None,
            },
            Some(TASK_ROOF),
        ),
        (
            APR_ACCESS_MONID,
            "msg_access_monid",
            crate::access::AccessCardView {
                name: "Monid".into(),
                title: "Give this channel access to Monid".into(),
                reason: Some("This channel makes weekly voice notes, and no connector can make audio.".into()),
                effect: "Calls still follow the Monid approval rule of this channel. Monid's Use in becomes Podcast, Research.".into(),
                options: access_options([
                    ("allow_always", "Always allow here"),
                    ("allow_temporary", "Allow for 24 hours"),
                    ("deny", "Deny"),
                ]),
                temporary_for_task: false,
                answered: None,
            },
            None,
        ),
    ]
}

fn access_approval(
    id: &str,
    card: crate::access::AccessCardView,
    task: Option<&str>,
) -> ApprovalView {
    ApprovalView {
        id: id.into(),
        task_id: task.map(Into::into),
        summary: card.title.clone(),
        kind: crate::approval_text::ApprovalKind::Other,
        choices: vec![
            ApprovalChoice::AllowOnce,
            ApprovalChoice::AllowForTask,
            ApprovalChoice::Deny,
        ],
        state: RequestState::Pending,
        button_labels: card
            .options
            .iter()
            .zip([
                ApprovalChoice::AllowOnce,
                ApprovalChoice::AllowForTask,
                ApprovalChoice::Deny,
            ])
            .map(|(o, c)| (c, o.label.clone()))
            .collect(),
        card: Some(Box::new(HubCardView::Access(Box::new(card)))),
        ..ApprovalView::default()
    }
}

/// Adds Monid to the mock: the panel list of house-lights, the rows and the
/// cards in its timeline, and the total of its task.
pub fn decorate(state: &mut ViewState) {
    if let Some(channel) = state.channels.iter_mut().find(|c| c.id == LIGHTS) {
        channel.monid = lights_access();
    }
    if let Some(task) = state.tasks.iter_mut().find(|t| t.id == TASK_ROOF) {
        task.monid_calls = 83;
        task.monid_cost_micros = 1_240_000;
    }
    if let Some(timeline) = state.timelines.iter_mut().find(|t| t.channel_id == LIGHTS) {
        let mut items = Vec::new();
        for (i, row) in rows().into_iter().enumerate() {
            items.push(notice(&format!("monid_{i}"), "11:36 AM", row));
        }
        for (id, message, card) in [
            (APR_CARD, "msg_monid_card", pending_card()),
            (APR_OVER_LIMIT, "msg_monid_over", over_limit_card()),
        ] {
            items.push(TimelineItem::Approval(ApprovalCardView {
                message_id: message.into(),
                speaker: Speaker::Consort,
                time_label: "11:44 AM".into(),
                created_at: 0,
                approval: approval(id, card, None),
            }));
        }
        for (id, message, card, task) in access_cards() {
            items.push(TimelineItem::Approval(ApprovalCardView {
                message_id: message.into(),
                speaker: Speaker::Consort,
                time_label: "11:46 AM".into(),
                created_at: 0,
                approval: access_approval(id, card, task),
            }));
        }
        timeline.items.extend(items);
    }
    if let Some(channel) = state.channels.iter_mut().find(|c| c.id == LIGHTS) {
        let now = crate::format::now_ms();
        channel.access_grants = vec![
            crate::access::AccessGrantView {
                id: "agr_mock_gmail".into(),
                name: "Gmail Work".into(),
                monid: false,
                task_id: Some(TASK_ROOF.into()),
                expires_at: None,
            },
            crate::access::AccessGrantView {
                id: "agr_mock_eleven".into(),
                name: "ElevenLabs text to speech".into(),
                monid: true,
                task_id: None,
                expires_at: Some(now + 27 * 3_600_000),
            },
        ];
    }
}

/// The state of the screens that need another state than the normal mock:
/// `monid-add` and `monid-offer` open with a hub that has no Monid key.
pub fn screen_state(screen: &str) -> Option<ViewState> {
    match screen {
        "monid-add" | "monid-offer" | "monid-no-key" => {
            let mut state = crate::mock::view_state();
            state.monid.key_present = false;
            state.monid.key_hint = None;
            state.monid.balance_micros = None;
            state.monid.spent_month_micros = 0;
            state.monid.spend.clear();
            state.monid.calls.clear();
            Some(state)
        }
        // The call rows alone, so a screenshot of the opened rows fits.
        "monid-calls" => {
            let mut state = crate::mock::view_state();
            if let Some(timeline) = state.timelines.iter_mut().find(|t| t.channel_id == LIGHTS) {
                timeline
                    .items
                    .retain(|i| !matches!(i, TimelineItem::Approval(_)));
            }
            Some(state)
        }
        _ => None,
    }
}

/// The catalog that the mock search looks in.
fn catalog() -> Vec<MonidSearchHit> {
    let hit = |p: &str, pn: &str, e: &str, n: &str, r: &str| MonidSearchHit {
        provider: p.into(),
        provider_name: pn.into(),
        endpoint: e.into(),
        name: n.into(),
        rate: r.into(),
    };
    vec![
        hit(
            "apify",
            "Apify",
            "/instagram-profile",
            "Apify Instagram profile scraper",
            "$0.0016 per result",
        ),
        hit(
            "apify",
            "Apify",
            "/google-maps",
            "Apify Google Maps scraper",
            "$0.004 per result",
        ),
        hit(
            "serpapi",
            "SerpApi",
            "/instagram-profile",
            "SerpApi Instagram profile",
            "$0.015 per call",
        ),
        hit(
            "elevenlabs",
            "ElevenLabs",
            "/text-to-speech",
            "ElevenLabs text to speech",
            "$0.08 per 1,000 characters",
        ),
        hit(
            "minimax",
            "MiniMax",
            "/v1/image_generation",
            "MiniMax image generation",
            "$0.0035 per call",
        ),
        hit(
            "openai",
            "OpenAI",
            "/v1/image/gpt-image-2",
            "OpenAI GPT Image 2",
            "$0.04 per call",
        ),
    ]
}

fn not_found(message: &str) -> ClientError {
    ClientError::Hub {
        code: "not_found".into(),
        message: message.into(),
    }
}

/// Every Notice row of a call in the state, in the timelines and the threads.
fn for_each_row(state: &mut ViewState, call_id: &str, mut f: impl FnMut(&mut MonidCallRowView)) {
    for timeline in &mut state.timelines {
        for item in &mut timeline.items {
            if let TimelineItem::Notice(n) = item
                && let Some(row) = n.monid.as_deref_mut()
                && row.call_id == call_id
            {
                f(row);
            }
        }
    }
    for thread in &mut state.threads {
        for post in &mut thread.posts {
            for body in &mut post.bodies {
                if let PostBody::Notice {
                    monid: Some(row), ..
                } = body
                    && row.call_id == call_id
                {
                    f(row);
                }
            }
        }
    }
}

fn channel_mut<'a>(
    state: &'a mut ViewState,
    id: &str,
) -> Result<&'a mut crate::view::ChannelView, ClientError> {
    state
        .channels
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| not_found("This channel no longer exists."))
}

/// Runs one Monid command on the mock state.
pub(crate) fn apply(state: &mut ViewState, command: Command) -> Result<CommandOutput, ClientError> {
    match command {
        Command::LoadMonidStatus => state.monid.load = LoadState::Loaded,
        Command::SetMonidKey { api_key, settings } => {
            let key = api_key.0.trim().to_owned();
            if key.is_empty() {
                return Err(ClientError::Hub {
                    code: "invalid".into(),
                    message: "Paste your Monid API key. Make one at app.monid.ai.".into(),
                });
            }
            let hint: String = key
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let kept = monid();
            state.monid = MonidView {
                key_present: true,
                key_hint: Some(hint),
                settings: settings.unwrap_or(kept.settings.clone()),
                ..kept
            };
        }
        Command::RemoveMonidKey => {
            state.monid = MonidView {
                load: LoadState::Loaded,
                settings: state.monid.settings.clone(),
                ..MonidView::default()
            };
        }
        Command::UpdateMonidSettings { settings } => state.monid.settings = settings,
        Command::SetChannelMonid { channel_id, access } => {
            channel_mut(state, &channel_id)?.monid = access;
        }
        Command::SearchMonid { query } => {
            let needle = query.trim().to_lowercase();
            state.monid.search.query = query;
            state.monid.search.hits = if needle.is_empty() {
                Vec::new()
            } else {
                catalog()
                    .into_iter()
                    .filter(|h| {
                        h.name.to_lowercase().contains(&needle)
                            || h.provider_name.to_lowercase().contains(&needle)
                    })
                    .collect()
            };
            state.monid.search.load = LoadState::Loaded;
        }
        Command::LoadMonidCalls { channel_id } => {
            state.monid.calls.entry(channel_id).or_default().load = LoadState::Loaded;
        }
        Command::LoadMonidCall { call_id } => {
            let found = detail(&call_id);
            for_each_row(state, &call_id, |row| row.detail = found.clone());
        }
        Command::StopMonidCall { call_id } => {
            for_each_row(state, &call_id, |row| {
                row.state = MonidCallStateView::Stopped;
                row.stoppable = false;
                row.summary = call_summary(row.state, row.cost_micros);
            });
        }
        Command::AnswerApprovalOption {
            approval_id,
            option_id,
        } => {
            let mut channel = None;
            let mut tool = None;
            for timeline in &mut state.timelines {
                for item in &mut timeline.items {
                    if let TimelineItem::Approval(c) = item
                        && c.approval.id == approval_id
                    {
                        c.approval.state = RequestState::Answered;
                        c.approval.answer = Some(if option_id == "never" {
                            ApprovalChoice::Deny
                        } else {
                            ApprovalChoice::AllowForTask
                        });
                        if let Some(HubCardView::Monid(card)) = c.approval.card.as_deref() {
                            tool = Some(MonidToolView {
                                provider: card.provider.clone(),
                                endpoint: Some(card.endpoint.clone()),
                                name: card.name.clone(),
                            });
                        }
                        channel = Some(timeline.channel_id.clone());
                    }
                }
            }
            if let (Some(channel), Some(tool)) = (channel, tool) {
                let channel = channel_mut(state, &channel)?;
                channel.monid = if option_id == "never" {
                    channel.monid.with_blocked(tool)
                } else {
                    let mut next = channel.monid.with_allowed(tool.clone());
                    for entry in &mut next.allowed {
                        if entry.tool == tool {
                            entry.run_without_asking = true;
                        }
                    }
                    next
                };
            }
        }
        Command::EndAccess {
            channel_id,
            grant_id,
        } => {
            let channel = channel_mut(state, &channel_id)?;
            channel.access_grants.retain(|g| g.id != grant_id);
        }
        Command::SetRoutineMonid { routine_id, monid } => {
            let routine = state
                .routines
                .iter_mut()
                .find(|r| r.id == routine_id)
                .ok_or_else(|| not_found("This routine no longer exists."))?;
            routine.monid = monid;
        }
        _ => {}
    }
    Ok(CommandOutput::Done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ViewState {
        crate::mock::view_state()
    }

    fn lights(state: &ViewState) -> &crate::view::ChannelView {
        state.channel(LIGHTS).expect("house-lights")
    }

    #[test]
    fn the_mock_has_every_state_the_designs_show() {
        let state = state();
        assert_eq!(state.monid.balance_micros, Some(49_340_000));
        assert_eq!(
            state.monid.footer_detail(),
            "$49.34 left. $3.87 spent this month."
        );
        let row = crate::monid::channel_row(&state, LIGHTS).expect("the panel row");
        assert_eq!(
            row.detail,
            "Paid tools. Asks before each call. $2.41 this month."
        );
        let task = state.task(TASK_ROOF).expect("task");
        assert_eq!(
            crate::monid::task_line(task.monid_calls).as_deref(),
            Some("Monid: 83 calls so far")
        );
        assert_eq!(task.monid_cost_micros, 1_240_000);
        let timeline = state.timeline(LIGHTS).expect("timeline");
        let states: Vec<_> = timeline
            .items
            .iter()
            .filter_map(|i| match i {
                TimelineItem::Notice(n) => n.monid.as_deref().map(|r| r.state),
                _ => None,
            })
            .collect();
        assert_eq!(states.len(), 6);
        let cards: Vec<_> = timeline
            .items
            .iter()
            .filter_map(|i| match i {
                TimelineItem::Approval(c) => c.approval.card.as_deref().cloned(),
                _ => None,
            })
            .collect();
        assert!(
            cards
                .iter()
                .any(|c| matches!(c, HubCardView::Monid(m) if m.over_limit.is_some()))
        );
        assert!(
            cards
                .iter()
                .any(|c| matches!(c, HubCardView::Monid(m) if m.over_limit.is_none()))
        );
        assert_eq!(
            cards
                .iter()
                .filter(|c| matches!(c, HubCardView::Access(_)))
                .count(),
            2
        );
        let rows = crate::access::temporary_access_rows(&state, LIGHTS, crate::format::now_ms());
        assert_eq!(rows.len(), 2);
        assert!(rows[1].detail.starts_with("Monid. Until "));
    }

    #[test]
    fn opening_a_row_loads_its_text_and_stop_ends_a_running_call() {
        let mut state = state();
        apply(
            &mut state,
            Command::LoadMonidCall {
                call_id: CALL_ELEVEN.into(),
            },
        )
        .unwrap();
        let row = {
            let mut found = None;
            for_each_row(&mut state, CALL_ELEVEN, |r| found = Some(r.clone()));
            found.expect("row")
        };
        let detail = row.detail.expect("detail");
        assert!(
            detail
                .cost_line
                .unwrap()
                .starts_with("$0.0496, 620 characters")
        );
        apply(
            &mut state,
            Command::StopMonidCall {
                call_id: CALL_RUNNING.into(),
            },
        )
        .unwrap();
        let mut stopped = None;
        for_each_row(&mut state, CALL_RUNNING, |r| stopped = Some(r.clone()));
        let stopped = stopped.expect("row");
        assert_eq!(stopped.summary, "Stopped");
        assert!(!stopped.stoppable);
    }

    #[test]
    fn a_card_option_changes_the_list_of_the_channel() {
        let mut state = state();
        apply(
            &mut state,
            Command::AnswerApprovalOption {
                approval_id: APR_CARD.into(),
                option_id: "always".into(),
            },
        )
        .unwrap();
        assert!(lights(&state).monid.allowed.iter().any(|a| {
            a.tool.endpoint.as_deref() == Some("/text-to-speech") && a.run_without_asking
        }));
        apply(
            &mut state,
            Command::AnswerApprovalOption {
                approval_id: APR_OVER_LIMIT.into(),
                option_id: "never".into(),
            },
        )
        .unwrap();
        assert!(
            lights(&state)
                .monid
                .blocked
                .iter()
                .any(|b| b.provider == "serpapi")
        );
    }

    #[test]
    fn the_key_never_shows_and_the_page_changes_with_the_commands() {
        let mut state = state();
        apply(
            &mut state,
            Command::SetMonidKey {
                api_key: crate::command::SecretText("monid_live_abcd1234".into()),
                settings: None,
            },
        )
        .unwrap();
        assert_eq!(state.monid.key_text(), "Ends in 1234");
        assert!(!format!("{:?}", state.monid).contains("monid_live"));
        apply(&mut state, Command::RemoveMonidKey).unwrap();
        assert!(!state.monid.key_present);
        // Search finds a tool or a provider, and an empty query clears it.
        apply(
            &mut state,
            Command::SearchMonid {
                query: "apify".into(),
            },
        )
        .unwrap();
        assert_eq!(state.monid.search.hits.len(), 2);
        apply(
            &mut state,
            Command::SearchMonid {
                query: String::new(),
            },
        )
        .unwrap();
        assert!(state.monid.search.hits.is_empty());
        // A routine takes its own setting.
        let routine = state.routines[0].id.clone();
        apply(
            &mut state,
            Command::SetRoutineMonid {
                routine_id: routine.clone(),
                monid: Some(MonidUnattendedView::Off),
            },
        )
        .unwrap();
        assert_eq!(state.routines[0].monid, Some(MonidUnattendedView::Off));
        // Ending access removes the grant.
        apply(
            &mut state,
            Command::EndAccess {
                channel_id: LIGHTS.into(),
                grant_id: "agr_mock_gmail".into(),
            },
        )
        .unwrap();
        assert_eq!(lights(&state).access_grants.len(), 1);
    }
}
