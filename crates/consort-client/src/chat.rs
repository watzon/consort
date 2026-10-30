//! The rows of a timeline as the chat shows them: bubbles in two lanes,
//! cards in the assistant lane, and centered time separators.
//!
//! The reducers make [`TimelineItem`]s, one for each message. This module
//! turns them into [`ChatRow`]s, one for each thing that the list draws:
//!
//! - **Lanes.** The assistant speaks in the left lane and the user in the
//!   right lane. Task cards, approval cards and decision cards sit in the
//!   assistant lane.
//! - **Groups.** Bubbles that follow each other in the same lane form a
//!   group. A time separator, a card or a notice ends a group. The last
//!   bubble of a group gets the small corner on the speaker's side.
//! - **One bubble for each message.** A message is one bubble, whatever it
//!   holds: paragraphs, a heading, a list, a table or a code block. The old
//!   rule split an assistant message at each paragraph, so a long answer
//!   looked like many messages (DECISIONS 731).
//! - **Time separators.** See [`separator_before`].

use markdown::{ParseOptions, mdast::Node};

use crate::view::{Speaker, TimelineItem};

/// A gap this long or longer between two items puts a time separator
/// between them. 30 minutes: a reply that comes in a few minutes stays in
/// the same group, and a new conversation later in the day gets its time.
pub const SEPARATOR_GAP_MS: i64 = 30 * 60 * 1000;

/// Which side of the timeline a row sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// Left: the assistant, its tasks and its cards.
    Assistant,
    /// Right: the user.
    You,
}

/// The space above a row. The view turns it into a size token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spacing {
    /// The first row of the timeline.
    Start,
    /// The next bubble of the same group (4 px in the design).
    InGroup,
    /// The first row of a new group (16 px in the design).
    NewGroup,
    /// The first row after a time separator. The separator has its own space.
    AfterSeparator,
}

/// One part of a message in its own bubble.
#[derive(Clone, Debug, PartialEq)]
pub struct BubbleRow {
    /// The index of the message in [`crate::view::TimelineView::items`].
    pub item: usize,
    /// The index of this part in the message, from 0.
    pub part: usize,
    /// The Markdown of this part. Empty for a title bubble.
    pub markdown: String,
    pub lane: Lane,
    /// The bubble shows the message title (the first line of a brief), in
    /// semibold.
    pub title: bool,
    /// The task references of a brief go under the text of this bubble.
    pub with_items: bool,
    /// The last bubble of its group: it has the small corner.
    pub last_in_group: bool,
    /// The bubble takes the full bubble width instead of fitting its text.
    /// See [`needs_full_width`].
    pub wide: bool,
    pub spacing: Spacing,
}

/// A card in the assistant lane: a task card, an approval or a decision.
#[derive(Clone, Debug, PartialEq)]
pub struct CardRow {
    pub item: usize,
    pub spacing: Spacing,
}

/// What an attachment row of a message shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentPart {
    /// The pictures of the message, as tiles in a grid: indexes into
    /// [`crate::view::MessageView::attachments`].
    Images(Vec<usize>),
    /// One file that is not a picture, as a file bubble or chip.
    File(usize),
}

/// The attachments of a message, in the lane of its speaker. Your pictures
/// sit above your text, and your files are file bubbles. The assistant's
/// attachments follow its text.
#[derive(Clone, Debug, PartialEq)]
pub struct AttachmentRow {
    pub item: usize,
    /// The index of this row among the attachment rows of the message.
    pub part: usize,
    pub lane: Lane,
    pub kind: AttachmentPart,
    pub spacing: Spacing,
}

/// One row of the chat.
#[derive(Clone, Debug, PartialEq)]
pub enum ChatRow {
    /// A centered caption, such as "Today 8:00 AM" or "9:12 AM".
    Separator {
        key: String,
        label: String,
    },
    Bubble(BubbleRow),
    /// Pictures or a file of a message.
    Attachment(AttachmentRow),
    Card(CardRow),
    /// A system notice: a quiet centered line.
    Notice {
        item: usize,
        spacing: Spacing,
    },
    /// The reaction chips and the reply line under a message, in its lane.
    Footer {
        item: usize,
        lane: Lane,
    },
    /// The line under a message group, which is always there, so a hover
    /// never moves the timeline. It acts on the last message of the group,
    /// and shows while the pointer is on any item from `first` to `item`.
    Actions {
        first: usize,
        item: usize,
        lane: Lane,
    },
}

impl ChatRow {
    /// A key that is stable while the row looks the same. It changes when
    /// the row moves to the end of its group, so a list that caches row
    /// heights measures the row again.
    pub fn key(&self, items: &[TimelineItem]) -> String {
        let item_key = |i: usize| items.get(i).map(TimelineItem::key).unwrap_or_default();
        match self {
            ChatRow::Separator { key, .. } => key.clone(),
            ChatRow::Bubble(b) => format!(
                "{}#{}{}",
                item_key(b.item),
                b.part,
                if b.last_in_group { "e" } else { "" }
            ),
            ChatRow::Attachment(a) => format!("{}#a{}", item_key(a.item), a.part),
            ChatRow::Card(c) => item_key(c.item).to_owned(),
            ChatRow::Notice { item, .. } => item_key(*item).to_owned(),
            ChatRow::Footer { item, .. } => format!("{}#f", item_key(*item)),
            ChatRow::Actions { item, .. } => format!("{}#g", item_key(*item)),
        }
    }
}

/// The time of an item, for the separators: Unix milliseconds and its label.
fn time_of(item: &TimelineItem) -> Option<(i64, &str)> {
    match item {
        TimelineItem::Day(_) => None,
        TimelineItem::Message(m) => Some((m.created_at, &m.time_label)),
        TimelineItem::TaskCard(c) => Some((c.created_at, &c.time_label)),
        TimelineItem::Approval(a) => Some((a.created_at, &a.time_label)),
        TimelineItem::Decision(d) => Some((d.created_at, &d.time_label)),
        TimelineItem::Notice(n) => Some((n.created_at, &n.time_label)),
    }
}

/// The label of a time separator, or `None` when the item needs none.
///
/// The rule:
/// - The first item of a day gets the day and the time: "Today 8:00 AM",
///   "Yesterday 4:10 PM", "Tuesday 9:00 AM", "October 12, 9:00 AM".
/// - A later item gets the time alone ("9:12 AM") when it comes
///   [`SEPARATOR_GAP_MS`] or more after the item before it.
/// - Other items get no separator.
///
/// `day` is the label of the day that starts before this item, if one does.
/// `previous` is the time of the item before, in the same day.
pub fn separator_before(
    day: Option<&str>,
    previous: Option<i64>,
    at: i64,
    time_label: &str,
) -> Option<String> {
    if let Some(day) = day.filter(|d| !d.is_empty()) {
        if time_label.is_empty() {
            return Some(day.to_owned());
        }
        // A date with a number in it takes a comma before the time, so the
        // two numbers do not run together.
        let comma = if day.chars().any(|c| c.is_ascii_digit()) {
            ","
        } else {
            ""
        };
        return Some(format!("{day}{comma} {time_label}"));
    }
    match previous {
        Some(prev) if at - prev >= SEPARATOR_GAP_MS && !time_label.is_empty() => {
            Some(time_label.to_owned())
        }
        _ => None,
    }
}

/// Whether a bubble's Markdown needs the full bubble width.
///
/// A bubble fits its text, but the layout cannot measure the natural width
/// of a list, a table, a quote or a code block: it only knows the width of
/// their widest part that cannot wrap, often one short heading. A bubble
/// with only paragraphs and link definitions fits its text; any other block
/// makes it wide, so that its lines do not wrap word by word.
pub fn needs_full_width(markdown: &str) -> bool {
    let mut options = ParseOptions::gfm();
    options.constructs.math_text = false;
    options.constructs.math_flow = false;
    let Ok(root) = markdown::to_mdast(markdown, &options) else {
        return false;
    };
    root.children().is_some_and(|children| {
        children
            .iter()
            .any(|child| !matches!(child, Node::Paragraph(_) | Node::Definition(_)))
    })
}

/// The lane of a speaker.
fn lane_of(speaker: Speaker) -> Lane {
    match speaker {
        Speaker::You => Lane::You,
        Speaker::Consort | Speaker::Task | Speaker::System => Lane::Assistant,
    }
}

/// Turns the items of a timeline into the rows of the chat.
pub fn rows(items: &[TimelineItem]) -> Vec<ChatRow> {
    let mut out: Vec<ChatRow> = Vec::new();
    // The label of the day that starts before the next item.
    let mut day: Option<&str> = None;
    let mut previous_at: Option<i64> = None;
    // The lane of the open group of bubbles, if one is open.
    let mut open_lane: Option<Lane> = None;
    // The index in `out` of the last bubble of the open group.
    let mut last_bubble: Option<usize> = None;
    let mut after_separator = false;

    // The messages of the open group: the first, and the last.
    let mut group: Option<(usize, usize)> = None;
    let mut after_proactive = false;

    fn close(
        out: &mut Vec<ChatRow>,
        last_bubble: &mut Option<usize>,
        open_lane: &mut Option<Lane>,
        group: &mut Option<(usize, usize)>,
    ) {
        if let Some(ix) = last_bubble.take()
            && let Some(ChatRow::Bubble(b)) = out.get_mut(ix)
        {
            b.last_in_group = true;
        }
        if let (Some(lane), Some((first, item))) = (*open_lane, group.take()) {
            out.push(ChatRow::Actions { first, item, lane });
        }
        *open_lane = None;
    }

    for (ix, item) in items.iter().enumerate() {
        if let TimelineItem::Day(d) = item {
            day = Some(d.label.as_str());
            previous_at = None;
            continue;
        }
        let Some((at, time_label)) = time_of(item) else {
            continue;
        };
        if let Some(label) = separator_before(day, previous_at, at, time_label) {
            close(&mut out, &mut last_bubble, &mut open_lane, &mut group);
            out.push(ChatRow::Separator {
                key: format!("sep-{}", item.key()),
                label,
            });
            after_separator = true;
        }
        day = None;
        previous_at = Some(at);
        let spacing_for = |in_group: bool, after_separator: bool, first: bool| {
            if first {
                Spacing::Start
            } else if after_separator {
                Spacing::AfterSeparator
            } else if in_group {
                Spacing::InGroup
            } else {
                Spacing::NewGroup
            }
        };
        match item {
            TimelineItem::Message(m) => {
                let lane = lane_of(m.speaker);
                // A proactive message is a group of its own, because the
                // footer of a group rates only its last message.
                let proactive = m.proactive.is_some();
                if open_lane != Some(lane) || proactive || after_proactive {
                    close(&mut out, &mut last_bubble, &mut open_lane, &mut group);
                }
                after_proactive = proactive;
                group = Some((group.map_or(ix, |(first, _)| first), ix));
                let mut parts: Vec<(String, bool)> = Vec::new();
                if m.title.as_deref().is_some_and(|t| !t.trim().is_empty()) {
                    parts.push((String::new(), true));
                }
                // A message is one bubble. Long text, a heading, a list, a
                // table or a code block do not split it.
                parts.push((
                    if lane == Lane::You {
                        m.body.clone()
                    } else {
                        m.body.trim().to_owned()
                    },
                    false,
                ));
                // A message with only files has no text bubble.
                if lane == Lane::You || m.body.trim().is_empty() {
                    parts.retain(|(markdown, title)| *title || !markdown.trim().is_empty());
                }
                if parts.iter().all(|(_, title)| *title) && m.attachments.is_empty() {
                    // A message with no text still gets its bubble.
                    parts.push((String::new(), false));
                }
                // The attachment rows: the pictures in one grid, then each
                // file.
                let images: Vec<usize> = m
                    .attachments
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| a.image)
                    .map(|(i, _)| i)
                    .collect();
                let mut files: Vec<AttachmentPart> = Vec::new();
                if !images.is_empty() {
                    files.push(AttachmentPart::Images(images));
                }
                files.extend(
                    m.attachments
                        .iter()
                        .enumerate()
                        .filter(|(_, a)| !a.image)
                        .map(|(i, _)| AttachmentPart::File(i)),
                );
                let mut push_attachments =
                    |out: &mut Vec<ChatRow>,
                     open_lane: &mut Option<Lane>,
                     after_separator: &mut bool| {
                        for (part, kind) in std::mem::take(&mut files).into_iter().enumerate() {
                            let in_group = *open_lane == Some(lane);
                            let spacing = spacing_for(in_group, *after_separator, out.is_empty());
                            out.push(ChatRow::Attachment(AttachmentRow {
                                item: ix,
                                part,
                                lane,
                                kind,
                                spacing,
                            }));
                            *after_separator = false;
                            *open_lane = Some(lane);
                        }
                    };
                // Your files come before your text.
                if lane == Lane::You {
                    push_attachments(&mut out, &mut open_lane, &mut after_separator);
                }
                let last_text = parts.iter().rposition(|(_, title)| !title);
                for (part, (markdown, title)) in parts.into_iter().enumerate() {
                    let in_group = open_lane == Some(lane);
                    let spacing = spacing_for(in_group, after_separator, out.is_empty());
                    let with_items = !m.items.is_empty() && Some(part) == last_text;
                    let wide = with_items || (!title && needs_full_width(&markdown));
                    out.push(ChatRow::Bubble(BubbleRow {
                        item: ix,
                        part,
                        markdown,
                        lane,
                        title,
                        with_items,
                        last_in_group: false,
                        wide,
                        spacing,
                    }));
                    after_separator = false;
                    open_lane = Some(lane);
                    last_bubble = Some(out.len() - 1);
                }
                push_attachments(&mut out, &mut open_lane, &mut after_separator);
                if !m.reactions.is_empty() || m.reply_count > 0 {
                    out.push(ChatRow::Footer { item: ix, lane });
                }
            }
            TimelineItem::TaskCard(_) | TimelineItem::Approval(_) | TimelineItem::Decision(_) => {
                close(&mut out, &mut last_bubble, &mut open_lane, &mut group);
                let spacing = spacing_for(false, after_separator, out.is_empty());
                out.push(ChatRow::Card(CardRow { item: ix, spacing }));
                after_separator = false;
            }
            TimelineItem::Notice(_) => {
                close(&mut out, &mut last_bubble, &mut open_lane, &mut group);
                let spacing = spacing_for(false, after_separator, out.is_empty());
                out.push(ChatRow::Notice { item: ix, spacing });
                after_separator = false;
            }
            TimelineItem::Day(_) => {}
        }
    }
    close(&mut out, &mut last_bubble, &mut open_lane, &mut group);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{DayView, MessageView, NoticeView, TaskCardView};

    #[test]
    fn only_paragraphs_fit_their_text() {
        assert!(!needs_full_width("One short line."));
        assert!(!needs_full_width(
            "Two\n\nparagraphs, with a [link][a].\n\n[a]: https://example.com"
        ));
        assert!(needs_full_width(
            "**Your accounts**\n\n- Gmail: search, read and draft.\n- Calendar"
        ));
        assert!(needs_full_width("## Run work\n\nStart tasks."));
        assert!(needs_full_width("Run this:\n\n```sh\ncargo test\n```"));
        assert!(needs_full_width("> A quote"));
        assert!(needs_full_width("| a | b |\n|---|---|\n| 1 | 2 |"));
    }

    const MIN: i64 = 60_000;

    fn day(label: &str) -> TimelineItem {
        TimelineItem::Day(DayView {
            key: format!("day-{label}"),
            label: label.into(),
        })
    }

    fn msg(id: &str, speaker: Speaker, at_min: i64, label: &str, body: &str) -> TimelineItem {
        TimelineItem::Message(MessageView {
            id: id.into(),
            speaker,
            created_at: at_min * MIN,
            time_label: label.into(),
            body: body.into(),
            ..MessageView::default()
        })
    }

    fn card(id: &str, at_min: i64, label: &str) -> TimelineItem {
        TimelineItem::TaskCard(TaskCardView {
            message_id: id.into(),
            task_id: format!("t-{id}"),
            created_at: at_min * MIN,
            time_label: label.into(),
            ..TaskCardView::default()
        })
    }

    fn bubbles(rows: &[ChatRow]) -> Vec<&BubbleRow> {
        rows.iter()
            .filter_map(|r| match r {
                ChatRow::Bubble(b) => Some(b),
                _ => None,
            })
            .collect()
    }

    fn labels(rows: &[ChatRow]) -> Vec<&str> {
        rows.iter()
            .filter_map(|r| match r {
                ChatRow::Separator { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect()
    }

    // One bubble for each message

    /// The Markdown of the bubbles that one assistant message makes.
    fn assistant_parts(body: &str) -> Vec<String> {
        let rows = rows(&[msg("a", Speaker::Consort, 0, "9:12 AM", body)]);
        bubbles(&rows).iter().map(|b| b.markdown.clone()).collect()
    }

    #[test]
    fn plain_paragraphs_stay_in_one_bubble() {
        // The old rule made two bubbles here, and a long answer looked like
        // many messages.
        assert_eq!(
            assistant_parts("One step is left.\n\nThe CI change is a patch."),
            ["One step is left.\n\nThe CI change is a patch."]
        );
    }

    #[test]
    fn a_long_answer_with_many_paragraphs_is_one_bubble() {
        let body = (1..=12)
            .map(|n| format!("Paragraph {n} of the answer."))
            .collect::<Vec<_>>()
            .join("\n\n");
        assert_eq!(assistant_parts(&body), [body]);
    }

    #[test]
    fn a_heading_a_list_a_table_and_a_code_block_stay_in_one_bubble() {
        let text = "## What to check\n\nRead the report.\n\n1. Build the engine.\n2. Review the rules.\n\n   Still part of item two.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```sh\ncargo build\n\ncargo test\n```\n\nThen decide.";
        let parts = assistant_parts(text);
        assert_eq!(parts, [text]);
    }

    #[test]
    fn a_bubble_with_text_around_a_block_needs_the_full_width() {
        let rows = rows(&[msg(
            "a",
            Speaker::Consort,
            0,
            "9:12 AM",
            "Two tasks:\n\n- One.\n- Two.\n\nThat is all.",
        )]);
        assert!(bubbles(&rows)[0].wide);
    }

    #[test]
    fn reference_links_stay_with_their_text_in_the_one_bubble() {
        let text = "See [the PR][pr].\n\nAnd [it][pr] again.\n\n[pr]: https://x.test/1";
        assert_eq!(assistant_parts(text), [text]);
    }

    #[test]
    fn a_message_is_trimmed_and_an_empty_one_still_gets_its_bubble() {
        assert_eq!(assistant_parts("  Hello.  "), ["Hello."]);
        assert_eq!(assistant_parts(""), [""]);
    }

    // Grouping

    #[test]
    fn bubbles_in_one_lane_form_a_group_and_the_last_has_the_corner() {
        let items = [
            day("Today"),
            msg("a", Speaker::You, 0, "9:12 AM", "What is left?"),
            msg(
                "b",
                Speaker::Consort,
                1,
                "9:13 AM",
                "One step.\n\nAnd a patch.",
            ),
            msg("c", Speaker::Consort, 2, "9:14 AM", "Done."),
        ];
        let rows = rows(&items);
        let b = bubbles(&rows);
        // One bubble for each message: the two paragraphs of `b` are one.
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].lane, Lane::You);
        assert!(b[0].last_in_group);
        assert_eq!(
            b[1..].iter().map(|b| b.lane).collect::<Vec<_>>(),
            [Lane::Assistant; 2]
        );
        assert_eq!(
            b[1..].iter().map(|b| b.last_in_group).collect::<Vec<_>>(),
            [false, true]
        );
        assert_eq!(b[1].spacing, Spacing::NewGroup);
        assert_eq!(b[2].spacing, Spacing::InGroup);
        assert_eq!(b[0].spacing, Spacing::AfterSeparator);
    }

    #[test]
    fn your_message_stays_one_bubble() {
        let items = [msg("a", Speaker::You, 0, "", "First.\n\nSecond.")];
        let rows = rows(&items);
        assert_eq!(bubbles(&rows).len(), 1);
        assert_eq!(bubbles(&rows)[0].markdown, "First.\n\nSecond.");
    }

    #[test]
    fn a_card_ends_the_group_and_sits_in_its_own() {
        let items = [
            msg("a", Speaker::Consort, 0, "", "Before."),
            card("k", 1, ""),
            msg("b", Speaker::Consort, 2, "", "After."),
        ];
        let rows = rows(&items);
        assert!(matches!(
            rows[1],
            ChatRow::Actions {
                first: 0,
                item: 0,
                ..
            }
        ));
        assert!(
            matches!(&rows[2], ChatRow::Card(c) if c.item == 1 && c.spacing == Spacing::NewGroup)
        );
        let b = bubbles(&rows);
        assert!(b[0].last_in_group, "the card ends the group before it");
        assert_eq!(b[1].spacing, Spacing::NewGroup);
        assert!(b[1].last_in_group);
    }

    #[test]
    fn a_brief_has_a_title_bubble_and_its_items_under_the_last_text() {
        let mut brief = MessageView {
            id: "brief".into(),
            title: Some("Good morning, Chris.".into()),
            body: "Two tasks need you.\n\nAnd one runs.".into(),
            items: vec![crate::view::BriefItem::default()],
            ..MessageView::default()
        };
        brief.speaker = Speaker::Consort;
        let rows = rows(&[TimelineItem::Message(brief)]);
        let b = bubbles(&rows);
        assert_eq!(b.len(), 2);
        assert!(b[0].title && b[0].markdown.is_empty());
        assert!(!b[0].with_items && b[1].with_items);
        assert!(b[1].last_in_group);
    }

    #[test]
    fn keys_are_unique_and_change_when_a_bubble_ends_its_group() {
        let first = [msg("a", Speaker::Consort, 0, "", "One.\n\nTwo.")];
        let rows_a = rows(&first);
        let keys: Vec<String> = rows_a.iter().map(|r| r.key(&first)).collect();
        assert_eq!(keys, ["a#0e", "a#g"]);
        let more = [
            msg("a", Speaker::Consort, 0, "", "One.\n\nTwo."),
            msg("b", Speaker::Consort, 1, "", "Three."),
        ];
        let rows_b = rows(&more);
        let keys: Vec<String> = rows_b.iter().map(|r| r.key(&more)).collect();
        assert_eq!(keys, ["a#0", "b#0e", "b#g"]);
    }

    // Time separators

    #[test]
    fn the_first_item_of_a_day_gets_the_day_and_the_time() {
        assert_eq!(
            separator_before(Some("Today"), None, 0, "8:00 AM").as_deref(),
            Some("Today 8:00 AM")
        );
        assert_eq!(
            separator_before(Some("October 12"), None, 0, "8:00 AM").as_deref(),
            Some("October 12, 8:00 AM")
        );
        assert_eq!(
            separator_before(Some("Yesterday"), Some(0), 1, "4:10 PM").as_deref(),
            Some("Yesterday 4:10 PM")
        );
    }

    #[test]
    fn a_long_gap_gets_the_time_and_a_short_gap_gets_nothing() {
        let gap = SEPARATOR_GAP_MS;
        assert_eq!(
            separator_before(None, Some(0), gap, "9:12 AM").as_deref(),
            Some("9:12 AM")
        );
        assert_eq!(separator_before(None, Some(0), gap - 1, "9:12 AM"), None);
        assert_eq!(separator_before(None, None, gap * 4, "9:12 AM"), None);
    }

    #[test]
    fn the_design_timeline_gets_two_separators() {
        // Tutti in the design: the brief at 8:00, then a question at 9:12,
        // the reply at 9:12 and a task card at 9:13.
        let items = [
            day("Today"),
            msg(
                "brief",
                Speaker::Consort,
                8 * 60,
                "8:00 AM",
                "Two tasks need you.",
            ),
            msg("q", Speaker::You, 9 * 60 + 12, "9:12 AM", "What is left?"),
            msg("r", Speaker::Consort, 9 * 60 + 12, "9:12 AM", "One step."),
            card("k", 9 * 60 + 13, "9:13 AM"),
        ];
        let rows = rows(&items);
        assert_eq!(labels(&rows), ["Today 8:00 AM", "9:12 AM"]);
        assert!(matches!(rows[0], ChatRow::Separator { .. }));
        // A separator closes the group before it.
        let b = bubbles(&rows);
        assert!(b[0].last_in_group);
    }

    #[test]
    fn a_new_day_gets_a_separator_even_after_a_short_gap() {
        let items = [
            day("Yesterday"),
            msg("a", Speaker::Consort, 0, "11:58 PM", "Late."),
            day("Today"),
            msg("b", Speaker::Consort, 5, "12:03 AM", "Early."),
        ];
        let rows = rows(&items);
        assert_eq!(labels(&rows), ["Yesterday 11:58 PM", "Today 12:03 AM"]);
        let b = bubbles(&rows);
        assert!(b[0].last_in_group && b[1].last_in_group);
    }

    #[test]
    fn a_notice_is_its_own_row_and_ends_the_group() {
        let items = [
            msg("a", Speaker::Consort, 0, "", "Hi."),
            TimelineItem::Notice(NoticeView {
                message_id: "n".into(),
                created_at: MIN,
                text: "The assistant is not available.".into(),
                ..NoticeView::default()
            }),
        ];
        let rows = rows(&items);
        assert!(matches!(rows[2], ChatRow::Notice { item: 1, .. }));
        assert!(bubbles(&rows)[0].last_in_group);
    }

    #[test]
    fn a_group_has_one_footer_that_covers_all_its_messages() {
        let items = [
            msg("a", Speaker::Consort, 0, "", "One."),
            msg("b", Speaker::Consort, 1, "", "Two."),
            msg("c", Speaker::You, 2, "", "Three."),
        ];
        let footers: Vec<(usize, usize, Lane)> = rows(&items)
            .iter()
            .filter_map(|r| match r {
                ChatRow::Actions { first, item, lane } => Some((*first, *item, *lane)),
                _ => None,
            })
            .collect();
        assert_eq!(footers, [(0, 1, Lane::Assistant), (2, 2, Lane::You)]);
    }

    #[test]
    fn a_proactive_message_is_a_group_of_its_own() {
        let TimelineItem::Message(mut proactive) = msg("b", Speaker::Consort, 1, "", "Two.") else {
            panic!("a message")
        };
        proactive.proactive = Some(Default::default());
        let items = [
            msg("a", Speaker::Consort, 0, "", "One."),
            TimelineItem::Message(proactive),
            msg("c", Speaker::Consort, 2, "", "Three."),
        ];
        let footers: Vec<(usize, usize)> = rows(&items)
            .iter()
            .filter_map(|r| match r {
                ChatRow::Actions { first, item, .. } => Some((*first, *item)),
                _ => None,
            })
            .collect();
        assert_eq!(footers, [(0, 0), (1, 1), (2, 2)]);
    }

    fn with_files(item: TimelineItem, names: &[&str]) -> TimelineItem {
        let TimelineItem::Message(mut m) = item else {
            panic!("a message")
        };
        m.attachments = names
            .iter()
            .map(|n| crate::files::AttachmentView {
                name: (*n).into(),
                image: n.ends_with(".jpg"),
                ..crate::files::AttachmentView::default()
            })
            .collect();
        TimelineItem::Message(m)
    }

    #[test]
    fn your_pictures_and_files_come_before_your_text_and_the_assistants_after() {
        let items = vec![
            day("Today"),
            with_files(
                msg("m1", Speaker::You, 0, "7:40 PM", "Here are the photos."),
                &["front.jpg", "side.jpg", "quote.pdf"],
            ),
            with_files(
                msg(
                    "m2",
                    Speaker::Consort,
                    1,
                    "7:41 PM",
                    "The comparison is ready.",
                ),
                &["comparison.xlsx"],
            ),
            with_files(msg("m3", Speaker::You, 2, "7:42 PM", ""), &["notes.pdf"]),
        ];
        let rows = rows(&items);
        let kinds: Vec<String> = rows
            .iter()
            .map(|r| match r {
                ChatRow::Separator { .. } => "sep".to_owned(),
                ChatRow::Bubble(b) => format!("bubble {}", b.item),
                ChatRow::Attachment(a) => format!("{:?} {}", a.kind, a.item),
                ChatRow::Card(_) => "card".to_owned(),
                ChatRow::Notice { .. } => "notice".to_owned(),
                ChatRow::Footer { .. } => "footer".to_owned(),
                ChatRow::Actions { .. } => "actions".to_owned(),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "sep",
                "Images([0, 1]) 1",
                "File(2) 1",
                "bubble 1",
                "actions",
                "bubble 2",
                "File(0) 2",
                "actions",
                // A message with only a file has no empty text bubble.
                "File(0) 3",
                "actions",
            ]
        );
        // The rows of one message keep one group; each has its own key.
        let ChatRow::Attachment(first) = &rows[1] else {
            panic!("pictures")
        };
        assert_eq!(first.spacing, Spacing::AfterSeparator);
        let ChatRow::Attachment(file) = &rows[2] else {
            panic!("file")
        };
        assert_eq!(file.spacing, Spacing::InGroup);
        assert_ne!(rows[1].key(&items), rows[2].key(&items));
        // Your text bubble is the last of your group, so it has the tail.
        assert!(bubbles(&rows)[0].last_in_group);
    }

    #[test]
    fn reactions_and_replies_get_a_footer_in_the_lane_of_the_message() {
        let mut answer = msg("m2", Speaker::Consort, 1, "9:01 AM", "One.\n\nTwo.");
        if let TimelineItem::Message(m) = &mut answer {
            m.reply_count = 4;
        }
        let mut question = msg("m1", Speaker::You, 0, "9:00 AM", "What is left?");
        if let TimelineItem::Message(m) = &mut question {
            m.reactions = vec![crate::convo::ReactionView {
                emoji: "👀".into(),
                mine: false,
                assistant: true,
            }];
        }
        let items = vec![
            question,
            answer,
            msg("m3", Speaker::Consort, 2, "9:02 AM", "Three."),
        ];
        let rows = rows(&items);
        let footers: Vec<(usize, Lane)> = rows
            .iter()
            .filter_map(|r| match r {
                ChatRow::Footer { item, lane } => Some((*item, *lane)),
                _ => None,
            })
            .collect();
        assert_eq!(footers, vec![(0, Lane::You), (1, Lane::Assistant)]);
        // The footer follows the last bubble of its message.
        let at = rows
            .iter()
            .position(|r| matches!(r, ChatRow::Footer { item: 1, .. }))
            .unwrap();
        assert!(matches!(&rows[at - 1], ChatRow::Bubble(b) if b.item == 1 && b.part == 0));
        assert_eq!(rows[at].key(&items), "m2#f");
    }
}
