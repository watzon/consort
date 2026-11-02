//! Mock first conversation: the content of the guided start in Tutti (Paper
//! T1 to T8, the worker card and the failed first turn), for `--screen`.
//!
//! The assistant's words here are from the designs. A real turn writes its own
//! words from `prompts/guide.md`.

use chrono::{Duration, NaiveTime, TimeZone};
use consort_proto::model::GuideState;

use crate::connectors::{
    AddWorkerCardView, ConnectCardView, HubCardView, LoadState, ServiceLogo, ToolApprovalsView,
    UseInView,
};
use crate::proactivity::SuggestionView;
use crate::view::{
    ApprovalCardView, ApprovalChoice, ApprovalView, DayView, MemorySavedView, MessageView,
    NoticeView, PairingCodeView, RequestState, Speaker, TimelineItem, UserView, ViewState,
    WorkerView,
};

/// Every screen of this module.
pub const FIRST_SCREENS: [&str; 13] = [
    "first-t1",
    "first-t2",
    "first-t3",
    "first-t4",
    "first-t5",
    "first-t6",
    "first-t7",
    "first-t8",
    "first-signed-out",
    "first-permission",
    "first-worker-other",
    "first-worker-loopback",
    "first-failed",
];

const TIME_HOUR: u32 = 20;
const TIME_MINUTE: u32 = 14;
const CODE_MS: i64 = 582_000;

/// Today at the time of the designs, plus `seconds`.
fn at(seconds: i64) -> i64 {
    let date = chrono::Local::now().date_naive();
    let time = NaiveTime::from_hms_opt(TIME_HOUR, TIME_MINUTE, 0).unwrap_or_default();
    chrono::Local
        .from_local_datetime(&date.and_time(time))
        .earliest()
        .map_or(0, |t| (t + Duration::seconds(seconds)).timestamp_millis())
}

fn assistant(id: &str, seconds: i64, body: &str) -> TimelineItem {
    TimelineItem::Message(message(id, seconds, Speaker::Consort, body, &[]))
}

fn asking(id: &str, seconds: i64, body: &str, chips: &[&str]) -> TimelineItem {
    TimelineItem::Message(message(id, seconds, Speaker::Consort, body, chips))
}

fn you(id: &str, seconds: i64, body: &str) -> TimelineItem {
    TimelineItem::Message(message(id, seconds, Speaker::You, body, &[]))
}

fn message(id: &str, seconds: i64, speaker: Speaker, body: &str, chips: &[&str]) -> MessageView {
    MessageView {
        id: id.into(),
        channel_id: crate::mock::tutti_id().into(),
        speaker,
        time_label: crate::format::time_label(at(seconds)),
        created_at: at(seconds),
        body: body.into(),
        suggestions: chips
            .iter()
            .map(|label| SuggestionView {
                label: (*label).into(),
                type_it: false,
            })
            .collect(),
        ..MessageView::default()
    }
}

fn notice(id: &str, seconds: i64, kind: &str, text: &str) -> TimelineItem {
    TimelineItem::Notice(NoticeView {
        message_id: id.into(),
        time_label: crate::format::time_label(at(seconds)),
        created_at: at(seconds),
        kind: kind.into(),
        text: text.into(),
        ..NoticeView::default()
    })
}

fn card(
    id: &str,
    seconds: i64,
    summary: &str,
    view: HubCardView,
    labels: (&str, &str),
) -> TimelineItem {
    TimelineItem::Approval(ApprovalCardView {
        message_id: id.into(),
        speaker: Speaker::Consort,
        time_label: crate::format::time_label(at(seconds)),
        created_at: at(seconds),
        approval: ApprovalView {
            check_note: None,
            blocked: Vec::new(),
            id: format!("apr_{id}"),
            task_id: None,
            summary: summary.into(),
            folder: None,
            detail: None,
            kind: crate::approval_text::ApprovalKind::Other,
            choices: vec![ApprovalChoice::AllowOnce, ApprovalChoice::Deny],
            state: RequestState::Pending,
            answer: None,
            allow_always_label: None,
            card: Some(Box::new(view)),
            button_labels: vec![
                (ApprovalChoice::AllowOnce, labels.0.into()),
                (ApprovalChoice::Deny, labels.1.into()),
            ],
        },
    })
}

const HELLO: &str = "Hello, I am your assistant. This is Tutti, where all the voices play together. From here I can read every channel and manage all of Consort.";
const OFFER: &str = "Do you want me to help you set up? I ask four short questions: who you are, which services you use, where I can run work, and what you work on.";

fn opening(chips: &[&str]) -> Vec<TimelineItem> {
    vec![
        TimelineItem::Day(DayView {
            key: "day-first".into(),
            label: "Today".into(),
        }),
        notice(
            "ntc_signed_in",
            0,
            "signed_in",
            "You signed in to Claude. The assistant can answer now.",
        ),
        assistant("msg_hello", 2, HELLO),
        asking("msg_offer", 3, OFFER, chips),
    ]
}

fn worker_card(this_computer: bool) -> TimelineItem {
    card(
        "worker",
        200,
        "Add a worker",
        HubCardView::AddWorker(AddWorkerCardView {
            this_computer,
            worker_name: None,
        }),
        ("Done", "Not now"),
    )
}

fn worker_question() -> Vec<TimelineItem> {
    vec![
        assistant(
            "msg_q3",
            120,
            "Third question, of four. Where can I run work?",
        ),
        assistant(
            "msg_q3_more",
            121,
            "I can talk to you from the hub, but a task needs a worker: a machine with your files and repos. No worker is connected yet. You set its limits, and it does not need open ports.",
        ),
        you("msg_a3", 180, "This computer"),
    ]
}

fn items(screen: &str) -> Vec<TimelineItem> {
    match screen {
        "first-t1" => opening(&["Yes", "Not now", "No, I look around myself"]),
        "first-t2" => {
            let mut items = opening(&[]);
            items.drain(..2);
            items.extend([
                you("msg_yes", 20, "Yes"),
                assistant("msg_q1", 22, "Good. First question, of four."),
                assistant(
                    "msg_q1_more",
                    23,
                    "What should I call you, and what do you do? A sentence or two is enough. I keep your answer in memory, and I read it in every channel.",
                ),
            ]);
            items
        }
        "first-t3" => vec![
            you(
                "msg_a1",
                60,
                "Chris. I build software, and I run a small home lab.",
            ),
            TimelineItem::Notice(NoticeView {
                message_id: "ntc_memory".into(),
                time_label: crate::format::time_label(at(61)),
                created_at: at(61),
                kind: "memory_saved".into(),
                text: "Saved to memory".into(),
                memory: Some(MemorySavedView {
                    scope: crate::memory::GLOBAL.into(),
                    path: "about-you.md".into(),
                    version: 1,
                    created: true,
                    suggested_by: String::new(),
                }),
                ..NoticeView::default()
            }),
            assistant("msg_q2", 62, "Thanks, Chris. Second question, of four."),
            assistant(
                "msg_q2_more",
                63,
                "Which services do you use each day? I can read them for you. I ask you first before I change or send anything.",
            ),
            you("msg_a2", 90, "Gmail"),
            card(
                "connect",
                92,
                "Connect Gmail",
                HubCardView::ConnectService(ConnectCardView {
                    service: "gmail".into(),
                    service_name: "Gmail".into(),
                    logo: ServiceLogo::default(),
                    name: "Gmail".into(),
                    use_in: UseInView::EveryChannel,
                    approvals: ToolApprovalsView::default(),
                    connection_id: None,
                }),
                ("Continue in browser", "Not now"),
            ),
        ],
        "first-t4" | "first-worker-loopback" => {
            let mut items = worker_question();
            items.push(worker_card(true));
            items
        }
        "first-worker-other" => {
            let mut items = worker_question();
            items.pop();
            items.push(you("msg_a3", 180, "Another machine"));
            items.push(worker_card(false));
            items
        }
        "first-t5" => vec![
            assistant("msg_q4", 300, "MacBook Pro is online. Last question."),
            assistant(
                "msg_q4_more",
                301,
                "What do you work on? I make a channel for each subject, so that its tasks, notes and files stay together.",
            ),
            you("msg_a4", 340, "Sayso, and my home lab."),
            notice("ntc_sayso", 342, "channel_made", "Created the channel sayso"),
            notice("ntc_homelab", 343, "channel_made", "Created the channel homelab"),
            assistant(
                "msg_channels",
                344,
                "Both channels are in the sidebar. When you open one, I ask what it is for and which repo it uses.",
            ),
        ],
        "first-t6" => {
            let mut items = opening(&[]);
            items.drain(..2);
            items.extend([
                you("msg_not_now", 20, "Not now"),
                assistant(
                    "msg_later",
                    22,
                    "OK. Ask me anything now. I offer this once more tomorrow, and then I stop. You can also say \"help me set up\" at any time.",
                ),
            ]);
            items
        }
        "first-t7" | "first-permission" => vec![
            assistant("msg_done", 400, "That is all, Chris. You are set up."),
            assistant(
                "msg_done_list",
                401,
                "I can read Gmail, I run tasks on MacBook Pro, and you have two channels.",
            ),
            asking(
                "msg_done_more",
                402,
                "I also look in by myself, and I send a short brief each morning. Tell me to be quieter at any time. What should we start with?",
                &["What is in my mail today?", "Find my Sayso repo", "Be quieter"],
            ),
        ],
        "first-signed-out" => opening(&[]),
        "first-failed" => vec![
            TimelineItem::Day(DayView {
                key: "day-first".into(),
                label: "Today".into(),
            }),
            notice(
                "ntc_signed_in",
                0,
                "signed_in",
                "You signed in to Claude. The assistant can answer now.",
            ),
            TimelineItem::Notice(NoticeView {
                message_id: "ntc_failed".into(),
                time_label: crate::format::time_label(at(4)),
                created_at: at(4),
                kind: "turn_failed".into(),
                text: String::new(),
                error: Some(crate::turn_errors::ErrorCardView {
                    title: "The assistant could not start".into(),
                    explanation: "Claude signed in, but the first turn failed: no boundary on the hub machine. The details say what to change.".into(),
                    actions: vec![crate::turn_errors::ErrorAction::TryAgain],
                    details: "The session host found no boundary on this computer.".into(),
                    try_again: Some(crate::turn_errors::TryAgainTarget::Greeting),
                    ..Default::default()
                }),
                ..NoticeView::default()
            }),
        ],
        _ => Vec::new(),
    }
}

/// A view state for a first conversation: Tutti, plus what the step has made.
pub fn screen_state(screen: &str) -> Option<ViewState> {
    if !FIRST_SCREENS.contains(&screen) {
        return None;
    }
    let mut state = crate::mock::view_state();
    let tutti = crate::mock::tutti_id().to_owned();
    let made = matches!(screen, "first-t5" | "first-t7" | "first-permission");
    state
        .channels
        .retain(|c| c.id == tutti || (made && matches!(c.name.as_str(), "sayso" | "homelab")));
    state.groups.clear();
    state.archived_channels.clear();
    for host in state.session_host.iter_mut() {
        host.accounts
            .retain(|a| a.status != crate::AccountStatus::Expired);
    }
    state.activity.clear();
    state.tasks.clear();
    state.tasks_panel = Default::default();
    state.user = UserView::from_name("Chris");
    state.composio.load = LoadState::Loaded;
    state.composio.key_present = false;
    if !made {
        state.workers.clear();
    } else {
        state.workers = vec![WorkerView {
            id: "wrk_macbook".into(),
            name: "MacBook Pro".into(),
            os: crate::Os::MacOs,
            online: true,
            ..WorkerView::default()
        }];
    }
    state.timelines.retain(|t| t.channel_id == tutti);
    if let Some(timeline) = state.timelines.first_mut() {
        timeline.items = items(screen);
        timeline.has_older = false;
    }
    state.hub_setup.guide = match screen {
        "first-t7" | "first-permission" => GuideState::Closed,
        "first-t8" => GuideState::NotStarted,
        "first-t6" => GuideState::Stopped,
        _ => GuideState::Running,
    };
    if matches!(screen, "first-t8" | "first-signed-out") {
        state.channels.iter_mut().for_each(|c| {
            c.coordinator_selection = None;
        });
    }
    if matches!(
        screen,
        "first-t4" | "first-worker-other" | "first-worker-loopback"
    ) {
        state.worker_pairing = Some(PairingCodeView {
            code: "KQ7M-4XPD".into(),
            hub_url: if screen == "first-worker-loopback" {
                "http://127.0.0.1:7600".into()
            } else {
                "https://hub.example.com".into()
            },
            tls_sha256: None,
            expires_at: crate::format::now_ms() + CODE_MS,
            loopback_only: screen == "first-worker-loopback",
        });
    }
    crate::derive::refresh(&mut state);
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_screen_has_a_state_and_others_have_none() {
        for screen in FIRST_SCREENS {
            assert!(screen_state(screen).is_some(), "{screen}");
        }
        assert!(screen_state("tutti").is_none());
    }

    #[test]
    fn the_worker_card_holds_no_code_of_its_own() {
        let state = screen_state("first-t4").expect("a state");
        let timeline = state.timelines.first().expect("Tutti");
        let shown = format!("{:?}", timeline.items);
        assert!(!shown.contains("KQ7M-4XPD"));
        assert!(state.worker_pairing.is_some());
    }

    #[test]
    fn the_guide_state_follows_the_screen() {
        let state = |s| screen_state(s).expect("a state").hub_setup.guide;
        assert_eq!(state("first-t7"), GuideState::Closed);
        assert_eq!(state("first-t1"), GuideState::Running);
    }
}
