//! Command rules: what a channel lets a shell command do without the usual
//! approval, and what it always asks about. See ADR 0019.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The most rules in one channel.
pub const COMMAND_RULES_MAX: usize = 100;
/// The most characters in a pattern or in one exception.
pub const COMMAND_RULE_PATTERN_MAX_CHARS: usize = 200;
/// The most exceptions in one rule.
pub const COMMAND_RULE_EXCEPT_MAX: usize = 20;
/// The most characters in a description.
pub const COMMAND_RULE_DESCRIPTION_MAX_CHARS: usize = 300;

/// What a matching rule does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandRuleResult {
    /// The command runs without asking.
    Allow,
    /// The user decides, in every approval mode.
    Ask,
}

/// Who added a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandRuleSource {
    /// The user, by hand.
    #[default]
    User,
    /// An assistant. The user allowed it when the rule makes a channel less
    /// strict.
    Assistant,
}

/// One rule, as a person or an assistant gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandRuleInput {
    /// The program and the words after it, such as `git push` or
    /// `gh pr merge`. It is no shell text and no regular expression.
    pub pattern: String,
    /// Words that stop the rule from matching, such as `--force` or `main`.
    #[serde(default)]
    pub except: Vec<String>,
    /// `allow` or `ask`.
    pub result: CommandRuleResult,
    /// One plain sentence for the user, such as "Push to a branch".
    pub description: String,
}

/// One rule of a channel. The rules of Tutti apply in every channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRule {
    /// The pattern, with single spaces. It is unique in a channel.
    pub pattern: String,
    /// Words that stop the rule from matching.
    #[serde(default)]
    pub except: Vec<String>,
    /// What a match does.
    pub result: CommandRuleResult,
    /// One plain sentence for the user.
    pub description: String,
    /// Who added the rule.
    #[serde(default)]
    pub source: CommandRuleSource,
}

/// Why a rule is not valid. The text goes to the user or the assistant.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidCommandRule(pub String);

impl CommandRuleInput {
    /// Checks the limits and gives the rule with a clean pattern.
    pub fn into_rule(self, source: CommandRuleSource) -> Result<CommandRule, InvalidCommandRule> {
        let pattern = self
            .pattern
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if pattern.is_empty() {
            return Err(InvalidCommandRule("A command rule needs a pattern.".into()));
        }
        if pattern.chars().count() > COMMAND_RULE_PATTERN_MAX_CHARS {
            return Err(InvalidCommandRule(format!(
                "The pattern of a command rule can have {COMMAND_RULE_PATTERN_MAX_CHARS} characters at most."
            )));
        }
        if pattern.contains(['&', '|', ';', '<', '>', '$', '`', '(', ')', '*', '\'', '"']) {
            return Err(InvalidCommandRule(format!(
                "The pattern `{pattern}` has shell text. Give only the program and its words."
            )));
        }
        if self.except.len() > COMMAND_RULE_EXCEPT_MAX
            || self
                .except
                .iter()
                .any(|e| e.trim().is_empty() || e.chars().count() > COMMAND_RULE_PATTERN_MAX_CHARS)
        {
            return Err(InvalidCommandRule(format!(
                "The exceptions of `{pattern}` are empty or too long."
            )));
        }
        let description = self.description.trim().to_owned();
        if description.is_empty()
            || description.chars().count() > COMMAND_RULE_DESCRIPTION_MAX_CHARS
        {
            return Err(InvalidCommandRule(format!(
                "The rule `{pattern}` needs a description of {COMMAND_RULE_DESCRIPTION_MAX_CHARS} characters at most."
            )));
        }
        Ok(CommandRule {
            pattern,
            except: self.except.iter().map(|e| e.trim().to_owned()).collect(),
            result: self.result,
            description,
            source,
        })
    }
}

impl CommandRule {
    /// A line that says what the rule does, for a card and for the command
    /// check.
    pub fn summary(&self) -> String {
        let result = match self.result {
            CommandRuleResult::Allow => "Allow",
            CommandRuleResult::Ask => "Ask",
        };
        let except = if self.except.is_empty() {
            String::new()
        } else {
            format!(", except {}", self.except.join(", "))
        };
        format!("{result} `{}`{except}: {}", self.pattern, self.description)
    }
}

/// Checks a whole list of rules for one channel.
pub fn validate_command_rules(rules: &[CommandRule]) -> Result<(), InvalidCommandRule> {
    if rules.len() > COMMAND_RULES_MAX {
        return Err(InvalidCommandRule(format!(
            "A channel can have {COMMAND_RULES_MAX} command rules at most."
        )));
    }
    for (i, rule) in rules.iter().enumerate() {
        if rules[..i].iter().any(|r| r.pattern == rule.pattern) {
            return Err(InvalidCommandRule(format!(
                "The channel has two rules for `{}`.",
                rule.pattern
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(pattern: &str) -> CommandRuleInput {
        CommandRuleInput {
            pattern: pattern.to_owned(),
            except: Vec::new(),
            result: CommandRuleResult::Ask,
            description: "Merge a PR".to_owned(),
        }
    }

    #[test]
    fn a_rule_has_a_clean_pattern_and_no_shell_text() {
        let rule = input("  gh   pr merge ")
            .into_rule(CommandRuleSource::Assistant)
            .unwrap();
        assert_eq!(rule.pattern, "gh pr merge");
        for bad in ["", "git push; rm", "git $X", "gi*"] {
            assert!(
                input(bad).into_rule(CommandRuleSource::User).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_channel_has_one_rule_for_a_pattern() {
        let rule = input("gh pr merge")
            .into_rule(CommandRuleSource::User)
            .unwrap();
        assert!(validate_command_rules(std::slice::from_ref(&rule)).is_ok());
        assert!(validate_command_rules(&[rule.clone(), rule]).is_err());
    }
}
