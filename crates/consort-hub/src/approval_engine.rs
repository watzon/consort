//! The approval table of the hub: what a tool call of a task session needs
//! in each approval mode. Worker policy is a separate upper limit, and the
//! worker checks it again. See `docs/HUB-PLAN.md`, "Approval modes".

use consort_proto::execution_link::ToolClass;
use consort_proto::model::ApprovalMode;

/// What the table says about one tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rule {
    /// The call runs.
    Allow,
    /// The user decides.
    Ask,
    /// The command check decides.
    Check,
}

/// The approval table.
pub(crate) fn rule_for(class: ToolClass, mode: ApprovalMode) -> Rule {
    use ApprovalMode::{Ask, Auto, AutoEdits, FullAccess};
    match (class, mode) {
        (ToolClass::Read, _) | (_, FullAccess) => Rule::Allow,
        (ToolClass::Write, Ask) => Rule::Ask,
        (ToolClass::Write, AutoEdits | Auto) => Rule::Allow,
        (ToolClass::Remove, Ask | AutoEdits | Auto) => Rule::Ask,
        (ToolClass::Shell | ToolClass::LocalMcp, Ask | AutoEdits) => Rule::Ask,
        (ToolClass::Shell | ToolClass::LocalMcp, Auto) => Rule::Check,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ApprovalMode::{Ask, Auto, AutoEdits, FullAccess};

    #[test]
    fn the_table_of_the_plan() {
        let cases = [
            (
                ToolClass::Read,
                [Rule::Allow, Rule::Allow, Rule::Allow, Rule::Allow],
            ),
            (
                ToolClass::Write,
                [Rule::Ask, Rule::Allow, Rule::Allow, Rule::Allow],
            ),
            (
                ToolClass::Remove,
                [Rule::Ask, Rule::Ask, Rule::Ask, Rule::Allow],
            ),
            (
                ToolClass::Shell,
                [Rule::Ask, Rule::Ask, Rule::Check, Rule::Allow],
            ),
            (
                ToolClass::LocalMcp,
                [Rule::Ask, Rule::Ask, Rule::Check, Rule::Allow],
            ),
        ];
        for (class, expected) in cases {
            for (mode, rule) in [Ask, AutoEdits, Auto, FullAccess].into_iter().zip(expected) {
                assert_eq!(rule_for(class, mode), rule, "{class:?} in {mode:?}");
            }
        }
    }
}
