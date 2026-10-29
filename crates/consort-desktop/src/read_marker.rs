//! Which message the app last marked as read in each channel, so it asks the
//! client to mark a channel read only when a newer message arrives.

use std::collections::HashMap;

use consort_client::{TimelineItem, TimelineView};

#[derive(Debug, Default)]
pub struct ReadMarker {
    marked: HashMap<String, String>,
}

impl ReadMarker {
    /// Records `newest` as marked for the channel, and says whether the
    /// client must mark the channel read. It must when the newest message is
    /// not the one marked last. With no message, nothing needs a mark.
    pub fn should_mark(&mut self, channel_id: &str, newest: Option<&str>) -> bool {
        let Some(newest) = newest else {
            return false;
        };
        if self.marked.get(channel_id).map(String::as_str) == Some(newest) {
            return false;
        }
        self.marked.insert(channel_id.to_owned(), newest.to_owned());
        true
    }

    /// Forgets a channel, so the next check marks it again. A failed mark
    /// uses this to try again.
    pub fn forget(&mut self, channel_id: &str) {
        self.marked.remove(channel_id);
    }
}

/// The key of the newest message in a timeline that the hub knows. A day
/// label is not a message, and a message that is still on its way (pending)
/// is the user's own, so neither counts.
pub fn newest_message(timeline: &TimelineView) -> Option<&str> {
    timeline.items.iter().rev().find_map(|item| match item {
        TimelineItem::Day(_) => None,
        TimelineItem::Message(m) if m.pending => None,
        other => Some(other.key()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_client::{DayView, MessageView, TaskCardView};

    fn message(id: &str, pending: bool) -> TimelineItem {
        TimelineItem::Message(MessageView {
            id: id.into(),
            pending,
            ..MessageView::default()
        })
    }

    fn timeline(items: Vec<TimelineItem>) -> TimelineView {
        TimelineView {
            channel_id: "c1".into(),
            items,
            has_older: false,
        }
    }

    #[test]
    fn a_channel_is_marked_once_per_newest_message() {
        let mut marker = ReadMarker::default();
        assert!(marker.should_mark("c1", Some("m1")));
        assert!(!marker.should_mark("c1", Some("m1")));
        assert!(marker.should_mark("c1", Some("m2")));
        // Another channel has its own mark.
        assert!(marker.should_mark("c2", Some("m2")));
        assert!(!marker.should_mark("c2", Some("m2")));
    }

    #[test]
    fn an_empty_channel_needs_no_mark() {
        let mut marker = ReadMarker::default();
        assert!(!marker.should_mark("c1", None));
        assert!(marker.should_mark("c1", Some("m1")));
    }

    #[test]
    fn forgetting_a_channel_marks_it_again() {
        let mut marker = ReadMarker::default();
        assert!(marker.should_mark("c1", Some("m1")));
        marker.forget("c1");
        assert!(marker.should_mark("c1", Some("m1")));
    }

    #[test]
    fn the_newest_message_skips_day_labels_and_pending_messages() {
        let tl = timeline(vec![
            TimelineItem::Day(DayView {
                key: "day-1".into(),
                label: "Today".into(),
            }),
            message("m1", false),
            TimelineItem::TaskCard(TaskCardView {
                message_id: "card-1".into(),
                ..TaskCardView::default()
            }),
            message("pending-x", true),
        ]);
        assert_eq!(newest_message(&tl), Some("card-1"));
        assert_eq!(newest_message(&timeline(vec![])), None);
        let only_day = timeline(vec![TimelineItem::Day(DayView::default())]);
        assert_eq!(newest_message(&only_day), None);
    }
}
