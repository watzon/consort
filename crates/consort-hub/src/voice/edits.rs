//! Learning from edits: the sentences that the user removed from or added
//! to a draft, and the changes that several edits agree on.

use consort_proto::ids::ApprovalId;
use consort_proto::model::{DraftPair, VOICE_EDITS_AGREE_MIN, placeholders};
use consort_proto::turn_input::{VoiceEditPair, VoiceEditPattern};

/// The sentences of a text: split at line breaks and after `.`, `!` and
/// `?`. Empty sentences are left out.
pub(crate) fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut current = String::new();
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            current.push(c);
            if matches!(c, '.' | '!' | '?') && chars.peek().is_none_or(|n| n.is_whitespace()) {
                let sentence = current.trim().to_owned();
                if !sentence.is_empty() {
                    out.push(sentence);
                }
                current.clear();
            }
        }
        let sentence = current.trim().to_owned();
        if !sentence.is_empty() {
            out.push(sentence);
        }
    }
    out
}

/// A sentence for comparing: lowercase, single spaces, no punctuation at
/// the ends.
fn key(sentence: &str) -> String {
    sentence
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_owned()
}

/// What the user removed from and added to one draft.
pub(crate) fn diff(draft: &str, final_text: &str) -> VoiceEditPair {
    let before = sentences(draft);
    let after = sentences(final_text);
    let before_keys: Vec<String> = before.iter().map(|s| key(s)).collect();
    let after_keys: Vec<String> = after.iter().map(|s| key(s)).collect();
    VoiceEditPair {
        approval_id: None,
        removed: before
            .iter()
            .zip(&before_keys)
            .filter(|(_, k)| !k.is_empty() && !after_keys.contains(k))
            .map(|(s, _)| s.clone())
            .collect(),
        added: after
            .iter()
            .zip(&after_keys)
            .filter(|(_, k)| !k.is_empty() && !before_keys.contains(k))
            .map(|(s, _)| s.clone())
            .collect(),
    }
}

/// The changes that at least [`VOICE_EDITS_AGREE_MIN`] edits agree on,
/// most first, with the cards of the edits. Each edit counts once for a
/// change.
pub(crate) fn agreeing(pairs: &[VoiceEditPair]) -> Vec<VoiceEditPattern> {
    let mut counts: Vec<(String, String, String, u32, Vec<ApprovalId>)> = Vec::new();
    for pair in pairs {
        let mut seen: Vec<(String, String)> = Vec::new();
        for (change, list) in [("removed", &pair.removed), ("added", &pair.added)] {
            for sentence in list {
                let k = key(sentence);
                if k.is_empty() || seen.contains(&(change.to_owned(), k.clone())) {
                    continue;
                }
                seen.push((change.to_owned(), k.clone()));
                let ids: Vec<ApprovalId> = pair.approval_id.into_iter().collect();
                match counts
                    .iter_mut()
                    .find(|(c, kk, _, _, _)| c == change && *kk == k)
                {
                    Some(entry) => {
                        entry.3 += 1;
                        entry.4.extend(ids);
                    }
                    None => counts.push((change.to_owned(), k, sentence.clone(), 1, ids)),
                }
            }
        }
    }
    let min = u32::try_from(VOICE_EDITS_AGREE_MIN).unwrap_or(3);
    let mut out: Vec<VoiceEditPattern> = counts
        .into_iter()
        .filter(|(_, _, _, n, _)| *n >= min)
        .map(|(change, _, text, count, approval_ids)| VoiceEditPattern {
            change,
            text,
            count,
            approval_ids,
        })
        .collect();
    out.sort_by_key(|a| std::cmp::Reverse(a.count));
    out
}

/// The edits of draft pairs, for a `voice_edits` event, with their cards.
/// Addresses, phone numbers and links in the sentences become
/// placeholders.
pub(crate) fn edit_pairs(pairs: &[DraftPair]) -> Vec<VoiceEditPair> {
    pairs
        .iter()
        .map(|p| {
            let mut pair = diff(&placeholders(&p.draft), &placeholders(&p.final_text));
            pair.approval_id = Some(p.approval_id);
            pair
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diff_lists_removed_and_added_sentences() {
        let pair = diff(
            "Hi Dana,\nHope you are well. The contract works for me.\nBest,\nChris",
            "Hi Dana,\nThe contract works for me!\nChris",
        );
        assert_eq!(pair.removed, vec!["Hope you are well.", "Best,"]);
        assert!(pair.added.is_empty());
    }

    #[test]
    fn three_agreeing_edits_make_a_pattern_and_two_do_not() {
        let edit = |removed: &str| VoiceEditPair {
            approval_id: None,
            removed: vec![removed.to_owned()],
            added: vec![],
        };
        let pairs = vec![
            edit("Hope you are well."),
            edit("hope you are well"),
            edit("Hope you are well!"),
            edit("Thanks a lot."),
            edit("Thanks a lot."),
        ];
        let found = agreeing(&pairs);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].change, "removed");
        assert_eq!(found[0].count, 3);
        assert!(agreeing(&pairs[..2]).is_empty());
    }
}
