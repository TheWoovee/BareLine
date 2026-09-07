// SPDX-License-Identifier: MPL-2.0
use bareline_document::{DocumentSnapshot, TextOffset};
include!(concat!(env!("OUT_DIR"), "/word.rs"));
fn is_word(c: char) -> bool {
    let code = c as u32;
    let index = WORD.partition_point(|range| range.1 < code);
    WORD.get(index).is_some_and(|range| range.0 <= code)
}
/// Bounded lookaround in the original text domain, including outside selection scope.
pub fn boundaries(snapshot: &DocumentSnapshot, start: usize, end: usize) -> Option<bool> {
    let mut before = start.saturating_sub(4);
    while !snapshot.is_boundary(TextOffset(before)) {
        before += 1;
    }
    let prefix = snapshot
        .read(TextOffset(before)..TextOffset(start), 4)
        .ok()?;
    if prefix.chars().next_back().is_some_and(is_word) {
        return Some(false);
    }
    let mut after = end.saturating_add(4).min(snapshot.len());
    while !snapshot.is_boundary(TextOffset(after)) {
        after -= 1;
    }
    let suffix = snapshot.read(TextOffset(end)..TextOffset(after), 4).ok()?;
    Some(!suffix.chars().next().is_some_and(is_word))
}
