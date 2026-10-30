//! The words of an approval card: its title, the one line that it shrinks
//! to after the answer, and the answer.
//!
//! The title says what the action is. The block under it says the exact
//! command or target. The action shows once, so a title that only repeats the
//! command falls back to a line for the kind of the action ("Run a
//! command").

use consort_proto::model::ApprovalDetails;

use crate::view::{ApprovalChoice, ApprovalView, RequestState};

/// A long command stops at this many lines until the user asks for all of it.
pub const COMMAND_LINES: usize = 5;

/// A button label longer than this gets its own row on the card. "Allow for
/// this task" still fits in the row of three.
const BUTTON_ROW_LABEL_CHARS: usize = 20;

/// Whether a button label is too long for the row of equal buttons, as a
/// harness's "Yes, and don't ask again for cargo build commands" is.
pub fn label_needs_own_row(label: &str) -> bool {
    label.chars().count() > BUTTON_ROW_LABEL_CHARS
}

/// What an approval asks for, in the words that a title falls back to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ApprovalKind {
    /// A card with no known action: an approval that closed before this
    /// client loaded it, and the cards that the hub makes itself.
    #[default]
    Other,
    /// Run a command.
    Command,
    /// Change files.
    FileChange,
    /// Reach the network: a web page or a web search.
    Network,
    /// Call a tool.
    Tool,
}

impl ApprovalKind {
    pub fn of(details: &ApprovalDetails) -> Self {
        match details {
            ApprovalDetails::Command { .. } => Self::Command,
            ApprovalDetails::FileChange { .. } => Self::FileChange,
            ApprovalDetails::Network { .. } => Self::Network,
            ApprovalDetails::Tool { .. } => Self::Tool,
            _ => Self::Other,
        }
    }
}

/// Text with every run of white space cut to one space, and with the quotes
/// and the ellipsis that a summary may add around a command taken off.
fn normal(text: &str) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    joined
        .trim_matches(|c| matches!(c, '`' | '"' | '\''))
        .trim_end_matches(['…', '.'])
        .trim_end()
        .to_owned()
}

/// Whether a summary only says the command again. The summary is text that
/// the agent wrote, so this is a check of words and not of meaning: the
/// summary is the command, holds it, or is the start of it that the agent
/// cut short.
pub fn repeats(summary: &str, detail: &str) -> bool {
    let summary = normal(summary);
    let detail = normal(detail);
    if summary.is_empty() || detail.is_empty() {
        return false;
    }
    summary == detail
        || summary.contains(&detail)
        // A cut summary, such as "curl -sS -o title.pdf ...".
        || (summary.chars().count() >= 8 && detail.contains(&summary))
}

impl ApprovalView {
    fn target_is_page(&self) -> bool {
        let target = self.detail.as_deref().unwrap_or_default().trim();
        target.starts_with("http://") || target.starts_with("https://")
    }

    /// The line for the kind of the action, as a title: "Run a command".
    fn kind_title(&self) -> &'static str {
        match self.kind {
            ApprovalKind::Command => "Run a command",
            ApprovalKind::FileChange => "Change files",
            ApprovalKind::Network if self.target_is_page() => "Open a web page",
            ApprovalKind::Network => "Search the web",
            ApprovalKind::Tool => "Use a tool",
            ApprovalKind::Other => "A request for your approval",
        }
    }

    /// The same line after the action ran: "Ran a command".
    fn kind_title_done(&self) -> &'static str {
        match self.kind {
            ApprovalKind::Command => "Ran a command",
            ApprovalKind::FileChange => "Changed files",
            ApprovalKind::Network if self.target_is_page() => "Opened a web page",
            ApprovalKind::Network => "Searched the web",
            ApprovalKind::Tool => "Used a tool",
            ApprovalKind::Other => "A request for your approval",
        }
    }

    /// Whether the summary says nothing that the block does not say: it is
    /// empty, or it repeats the command or the target.
    fn summary_is_noise(&self) -> bool {
        let summary = self.summary.trim();
        summary.is_empty()
            || self
                .detail
                .as_deref()
                .is_some_and(|detail| repeats(summary, detail))
    }

    /// The title of the card while it waits: the summary, and when the
    /// summary repeats the command, a line for the kind of the action.
    pub fn title(&self) -> String {
        if self.summary_is_noise() {
            self.kind_title().to_owned()
        } else {
            self.summary.trim().to_owned()
        }
    }

    /// The title of an opened answered line. A title that fell back to the
    /// kind says what happened ("Ran a command") when the user allowed it.
    /// A denied or a closed request keeps the line of the request, because
    /// the action did not run.
    pub fn title_done(&self) -> String {
        let ran = self.state == RequestState::Answered
            && matches!(
                self.answer,
                Some(ApprovalChoice::AllowOnce | ApprovalChoice::AllowForTask)
            );
        if ran && self.summary_is_noise() {
            self.kind_title_done().to_owned()
        } else {
            self.title()
        }
    }

    /// The action cut to one line, for the line after the answer: the first
    /// line of the command or the target (with "…" when more follows), and
    /// "Search: <query>" for a web search. A request with no block uses its
    /// summary.
    pub fn action_line(&self) -> String {
        let detail = self.detail.as_deref().map(str::trim).unwrap_or_default();
        let summary = self.summary.trim();
        let first_line = |text: &str| {
            let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
            let first = lines.next().unwrap_or_default().to_owned();
            if lines.next().is_some() {
                format!("{first} …")
            } else {
                first
            }
        };
        match self.kind {
            ApprovalKind::Command if !detail.is_empty() => first_line(detail),
            ApprovalKind::Network if !detail.is_empty() && self.target_is_page() => {
                first_line(detail)
            }
            ApprovalKind::Network if !detail.is_empty() => {
                format!("Search: {}", first_line(detail))
            }
            _ if !summary.is_empty() => first_line(summary),
            _ => first_line(detail),
        }
    }

    /// The answer on the line after the answer.
    pub fn answer_label(&self) -> String {
        match self.state {
            RequestState::Pending | RequestState::Sending => "Needs you".to_owned(),
            RequestState::Expired | RequestState::Closed => "Closed, no answer".to_owned(),
            RequestState::Answered => match self.answer {
                Some(ApprovalChoice::AllowOnce) => "Allowed once".to_owned(),
                // With no task, the harness's wide choice lasts from now
                // on, and "for this task" would be wrong.
                Some(ApprovalChoice::AllowForTask) if self.task_id.is_none() => {
                    "Allowed from now on".to_owned()
                }
                Some(ApprovalChoice::AllowForTask) => "Allowed for this task".to_owned(),
                Some(ApprovalChoice::Deny) => "Denied".to_owned(),
                None => "Answered".to_owned(),
            },
        }
    }

    /// The harness's own words for the wide choice, when the user chose it.
    /// The scope is the harness's and can be wider than the task, so an
    /// opened line keeps the words that the button showed.
    pub fn scope_words(&self) -> Option<String> {
        if self.state != RequestState::Answered || self.answer != Some(ApprovalChoice::AllowForTask)
        {
            return None;
        }
        self.allow_always_label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(|label| format!("You chose: {label}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(summary: &str, detail: &str) -> ApprovalView {
        ApprovalView {
            summary: summary.into(),
            detail: Some(detail.into()),
            kind: ApprovalKind::Command,
            ..ApprovalView::default()
        }
    }

    #[test]
    fn a_summary_that_repeats_the_command_falls_back_to_the_kind() {
        let curl = "cd /tmp/x && curl -sS -o closing.pdf 'https://example.com/a.pdf'";
        // The same text, the text inside quotes, a longer line that holds
        // the command, and a start that the agent cut short.
        for summary in [
            curl,
            "`cd /tmp/x && curl -sS -o closing.pdf 'https://example.com/a.pdf'`",
            "Run: cd /tmp/x && curl -sS -o closing.pdf 'https://example.com/a.pdf'",
            "cd /tmp/x && curl -sS -o closing.pdf …",
            "cd   /tmp/x &&\n curl -sS -o closing.pdf 'https://example.com/a.pdf'",
            "",
        ] {
            assert_eq!(
                command(summary, curl).title(),
                "Run a command",
                "summary {summary:?}"
            );
        }
    }

    #[test]
    fn a_summary_with_its_own_words_stays_the_title() {
        let view = command("Delete files outside the workspace", "rm -rf ~/old-scans");
        assert_eq!(view.title(), "Delete files outside the workspace");
        let view = command("Codex wants to run a command", "cargo build --release");
        assert_eq!(view.title(), "Codex wants to run a command");
        // A short summary inside a long command is not a repeat.
        let view = command("Run", "Run something else entirely");
        assert_eq!(view.title(), "Run");
    }

    #[test]
    fn each_kind_has_a_fallback_title() {
        let titles = |kind, detail: Option<&str>| {
            ApprovalView {
                summary: detail.unwrap_or_default().into(),
                detail: detail.map(str::to_owned),
                kind,
                ..ApprovalView::default()
            }
            .title()
        };
        assert_eq!(titles(ApprovalKind::Command, Some("ls")), "Run a command");
        assert_eq!(
            titles(ApprovalKind::FileChange, Some("/a/b.txt")),
            "Change files"
        );
        assert_eq!(
            titles(ApprovalKind::Network, Some("iBex Home Warranty plans")),
            "Search the web"
        );
        assert_eq!(
            titles(ApprovalKind::Network, Some("https://example.com/terms")),
            "Open a web page"
        );
        assert_eq!(titles(ApprovalKind::Tool, None), "Use a tool");
        assert_eq!(
            titles(ApprovalKind::Other, None),
            "A request for your approval"
        );
    }

    #[test]
    fn the_title_after_an_allowed_action_is_in_the_past() {
        let mut view = command("ls -la", "ls -la");
        view.state = RequestState::Answered;
        view.answer = Some(ApprovalChoice::AllowOnce);
        assert_eq!(view.title_done(), "Ran a command");
        // A denied action did not run.
        view.answer = Some(ApprovalChoice::Deny);
        assert_eq!(view.title_done(), "Run a command");
        // A closed one did not run either.
        view.state = RequestState::Closed;
        view.answer = None;
        assert_eq!(view.title_done(), "Run a command");
        // A title with its own words does not change.
        let mut own = command("Delete the old scans", "rm -rf scans");
        own.state = RequestState::Answered;
        own.answer = Some(ApprovalChoice::AllowForTask);
        assert_eq!(own.title_done(), "Delete the old scans");
    }

    #[test]
    fn the_action_is_cut_to_one_line() {
        let view = command("ls", "cd /tmp/x &&\n curl -sS -o title.pdf");
        assert_eq!(view.action_line(), "cd /tmp/x && …");
        let view = command("ls", "  rm -rf ~/Documents/home-docs/old-scans  ");
        assert_eq!(view.action_line(), "rm -rf ~/Documents/home-docs/old-scans");
        let search = ApprovalView {
            summary: "Search".into(),
            detail: Some("iBex Home Warranty \"One Simple Plan\" Utah".into()),
            kind: ApprovalKind::Network,
            ..ApprovalView::default()
        };
        assert_eq!(
            search.action_line(),
            "Search: iBex Home Warranty \"One Simple Plan\" Utah"
        );
        let page = ApprovalView {
            detail: Some("https://example.com/terms".into()),
            kind: ApprovalKind::Network,
            ..ApprovalView::default()
        };
        assert_eq!(page.action_line(), "https://example.com/terms");
        let file = ApprovalView {
            summary: "Write hello.txt".into(),
            detail: Some("/work/hello.txt".into()),
            kind: ApprovalKind::FileChange,
            ..ApprovalView::default()
        };
        assert_eq!(file.action_line(), "Write hello.txt");
        // No block: the summary.
        let closed = ApprovalView {
            summary: "Consort wanted to use post_message from hub".into(),
            ..ApprovalView::default()
        };
        assert_eq!(
            closed.action_line(),
            "Consort wanted to use post_message from hub"
        );
    }

    #[test]
    fn the_answer_has_one_label_for_each_outcome() {
        let mut view = ApprovalView {
            task_id: Some("tsk_1".into()),
            state: RequestState::Answered,
            ..ApprovalView::default()
        };
        let mut label = |answer, state, task: bool| {
            view.answer = answer;
            view.state = state;
            view.task_id = task.then(|| "tsk_1".to_owned());
            view.answer_label()
        };
        let answered = RequestState::Answered;
        assert_eq!(
            label(Some(ApprovalChoice::AllowOnce), answered, true),
            "Allowed once"
        );
        assert_eq!(
            label(Some(ApprovalChoice::AllowForTask), answered, true),
            "Allowed for this task"
        );
        assert_eq!(
            label(Some(ApprovalChoice::AllowForTask), answered, false),
            "Allowed from now on"
        );
        assert_eq!(label(Some(ApprovalChoice::Deny), answered, true), "Denied");
        assert_eq!(label(None, answered, true), "Answered");
        assert_eq!(label(None, RequestState::Closed, true), "Closed, no answer");
        assert_eq!(
            label(None, RequestState::Expired, true),
            "Closed, no answer"
        );
        assert_eq!(label(None, RequestState::Pending, true), "Needs you");
    }

    #[test]
    fn the_scope_words_of_the_wide_choice_are_kept() {
        let view = ApprovalView {
            state: RequestState::Answered,
            answer: Some(ApprovalChoice::AllowForTask),
            allow_always_label: Some("  Yes, and don't ask again for cargo build commands ".into()),
            ..ApprovalView::default()
        };
        assert_eq!(
            view.scope_words().as_deref(),
            Some("You chose: Yes, and don't ask again for cargo build commands")
        );
        let once = ApprovalView {
            answer: Some(ApprovalChoice::AllowOnce),
            ..view.clone()
        };
        assert_eq!(once.scope_words(), None);
        let no_words = ApprovalView {
            allow_always_label: None,
            ..view
        };
        assert_eq!(no_words.scope_words(), None);
    }

    #[test]
    fn only_a_long_label_leaves_the_row_of_buttons() {
        assert!(!label_needs_own_row("Allow for this task"));
        assert!(label_needs_own_row(
            "Yes, and don't ask again for cargo build commands"
        ));
    }
}
