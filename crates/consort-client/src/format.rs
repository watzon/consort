//! Labels made from times, in the user's time zone, and from platform names.

use chrono::{DateTime, Datelike, Local, TimeZone};
use markdown::{
    ParseOptions,
    mdast::{self, Node},
};

fn local(ms: i64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(ms).single()
}

/// The architecture as the detected tags name it: `arm64` or `x64`. A worker
/// reports the Rust names `aarch64` and `x86_64`. Other names stay as they are.
pub fn arch_label(arch: &str) -> &str {
    match arch {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    }
}

/// "9:12 AM".
pub fn time_label(ms: i64) -> String {
    local(ms)
        .map(|t| t.format("%-I:%M %p").to_string())
        .unwrap_or_default()
}

/// The size of something in bytes, in a few characters: "0 B", "512 B",
/// "1.2 KB", "3.4 MB".
pub fn size_label(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let kb = bytes as f64 / KB;
    if kb < KB {
        return format!("{kb:.1} KB");
    }
    format!("{:.1} MB", kb / KB)
}

/// The size of a file, as the file chips and the Files tab show it: "512 B",
/// "6 KB", "48 KB", "1.2 MB", "14.2 MB", "1.1 GB". Kilobytes have no
/// decimal; megabytes and gigabytes have one.
pub fn file_size_label(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let kb = bytes as f64 / KB;
    if kb < KB {
        return format!("{} KB", kb.round().max(1.0) as u64);
    }
    let mb = kb / KB;
    if mb < KB {
        return format!("{mb:.1} MB");
    }
    format!("{:.1} GB", mb / KB)
}

/// "Today", "Yesterday", "Tuesday" within a week, then "October 12".
pub fn day_label(ms: i64, now_ms: i64) -> String {
    let (Some(day), Some(now)) = (local(ms), local(now_ms)) else {
        return String::new();
    };
    let days = now
        .date_naive()
        .signed_duration_since(day.date_naive())
        .num_days();
    match days {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => day.format("%A").to_string(),
        _ if day.year() == now.year() => day.format("%B %-d").to_string(),
        _ => day.format("%B %-d, %Y").to_string(),
    }
}

/// A time in the future: "today at 3:40 PM", "tomorrow at 3:40 PM", or the
/// short date and time ("Oct 31, 3:40 PM") for a later day.
pub fn when_label(ms: i64, now_ms: i64) -> String {
    let (Some(day), Some(now)) = (local(ms), local(now_ms)) else {
        return String::new();
    };
    let clock = day.format("%-I:%M %p").to_string();
    match day
        .date_naive()
        .signed_duration_since(now.date_naive())
        .num_days()
    {
        0 => format!("today at {clock}"),
        1 => format!("tomorrow at {clock}"),
        _ => day.format("%b %-d, %-I:%M %p").to_string(),
    }
}

/// The date of a file: "Today" for today, else the short date ("Oct 24"),
/// with the year when it is not this year ("Oct 24, 2025").
pub fn file_date_label(ms: i64, now_ms: i64) -> String {
    let (Some(day), Some(now)) = (local(ms), local(now_ms)) else {
        return String::new();
    };
    if day.date_naive() == now.date_naive() {
        "Today".into()
    } else if day.year() == now.year() {
        day.format("%b %-d").to_string()
    } else {
        day.format("%b %-d, %Y").to_string()
    }
}

/// "20:41": the local time on a 24-hour clock, as the routine and memory
/// designs show times.
pub fn clock_24(ms: i64) -> String {
    local(ms)
        .map(|t| t.format("%-H:%M").to_string())
        .unwrap_or_default()
}

/// "Oct 27, 20:14": the local date and time on a 24-hour clock.
pub fn local_date_time(ms: i64) -> String {
    local(ms)
        .map(|t| t.format("%b %-d, %-H:%M").to_string())
        .unwrap_or_default()
}

/// The local day of a time, to group messages under day labels.
pub fn day_key(ms: i64) -> String {
    local(ms)
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// "Now", "2 min", "3 h", "4 d".
pub fn age_label(ms: i64, now_ms: i64) -> String {
    let minutes = (now_ms - ms).max(0) / 60_000;
    match minutes {
        0 => "Now".into(),
        1..=59 => format!("{minutes} min"),
        60..=1439 => format!("{} h", minutes / 60),
        _ => format!("{} d", minutes / 1440),
    }
}

/// Parses Markdown the way the text view does (`markdown` with the GFM
/// options), or returns `None` when the parser fails. Math is off: a text
/// such as "$5 and $10" is a price list, not a formula, and the text view
/// shows it as prose too.
fn parse(markdown: &str) -> Option<Node> {
    let mut options = ParseOptions::gfm();
    options.constructs.math_text = false;
    options.constructs.math_flow = false;
    markdown::to_mdast(markdown, &options).ok()
}

/// The text of inline nodes, as the reader sees it. Emphasis marks and link
/// targets are gone. A code span keeps every character it holds, and so do
/// the characters that the parser reads as plain text: a lone `*`, the
/// brackets of `arr[0]`, and a name such as `my_script.sh`.
fn inline_text(nodes: &[Node], out: &mut String) {
    for node in nodes {
        match node {
            Node::Text(n) => out.push_str(&n.value),
            Node::InlineCode(n) => out.push_str(&n.value),
            Node::InlineMath(n) => out.push_str(&n.value),
            Node::Html(n) => out.push_str(&n.value),
            Node::Break(_) => out.push('\n'),
            Node::Image(n) => out.push_str(&n.alt),
            Node::ImageReference(n) => out.push_str(&n.alt),
            Node::FootnoteReference(n) => {
                out.push_str("[^");
                out.push_str(n.label.as_deref().unwrap_or(&n.identifier));
                out.push(']');
            }
            other => {
                if let Some(children) = other.children() {
                    inline_text(children, out);
                }
            }
        }
    }
}

fn inline_of(node: &Node) -> String {
    let mut out = String::new();
    if let Some(children) = node.children() {
        inline_text(children, &mut out);
    }
    out
}

/// The lines of a list, with a mark for each item and two spaces for each
/// level of nesting.
fn list_lines(list: &mdast::List, depth: usize, out: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    for (i, item) in list.children.iter().enumerate() {
        let Node::ListItem(item) = item else {
            continue;
        };
        let mark = if list.ordered {
            format!("{}. ", list.start.unwrap_or(1) as usize + i)
        } else {
            "- ".to_owned()
        };
        let check = match item.checked {
            Some(true) => "[x] ",
            Some(false) => "[ ] ",
            None => "",
        };
        let mut first = true;
        for child in &item.children {
            match child {
                Node::List(nested) => list_lines(nested, depth + 1, out),
                other => {
                    let mut blocks = Vec::new();
                    block_texts(other, &mut blocks);
                    for text in blocks {
                        for (n, line) in text.lines().enumerate() {
                            if first && n == 0 {
                                out.push(format!("{indent}{mark}{check}{line}"));
                            } else {
                                out.push(format!("{indent}  {line}"));
                            }
                        }
                        first = false;
                    }
                }
            }
        }
        if first {
            out.push(format!("{indent}{mark}{check}").trim_end().to_owned());
        }
    }
}

/// The blocks of Markdown as plain text, one string for each block. A list
/// is one block, with a line for each item.
fn block_texts(node: &Node, out: &mut Vec<String>) {
    match node {
        Node::Root(_) | Node::Blockquote(_) | Node::FootnoteDefinition(_) | Node::ListItem(_) => {
            for child in node.children().into_iter().flatten() {
                block_texts(child, out);
            }
        }
        Node::Paragraph(_) | Node::Heading(_) => {
            let text = inline_of(node);
            if !text.trim().is_empty() {
                out.push(text);
            }
        }
        // Code is text as it is: no mark comes off.
        Node::Code(n) => out.push(n.value.clone()),
        Node::Math(n) => out.push(n.value.clone()),
        Node::Html(n) => out.push(n.value.clone()),
        Node::List(list) => {
            let mut lines = Vec::new();
            list_lines(list, 0, &mut lines);
            out.push(lines.join("\n"));
        }
        Node::Table(table) => {
            let rows: Vec<String> = table
                .children
                .iter()
                .map(|row| {
                    row.children()
                        .into_iter()
                        .flatten()
                        .map(inline_of)
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .collect();
            out.push(rows.join("\n"));
        }
        // Nothing of these shows in the text.
        Node::Definition(_) | Node::ThematicBreak(_) | Node::Yaml(_) | Node::Toml(_) => {}
        inline => {
            let mut text = String::new();
            inline_text(std::slice::from_ref(inline), &mut text);
            if !text.trim().is_empty() {
                out.push(text);
            }
        }
    }
}

/// Markdown as plain text, for "Copy text". It is the text that the reader
/// sees: emphasis and heading marks are gone, a link keeps its words, and
/// blocks are separated by a blank line. Code spans and code blocks keep
/// every character, so `rm -rf build/*` and `__init__.py` copy as they were
/// written. The text comes from the parsed Markdown, so a lone `*` or the
/// brackets of `arr[0]` stay too. If the parser fails, the source is
/// returned as it is.
pub fn plain_text(markdown: &str) -> String {
    let Some(root) = parse(markdown) else {
        return markdown.to_owned();
    };
    let mut blocks = Vec::new();
    block_texts(&root, &mut blocks);
    blocks.join("\n\n")
}

/// Markdown as plain text for a short summary on a card, one block to a
/// line. Headings and list marks go, emphasis and code marks go, and a link
/// keeps its words. A paragraph is one line, even when the source wraps it.
/// A heading, a list item, a quote, and each line of a code block (fenced or
/// indented) is a line of its own, so the lines of a code block never run
/// together with the text around them. The card clamps the lines.
pub fn card_summary(markdown: &str) -> String {
    let Some(root) = parse(markdown) else {
        return markdown.split_whitespace().collect::<Vec<_>>().join(" ");
    };
    let mut lines = Vec::new();
    summary_lines(&root, &mut lines);
    lines.join("\n")
}

fn summary_lines(node: &Node, out: &mut Vec<String>) {
    match node {
        Node::Root(_)
        | Node::Blockquote(_)
        | Node::List(_)
        | Node::ListItem(_)
        | Node::FootnoteDefinition(_) => {
            for child in node.children().into_iter().flatten() {
                summary_lines(child, out);
            }
        }
        Node::Paragraph(_) | Node::Heading(_) => {
            let text = inline_of(node).replace('\n', " ");
            let text = text.trim();
            if !text.is_empty() {
                out.push(text.to_owned());
            }
        }
        Node::Code(n) => out.extend(
            n.value
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned),
        ),
        other => {
            let mut blocks = Vec::new();
            block_texts(other, &mut blocks);
            for text in blocks {
                out.extend(
                    text.lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .map(str::to_owned),
                );
            }
        }
    }
}

/// The time now, in Unix milliseconds.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_size_reads_in_a_few_characters() {
        assert_eq!(super::file_size_label(512), "512 B");
        assert_eq!(super::file_size_label(48 * 1024), "48 KB");
        assert_eq!(super::file_size_label(380 * 1024), "380 KB");
        assert_eq!(super::file_size_label(2_516_582), "2.4 MB");
        assert_eq!(super::file_size_label(3 * 1024 * 1024 * 1024), "3.0 GB");
        assert_eq!(super::size_label(0), "0 B");
        assert_eq!(super::size_label(512), "512 B");
        assert_eq!(super::size_label(1536), "1.5 KB");
        assert_eq!(super::size_label(3 * 1024 * 1024 + 400_000), "3.4 MB");
    }

    use super::*;

    #[test]
    fn ages_round_down() {
        let now = 1_000_000_000_000;
        assert_eq!(age_label(now - 30_000, now), "Now");
        assert_eq!(age_label(now - 2 * 60_000, now), "2 min");
        assert_eq!(age_label(now - 3 * 3_600_000 - 5, now), "3 h");
        assert_eq!(age_label(now - 50 * 3_600_000, now), "2 d");
    }

    #[test]
    fn card_summaries_are_clean_text_with_one_block_to_a_line() {
        let md = "Wrote **hello.txt** in the workspace.\n\n```sh\ncat hello.txt\n```\n\n- It says [hi](https://x.y).\n- Tests pass.";
        assert_eq!(
            card_summary(md),
            "Wrote hello.txt in the workspace.\ncat hello.txt\nIt says hi.\nTests pass."
        );
        assert_eq!(card_summary("## Done\n1. One"), "Done\nOne");
        assert_eq!(
            card_summary("Edit `rules/new_members.toml`, _not_ __this__."),
            "Edit rules/new_members.toml, not this."
        );
        assert_eq!(card_summary("  \n\n"), "");
    }

    /// Finding 16 of review cycle 2: "...were changed. uname -s date -u %Y"
    /// ran the lines of a code block together with the text before it.
    #[test]
    fn the_lines_of_a_code_block_do_not_run_together() {
        let indented =
            "Three files were changed.\n\n    uname -s\n    date -u +%Y\n\nNo other files changed.";
        assert_eq!(
            card_summary(indented),
            "Three files were changed.\nuname -s\ndate -u +%Y\nNo other files changed."
        );
        let fenced = "Three files were changed.\n```\nuname -s\ndate -u +%Y\n```\nDone.";
        assert_eq!(
            card_summary(fenced),
            "Three files were changed.\nuname -s\ndate -u +%Y\nDone."
        );
        // Marks in code stay: they are not Markdown there.
        assert_eq!(card_summary("```\nls *_x\n```"), "ls *_x");
    }

    #[test]
    fn a_paragraph_that_wraps_in_the_source_stays_one_line() {
        let md = "The first line of the\nsame paragraph.\n\nA second paragraph.";
        assert_eq!(
            card_summary(md),
            "The first line of the same paragraph.\nA second paragraph."
        );
        // An indented line inside a paragraph is not code.
        assert_eq!(card_summary("Text\n    still text"), "Text still text");
    }

    /// Review cycle 3, finding 5: "Copy text" ran every message through a
    /// filter that dropped each `*` and `_`, also inside backticks. So
    /// `rm -rf build/*` copied as `rm -rf build/`.
    #[test]
    fn code_spans_copy_exactly_as_written() {
        assert_eq!(
            plain_text("Clean up with `rm -rf build/*` and `ls *.log`"),
            "Clean up with rm -rf build/* and ls *.log"
        );
        assert_eq!(
            plain_text("Edit `__init__.py` and `my_script.sh`, then run `cargo test -p x`."),
            "Edit __init__.py and my_script.sh, then run cargo test -p x."
        );
        assert_eq!(
            plain_text("Pass `*args` and `**kwargs`, and `a_b_c`."),
            "Pass *args and **kwargs, and a_b_c."
        );
    }

    #[test]
    fn code_blocks_copy_exactly_as_written() {
        let md =
            "Run this:\n\n```sh\nrm -rf build/*\nprintf '__init__'\n  indented [x] *\n```\n\nDone.";
        assert_eq!(
            plain_text(md),
            "Run this:\n\nrm -rf build/*\nprintf '__init__'\n  indented [x] *\n\nDone."
        );
        // An indented block too.
        assert_eq!(
            plain_text("Text.\n\n    ls *.log\n    cat __init__.py\n"),
            "Text.\n\nls *.log\ncat __init__.py"
        );
    }

    #[test]
    fn marks_that_the_parser_reads_as_text_stay() {
        assert_eq!(plain_text("Run ls *.log"), "Run ls *.log");
        assert_eq!(
            plain_text("Set arr[0] to the value of b[i]"),
            "Set arr[0] to the value of b[i]"
        );
        assert_eq!(
            plain_text("Call it with *args and **kwargs"),
            "Call it with *args and **kwargs"
        );
        assert_eq!(
            plain_text("The total is 2 * 3 = 6"),
            "The total is 2 * 3 = 6"
        );
        assert_eq!(
            plain_text("A file my_script.sh and new_members"),
            "A file my_script.sh and new_members"
        );
        assert_eq!(plain_text("It costs $5 and $10."), "It costs $5 and $10.");
        // An escape shows as the mark.
        assert_eq!(plain_text(r"Five \* six"), "Five * six");
    }

    #[test]
    fn emphasis_and_links_lose_their_marks_and_keep_their_words() {
        assert_eq!(
            plain_text(
                "This is **bold**, _italic_ and ~~gone~~ text. See [the docs](https://x.y)."
            ),
            "This is bold, italic and gone text. See the docs."
        );
        assert_eq!(plain_text("## Title\n\nBody"), "Title\n\nBody");
        assert_eq!(
            plain_text("See https://example.com/a_b."),
            "See https://example.com/a_b."
        );
    }

    #[test]
    fn lists_keep_their_marks_and_tasks_keep_their_boxes() {
        assert_eq!(
            plain_text("- one\n- two\n  - nested"),
            "- one\n- two\n  - nested"
        );
        assert_eq!(plain_text("1. one\n2. two"), "1. one\n2. two");
        assert_eq!(
            plain_text("- [x] done\n- [ ] open"),
            "- [x] done\n- [ ] open"
        );
    }

    #[test]
    fn a_line_break_in_a_paragraph_stays_a_line_break() {
        assert_eq!(plain_text("line one\nline two"), "line one\nline two");
        assert_eq!(plain_text(""), "");
    }

    /// The text view shows `__init__.py` outside code as bold "init" and
    /// ".py", because CommonMark reads the underscores as strong marks. The
    /// copy matches the screen. The exact text is in "Copy as Markdown", and
    /// in a code span.
    #[test]
    fn the_copy_matches_what_the_reader_sees() {
        assert_eq!(plain_text("Edit __init__.py now"), "Edit init.py now");
    }

    #[test]
    fn a_card_summary_keeps_code_exactly_too() {
        assert_eq!(
            card_summary("Run `rm -rf build/*` now.\n\n- Use `__init__.py`."),
            "Run rm -rf build/* now.\nUse __init__.py."
        );
    }

    #[test]
    fn days_are_named() {
        let now = Local
            .with_ymd_and_hms(2026, 10, 29, 12, 0, 0)
            .single()
            .expect("valid")
            .timestamp_millis();
        assert_eq!(day_label(now - 3_600_000, now), "Today");
        assert_eq!(day_label(now - 24 * 3_600_000, now), "Yesterday");
        assert_eq!(day_label(now - 40 * 24 * 3_600_000, now), "September 19");
    }
}
