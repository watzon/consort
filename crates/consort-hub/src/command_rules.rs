//! Reads a shell command against the command rules of a channel. See ADR 0019.
//!
//! The two results read a command in different ways, and each way fails safe:
//!
//! - **Ask reads loosely.** It looks through wrappers, paths, variables and
//!   `sh -c`, and into text that it cannot parse. A false match only asks.
//! - **Allow reads strictly.** Anything that can change what the command does
//!   stops the match: a variable, a path to the program, a redirect, an
//!   interpreter or a command substitution. A missed match only falls through
//!   to the approval mode.
//!
//! This is a convenience and not a lock. A script can still do what a pattern
//! names. Worker policy, the OS boundary and the rules of the remote service
//! still hold.

use consort_proto::model::{CommandRule, CommandRuleResult};

/// What the rules say about one command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuleOutcome<'a> {
    /// The user decides, whatever the approval mode is.
    Ask(&'a CommandRule),
    /// The command runs. The rule is the first that allowed a part of it.
    Allow(&'a CommandRule),
    /// The rules say nothing, so the approval mode decides.
    NoRule,
}

/// Programs that run other text. They never match Allow, and Ask reads their
/// whole text.
const INTERPRETERS: [&str; 16] = [
    "bash", "sh", "zsh", "fish", "dash", "ksh", "eval", "source", ".", "python", "python3", "node",
    "perl", "ruby", "php", "xargs",
];
/// Programs that run the rest of the line. Ask looks through them. They never
/// match Allow.
const WRAPPERS: [&str; 7] = ["sudo", "env", "command", "time", "nohup", "exec", "nice"];
/// Commands that do not count against Allow, so `cd a && git push` can match.
const NEUTRAL: [&str; 4] = ["cd", "pushd", "popd", "true"];

/// One simple command of a line.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Part {
    words: Vec<String>,
    /// The tokenizer saw nothing that can change what the words mean.
    plain: bool,
}

#[derive(Debug, Default)]
struct Parsed {
    parts: Vec<Part>,
    /// Some text cannot be read as words, such as a heredoc.
    opaque: bool,
}

/// Splits a line into simple commands, and follows the quotes of a shell.
fn parse(command: &str) -> Parsed {
    let mut parsed = Parsed::default();
    let mut part = Part {
        plain: true,
        ..Part::default()
    };
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = command.chars().peekable();
    macro_rules! end_word {
        () => {
            if in_word {
                part.words.push(std::mem::take(&mut word));
                in_word = false;
            }
        };
    }
    macro_rules! end_part {
        () => {
            end_word!();
            if !part.words.is_empty() {
                parsed.parts.push(std::mem::replace(
                    &mut part,
                    Part {
                        plain: true,
                        ..Part::default()
                    },
                ));
            } else {
                part.plain = true;
            }
        };
    }
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                let mut closed = false;
                for q in chars.by_ref() {
                    if q == '\'' {
                        closed = true;
                        break;
                    }
                    word.push(q);
                }
                parsed.opaque |= !closed;
            }
            '"' => {
                in_word = true;
                let mut closed = false;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => {
                            closed = true;
                            break;
                        }
                        '\\' => {
                            if let Some(next) = chars.next() {
                                word.push(next);
                            }
                        }
                        '$' | '`' => {
                            parsed.opaque = true;
                            word.push(q);
                        }
                        _ => word.push(q),
                    }
                }
                parsed.opaque |= !closed;
            }
            '\\' => {
                // A backslash at the end of a line joins the two lines.
                match chars.next() {
                    Some('\n') | None => {}
                    Some(next) => {
                        in_word = true;
                        word.push(next);
                    }
                }
            }
            '$' | '`' => {
                parsed.opaque = true;
                in_word = true;
                word.push(c);
            }
            ' ' | '\t' => end_word!(),
            '\n' | '\r' | ';' | '|' | '&' | '(' | ')' | '{' | '}' => {
                end_part!();
            }
            '<' | '>' => {
                let next = chars.peek().copied();
                if c == '<' && matches!(next, Some('<' | '(')) || c == '>' && next == Some('(') {
                    parsed.opaque = true;
                }
                // `2>&1` changes nothing that a rule cares about.
                if c == '>' && next == Some('&') && word == "2" {
                    chars.next();
                    if chars.peek() == Some(&'1') {
                        chars.next();
                        word.clear();
                        in_word = false;
                        continue;
                    }
                    parsed.opaque = true;
                }
                // Any other redirect can write somewhere else. Its target is
                // not a word of the command.
                part.plain = false;
                if in_word && word.chars().all(|d| d.is_ascii_digit()) {
                    word.clear();
                    in_word = false;
                }
                end_word!();
                while chars.peek().is_some_and(|n| matches!(n, '<' | '>' | '&')) {
                    chars.next();
                }
                while chars.peek() == Some(&' ') {
                    chars.next();
                }
                while chars
                    .peek()
                    .is_some_and(|n| !n.is_whitespace() && !matches!(n, ';' | '|' | '&'))
                {
                    chars.next();
                }
            }
            _ => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        part.words.push(word);
    }
    if !part.words.is_empty() {
        parsed.parts.push(part);
    }
    parsed
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with('-')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn base_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// The words after the global options of git, with the program first. The
/// bool says whether every option is safe for Allow: `-c` can point git at
/// another program.
fn strip_git_options(words: &[String]) -> (Vec<String>, bool) {
    let mut safe = true;
    let mut out = vec![words[0].clone()];
    let mut i = 1;
    while let Some(word) = words.get(i) {
        match word.as_str() {
            "-C" => i += 2,
            "--no-pager" | "-P" => i += 1,
            "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--exec-path" => {
                safe = false;
                i += 2;
            }
            w if w.starts_with('-') => {
                safe = false;
                i += 1;
            }
            _ => break,
        }
    }
    out.extend(words.iter().skip(i).cloned());
    (out, safe)
}

/// One part, as Ask reads it and as Allow reads it.
struct Reading {
    loose: Vec<String>,
    /// `None` when something stops every Allow rule.
    strict: Option<Vec<String>>,
}

/// Reads one part. Text inside `sh -c` adds its own parts to `nested`.
fn read_part(part: &Part, parsed_opaque: &mut bool, nested: &mut Vec<Reading>) -> Reading {
    let mut words = part.words.as_slice();
    let mut strict_ok = part.plain && !*parsed_opaque;
    while words.first().is_some_and(|w| is_assignment(w)) {
        strict_ok = false;
        words = &words[1..];
    }
    while let Some(first) = words.first()
        && WRAPPERS.contains(&base_name(first))
    {
        strict_ok = false;
        words = &words[1..];
        while words
            .first()
            .is_some_and(|w| w.starts_with('-') || is_assignment(w))
        {
            words = &words[1..];
        }
    }
    let Some(first) = words.first() else {
        return Reading {
            loose: Vec::new(),
            strict: None,
        };
    };
    if first.contains('/') {
        strict_ok = false;
    }
    let program = base_name(first).to_owned();
    if INTERPRETERS.contains(&program.as_str()) {
        strict_ok = false;
        // `sh -c "text"`: the text is a command line of its own. Any other
        // use runs text that the hub cannot see, so Ask reads the whole line.
        let script = words
            .iter()
            .position(|w| w == "-c")
            .filter(|_| matches!(program.as_str(), "bash" | "sh" | "zsh" | "dash" | "ksh"))
            .and_then(|i| words.get(i + 1));
        match script {
            Some(script) => {
                let inner = parse(script);
                *parsed_opaque |= inner.opaque;
                let mut opaque = true;
                for part in &inner.parts {
                    let reading = read_part(part, &mut opaque, nested);
                    nested.push(reading);
                }
            }
            None => *parsed_opaque = true,
        }
    }
    let mut named: Vec<String> = vec![program.clone()];
    named.extend(words.iter().skip(1).cloned());
    if program == "git" {
        let (stripped, safe) = strip_git_options(&named);
        named = stripped;
        strict_ok &= safe;
    }
    Reading {
        strict: strict_ok.then(|| named.clone()),
        loose: named,
    }
}

/// Whether a word of a command is this flag or word of a rule. It knows
/// `--force=x`, `-fu` for `-f`, `HEAD:main` for `main`, and `+main` for a
/// force flag.
fn word_hits(word: &str, wanted: &str) -> bool {
    if word == wanted
        || word
            .strip_prefix(wanted)
            .is_some_and(|r| r.starts_with('='))
    {
        return true;
    }
    if wanted.starts_with("--force") && word.starts_with('+') {
        return true;
    }
    if let Some(flag) = wanted.strip_prefix('-')
        && flag.len() == 1
        && let Some(cluster) = word.strip_prefix('-')
        && !cluster.starts_with('-')
        && cluster.contains(flag)
    {
        return true;
    }
    !wanted.starts_with('-') && word.rsplit_once(':').is_some_and(|(_, to)| to == wanted)
}

/// Whether a rule matches the words of one part. `exact`: the words of the
/// pattern come first, right after the program. Without it they can have
/// other words between them, such as `gh -R a/b pr merge`.
fn rule_matches(rule: &CommandRule, words: &[String], exact: bool) -> bool {
    let mut pattern = rule.pattern.split(' ');
    let (Some(program), Some((_, args))) = (pattern.next(), words.split_first()) else {
        return false;
    };
    if words[0] != program {
        return false;
    }
    if rule
        .except
        .iter()
        .any(|ex| args.iter().any(|w| word_hits(w, ex)))
    {
        return false;
    }
    let (flags, names): (Vec<&str>, Vec<&str>) = pattern.partition(|w| w.starts_with('-'));
    if !flags
        .iter()
        .all(|flag| args.iter().any(|w| word_hits(w, flag)))
    {
        return false;
    }
    if exact {
        names.len() <= args.len() && names.iter().zip(args).all(|(n, w)| *n == w)
    } else {
        let mut rest = args.iter();
        names.iter().all(|n| rest.any(|w| w == n))
    }
}

/// The result of one part: the most specific rule wins, and Ask wins a tie.
fn part_outcome<'a>(reading: &Reading, rules: &'a [CommandRule]) -> Option<&'a CommandRule> {
    rules
        .iter()
        .filter(|rule| match rule.result {
            CommandRuleResult::Ask => rule_matches(rule, &reading.loose, false),
            CommandRuleResult::Allow => reading
                .strict
                .as_ref()
                .is_some_and(|words| rule_matches(rule, words, true)),
        })
        .max_by_key(|rule| {
            (
                rule.pattern.split(' ').count(),
                rule.result == CommandRuleResult::Ask,
            )
        })
}

/// The rule of a text that cannot be read as words: an Ask rule whose words
/// all show up in order.
fn opaque_ask<'a>(command: &str, rules: &'a [CommandRule]) -> Option<&'a CommandRule> {
    let words: Vec<&str> = command
        .split(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | ';' | '(' | ')' | '`'))
        .filter(|w| !w.is_empty())
        .collect();
    rules
        .iter()
        .filter(|r| r.result == CommandRuleResult::Ask)
        .find(|rule| {
            let mut rest = words.iter();
            rule.pattern
                .split(' ')
                .all(|p| rest.any(|w| *w == p || base_name(w) == p))
        })
}

/// What the rules say about one command line.
pub(crate) fn evaluate<'a>(command: &str, rules: &'a [CommandRule]) -> RuleOutcome<'a> {
    if rules.is_empty() {
        return RuleOutcome::NoRule;
    }
    let parsed = parse(command);
    let mut opaque = parsed.opaque;
    let mut nested = Vec::new();
    let mut readings: Vec<Reading> = parsed
        .parts
        .iter()
        .map(|part| read_part(part, &mut opaque, &mut nested))
        .collect();
    readings.append(&mut nested);
    let outcomes: Vec<(&Reading, Option<&CommandRule>)> = readings
        .iter()
        .map(|reading| (reading, part_outcome(reading, rules)))
        .collect();
    if let Some(rule) = outcomes
        .iter()
        .filter_map(|(_, rule)| *rule)
        .find(|rule| rule.result == CommandRuleResult::Ask)
    {
        return RuleOutcome::Ask(rule);
    }
    if opaque {
        return opaque_ask(command, rules).map_or(RuleOutcome::NoRule, RuleOutcome::Ask);
    }
    let mut allowed = None;
    for (reading, rule) in &outcomes {
        match rule {
            Some(rule) => allowed = allowed.or(Some(*rule)),
            None => {
                let neutral = reading.strict.as_ref().is_some_and(|words| {
                    words.first().is_some_and(|w| NEUTRAL.contains(&w.as_str()))
                });
                if !neutral {
                    return RuleOutcome::NoRule;
                }
            }
        }
    }
    allowed.map_or(RuleOutcome::NoRule, RuleOutcome::Allow)
}

/// The rules that count in a channel: its own, and those of Tutti whose
/// pattern it does not have. In Tutti itself `tutti` is empty.
pub(crate) fn effective_rules(tutti: &[CommandRule], channel: &[CommandRule]) -> Vec<CommandRule> {
    let mut rules = channel.to_vec();
    rules.extend(
        tutti
            .iter()
            .filter(|t| !channel.iter().any(|c| c.pattern == t.pattern))
            .cloned(),
    );
    rules
}

/// Whether a new list gives more room than the old one, so the user must allow
/// it first: a new or changed Allow rule, a rule that stopped asking, or a
/// wider exception list on an Ask rule.
pub(crate) fn widens(old: &[CommandRule], new: &[CommandRule]) -> bool {
    let added_room = new.iter().any(|rule| {
        let before = old.iter().find(|o| o.pattern == rule.pattern);
        match rule.result {
            CommandRuleResult::Allow => before.is_none_or(|o| {
                o.result != CommandRuleResult::Allow
                    || o.except.iter().any(|e| !rule.except.contains(e))
            }),
            CommandRuleResult::Ask => before.is_some_and(|o| {
                o.result == CommandRuleResult::Ask
                    && rule.except.iter().any(|e| !o.except.contains(e))
            }),
        }
    });
    let stopped_asking = old
        .iter()
        .filter(|o| o.result == CommandRuleResult::Ask)
        .any(|o| !new.iter().any(|n| n.pattern == o.pattern));
    added_room || stopped_asking
}

#[cfg(test)]
mod tests {
    use consort_proto::model::CommandRuleSource;

    use super::*;

    fn rule(pattern: &str, except: &[&str], result: CommandRuleResult) -> CommandRule {
        CommandRule {
            pattern: pattern.to_owned(),
            except: except.iter().map(|e| (*e).to_owned()).collect(),
            result,
            description: "A rule".to_owned(),
            source: CommandRuleSource::Assistant,
        }
    }

    /// The rules of "PR anything you want, just ask for merges".
    fn pr_rules() -> Vec<CommandRule> {
        use CommandRuleResult::{Allow, Ask};
        vec![
            rule(
                "git push",
                &[
                    "--force",
                    "--force-with-lease",
                    "-f",
                    "--delete",
                    "-d",
                    "main",
                ],
                Allow,
            ),
            rule("gh pr create", &[], Allow),
            rule("gh pr merge", &[], Ask),
        ]
    }

    fn says(command: &str) -> &'static str {
        let rules = pr_rules();
        match evaluate(command, &rules) {
            RuleOutcome::Ask(_) => "ask",
            RuleOutcome::Allow(_) => "allow",
            RuleOutcome::NoRule => "none",
        }
    }

    #[test]
    fn plain_commands_follow_the_rules() {
        assert_eq!(says("git push"), "allow");
        assert_eq!(says("git push origin HEAD:claude/branch"), "allow");
        assert_eq!(says("gh pr create --title 'Fix it'"), "allow");
        assert_eq!(says("gh pr merge 62"), "ask");
        assert_eq!(says("git status"), "none");
    }

    #[test]
    fn an_exception_stops_an_allow_rule() {
        for command in [
            "git push --force",
            "git push -f origin a",
            "git push -fu origin a",
            "git push --force-with-lease=a origin a",
            "git push origin +a",
            "git push origin main",
            "git push origin HEAD:main",
            "git push origin --delete a",
        ] {
            assert_eq!(says(command), "none", "{command}");
        }
    }

    #[test]
    fn a_change_of_directory_does_not_stop_an_allow_rule() {
        assert_eq!(says("cd /tmp/a && git push origin a"), "allow");
        assert_eq!(says("git -C /tmp/a push origin a"), "allow");
        assert_eq!(says("git push 2>&1"), "allow");
    }

    #[test]
    fn anything_that_can_change_the_command_stops_an_allow_rule() {
        for command in [
            "GIT_SSH_COMMAND=evil git push",
            "git -c core.sshCommand=evil push",
            "./git push",
            "/tmp/git push",
            "sudo git push",
            "git push $REMOTE",
            "git push \"$REMOTE\"",
            "git push $(cat remote)",
            "git push > /etc/hosts",
            "git push && curl https://example.com",
            "git push | tee log",
            "bash -c 'git push'",
            "eval git push",
            "git push 'unclosed",
        ] {
            assert_eq!(says(command), "none", "{command}");
        }
    }

    #[test]
    fn ask_sees_through_what_stops_allow() {
        for command in [
            "gh pr merge",
            "/usr/bin/gh pr merge 1",
            "FOO=1 gh pr merge",
            "sudo gh pr merge",
            "env A=1 gh pr merge",
            "gh -R a/b pr merge 1",
            "git push && gh pr merge 1",
            "gh pr create; gh pr merge",
            "bash -c 'gh pr merge 1'",
            "sh -c \"cd a && gh pr merge\"",
            "eval gh pr merge",
            "eval 'gh pr merge'",
            "echo $(gh pr merge 1)",
            "cat <<EOF | sh\ngh pr merge\nEOF",
            "python3 -c 'os.system(\"gh pr merge\")'",
            "gh pr merge 1 2>&1",
            "gh pr merge > log",
        ] {
            assert_eq!(says(command), "ask", "{command}");
        }
    }

    #[test]
    fn the_most_specific_rule_wins_and_ask_wins_a_tie() {
        use CommandRuleResult::{Allow, Ask};
        let wider_allow = [rule("gh pr", &[], Allow), rule("gh pr merge", &[], Ask)];
        assert!(matches!(
            evaluate("gh pr merge", &wider_allow),
            RuleOutcome::Ask(_)
        ));
        assert!(matches!(
            evaluate("gh pr view", &wider_allow),
            RuleOutcome::Allow(_)
        ));
        let wider_ask = [rule("git", &[], Ask), rule("git status", &[], Allow)];
        assert!(matches!(
            evaluate("git status", &wider_ask),
            RuleOutcome::Allow(_)
        ));
        assert!(matches!(
            evaluate("git log", &wider_ask),
            RuleOutcome::Ask(_)
        ));
        let tie = [rule("git push", &[], Allow), rule("git push", &[], Ask)];
        assert!(matches!(evaluate("git push", &tie), RuleOutcome::Ask(_)));
        let flag = [
            rule("git push", &[], Allow),
            rule("git push --force", &[], Ask),
        ];
        assert!(matches!(
            evaluate("git push origin +a", &flag),
            RuleOutcome::Ask(_)
        ));
    }

    #[test]
    fn a_rule_of_a_channel_replaces_the_rule_of_tutti_for_its_pattern() {
        use CommandRuleResult::{Allow, Ask};
        let tutti = [rule("git push", &[], Ask), rule("gh pr merge", &[], Ask)];
        let channel = [rule("git push", &[], Allow)];
        let rules = effective_rules(&tutti, &channel);
        assert!(matches!(
            evaluate("git push", &rules),
            RuleOutcome::Allow(_)
        ));
        assert!(matches!(
            evaluate("gh pr merge", &rules),
            RuleOutcome::Ask(_)
        ));
    }

    #[test]
    fn only_more_room_needs_the_user() {
        use CommandRuleResult::{Allow, Ask};
        let push = rule("git push", &["--force"], Allow);
        let merge = rule("gh pr merge", &[], Ask);
        // Stricter: a new Ask rule, a removed Allow rule, more exceptions, or
        // an Allow rule that now asks.
        assert!(!widens(&[], std::slice::from_ref(&merge)));
        assert!(!widens(std::slice::from_ref(&push), &[]));
        assert!(!widens(
            std::slice::from_ref(&push),
            &[rule("git push", &["--force", "main"], Allow)]
        ));
        assert!(!widens(
            std::slice::from_ref(&push),
            &[rule("git push", &[], Ask)]
        ));
        assert!(!widens(
            std::slice::from_ref(&push),
            std::slice::from_ref(&push)
        ));
        // Wider: a new Allow rule, fewer exceptions, a removed Ask rule, an
        // Ask rule that now allows, or more exceptions on an Ask rule.
        assert!(widens(&[], std::slice::from_ref(&push)));
        assert!(widens(
            std::slice::from_ref(&push),
            &[rule("git push", &[], Allow)]
        ));
        assert!(widens(std::slice::from_ref(&merge), &[]));
        assert!(widens(
            std::slice::from_ref(&merge),
            &[rule("gh pr merge", &[], Allow)]
        ));
        assert!(widens(
            std::slice::from_ref(&merge),
            &[rule("gh pr merge", &["--squash"], Ask)]
        ));
    }
}
