//! The plan of a task, as the thread shows it: a checklist from the plan
//! items of the harness, and the step that the header names.
//!
//! The harness reports its plan as an item whose steps change under one ID.
//! The cache keeps the newest steps ([`crate::cache::ItemSummary::Plan`]).
//! The thread shows them as a card under the newest message. The header of
//! the thread says "Working, <current step>".

use consort_proto::model::{PlanStep, PlanStepStatus};

use crate::view::TaskStatus;

/// Where a step is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlanStepState {
    /// The step is done.
    Done,
    /// The harness works on the step now.
    Current,
    /// The step has not started.
    #[default]
    Next,
}

/// One line of the checklist.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanStepView {
    pub text: String,
    pub state: PlanStepState,
}

/// The plan card of a thread.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanView {
    pub steps: Vec<PlanStepView>,
}

impl PlanView {
    /// The checklist for these steps. A plan with no step with text has no
    /// card, so the result is `None`.
    pub fn of(steps: &[PlanStep]) -> Option<PlanView> {
        let steps: Vec<PlanStepView> = steps
            .iter()
            .filter(|s| !s.text.trim().is_empty())
            .map(|s| PlanStepView {
                text: s.text.trim().to_owned(),
                state: match s.status {
                    PlanStepStatus::Completed => PlanStepState::Done,
                    PlanStepStatus::InProgress => PlanStepState::Current,
                    PlanStepStatus::Pending => PlanStepState::Next,
                },
            })
            .collect();
        (!steps.is_empty()).then_some(PlanView { steps })
    }

    pub fn done(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.state == PlanStepState::Done)
            .count()
    }

    /// The head of the card: "1 of 3 done".
    pub fn heading(&self) -> String {
        format!("{} of {} done", self.done(), self.steps.len())
    }

    /// The step that the harness works on. When none is marked, it is the
    /// first step that has not started, so the header still says what comes
    /// next. `None` when every step is done.
    pub fn current(&self) -> Option<&str> {
        self.steps
            .iter()
            .find(|s| s.state == PlanStepState::Current)
            .or_else(|| self.steps.iter().find(|s| s.state == PlanStepState::Next))
            .map(|s| s.text.as_str())
    }

    /// A key that changes when a step or its state changes, so a list that
    /// caches the height of the card measures it again.
    pub fn key(&self) -> String {
        self.steps
            .iter()
            .map(|s| {
                let mark = match s.state {
                    PlanStepState::Done => 'x',
                    PlanStepState::Current => '>',
                    PlanStepState::Next => '-',
                };
                format!("{mark}{}", s.text.len())
            })
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// The first words of the status line of a thread header: the status, and
/// for a working task the current step. "Working, Find your iBex plan".
pub fn header_status(status: TaskStatus, plan: Option<&PlanView>) -> String {
    match (status, plan.and_then(PlanView::current)) {
        (TaskStatus::Working, Some(step)) => format!("Working, {}", step.trim_end_matches('.')),
        _ => status.label().to_owned(),
    }
}

/// The words of a task line in the chat: the status, and for a working task
/// how long it has run. "Working, 2 min". The line has no card under it, so
/// these words are all the chat says while the task runs.
pub fn line_state(status: TaskStatus, age: &str) -> String {
    match status {
        TaskStatus::NeedsInput => "Needs you".to_owned(),
        TaskStatus::Working if !age.is_empty() && age != "Now" => format!("Working, {age}"),
        other => other.label().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(text: &str, status: PlanStepStatus) -> PlanStep {
        PlanStep {
            text: text.into(),
            status,
        }
    }

    fn plan() -> PlanView {
        PlanView::of(&[
            step("Find the warranty documents", PlanStepStatus::Completed),
            step("Read the plan and the contract", PlanStepStatus::InProgress),
            step("Write the summary as a document", PlanStepStatus::Pending),
        ])
        .unwrap()
    }

    #[test]
    fn the_checklist_keeps_the_order_and_counts_the_done_steps() {
        let plan = plan();
        let states: Vec<_> = plan.steps.iter().map(|s| s.state).collect();
        assert_eq!(
            states,
            [
                PlanStepState::Done,
                PlanStepState::Current,
                PlanStepState::Next
            ]
        );
        assert_eq!(plan.heading(), "1 of 3 done");
        assert_eq!(plan.current(), Some("Read the plan and the contract"));
    }

    #[test]
    fn a_plan_with_no_text_has_no_card() {
        assert_eq!(PlanView::of(&[]), None);
        assert_eq!(PlanView::of(&[step("  ", PlanStepStatus::Pending)]), None);
    }

    #[test]
    fn with_no_step_marked_the_next_step_is_current() {
        let plan = PlanView::of(&[
            step("One", PlanStepStatus::Completed),
            step("Two", PlanStepStatus::Pending),
            step("Three", PlanStepStatus::Pending),
        ])
        .unwrap();
        assert_eq!(plan.current(), Some("Two"));
        let done = PlanView::of(&[step("One", PlanStepStatus::Completed)]).unwrap();
        assert_eq!(done.current(), None);
        assert_eq!(done.heading(), "1 of 1 done");
    }

    #[test]
    fn the_key_changes_when_a_step_moves() {
        let before = plan();
        let mut after = before.clone();
        after.steps[1].state = PlanStepState::Done;
        after.steps[2].state = PlanStepState::Current;
        assert_ne!(before.key(), after.key());
        assert_eq!(before.key(), plan().key());
    }

    #[test]
    fn the_header_names_the_current_step_only_while_the_task_works() {
        let plan = plan();
        assert_eq!(
            header_status(TaskStatus::Working, Some(&plan)),
            "Working, Read the plan and the contract"
        );
        assert_eq!(header_status(TaskStatus::Working, None), "Working");
        assert_eq!(
            header_status(TaskStatus::NeedsInput, Some(&plan)),
            TaskStatus::NeedsInput.label()
        );
        assert_eq!(
            header_status(TaskStatus::Review, Some(&plan)),
            TaskStatus::Review.label()
        );
        let one = PlanView::of(&[step("Find the plan.", PlanStepStatus::InProgress)]);
        assert_eq!(
            header_status(TaskStatus::Working, one.as_ref()),
            "Working, Find the plan"
        );
    }

    #[test]
    fn the_task_line_says_how_long_a_working_task_has_run() {
        assert_eq!(line_state(TaskStatus::Working, "2 min"), "Working, 2 min");
        assert_eq!(line_state(TaskStatus::Working, "Now"), "Working");
        assert_eq!(line_state(TaskStatus::Working, ""), "Working");
        assert_eq!(line_state(TaskStatus::NeedsInput, "2 min"), "Needs you");
        assert_eq!(
            line_state(TaskStatus::Review, "2 min"),
            TaskStatus::Review.label()
        );
    }

    #[test]
    fn a_task_card_is_the_line_alone_unless_something_needs_the_user() {
        use crate::view::{ApprovalView, RequestState, TaskCardView};
        let card = |status| TaskCardView {
            status,
            ..TaskCardView::default()
        };
        for status in [
            TaskStatus::Working,
            TaskStatus::Review,
            TaskStatus::Resolved,
            TaskStatus::UpNext,
            TaskStatus::Scheduled,
            TaskStatus::NeedsInput,
        ] {
            assert!(card(status).is_line_only(), "{status:?}");
        }
        assert!(!card(TaskStatus::Failed).is_line_only(), "the reason stays");
        let mut waiting = card(TaskStatus::Working);
        waiting.approval = Some(ApprovalView {
            state: RequestState::Pending,
            ..ApprovalView::default()
        });
        assert!(!waiting.is_line_only(), "an approval that waits stays");
        waiting.approval.as_mut().unwrap().state = RequestState::Answered;
        assert!(waiting.is_line_only(), "an answered approval is a line");
        let mut voice = card(TaskStatus::Review);
        voice.steps = vec!["Read the sources".into()];
        assert!(!voice.is_line_only());
        let mut note = card(TaskStatus::Review);
        note.note = Some("Raw text stays in this task.".into());
        assert!(!note.is_line_only());
    }
}
