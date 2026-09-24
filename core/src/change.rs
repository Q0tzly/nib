use ropey::Rope;

use crate::Error;
use crate::grapheme::check_position;

/// Replaces the byte range `start..end` of the original text with `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

impl Edit {
    pub fn new(start: usize, end: usize, text: impl Into<String>) -> Self {
        Self {
            start,
            end,
            text: text.into(),
        }
    }

    pub fn insert(pos: usize, text: impl Into<String>) -> Self {
        Self::new(pos, pos, text)
    }

    pub fn delete(start: usize, end: usize) -> Self {
        Self::new(start, end, "")
    }
}

/// Which side of an insertion a mapped position ends up on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assoc {
    Before,
    After,
}

/// Edits validated against one version of a text: sorted, non-overlapping,
/// and on char boundaries. All offsets refer to the text before the change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeSet {
    edits: Vec<Edit>,
}

impl ChangeSet {
    pub fn new(mut edits: Vec<Edit>, text: &Rope) -> Result<Self, Error> {
        // Stable, so insertions at the same position keep the given order.
        edits.sort_by_key(|edit| edit.start);
        let mut prev_end = 0;
        for edit in &edits {
            check_position(text, edit.start)?;
            check_position(text, edit.end)?;
            if edit.end < edit.start {
                return Err(Error::InvalidPosition(edit.end));
            }
            if edit.start < prev_end {
                return Err(Error::OverlappingEdits);
            }
            prev_end = edit.end;
        }
        Ok(Self { edits })
    }

    pub fn edits(&self) -> &[Edit] {
        &self.edits
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// Applies the edits and returns the change set that undoes them.
    pub(crate) fn apply(&self, text: &mut Rope) -> ChangeSet {
        let mut removed = Vec::with_capacity(self.edits.len());
        // Back to front, so earlier offsets stay valid.
        for edit in self.edits.iter().rev() {
            let start = text.byte_to_char(edit.start);
            let end = text.byte_to_char(edit.end);
            removed.push(text.slice(start..end).to_string());
            text.remove(start..end);
            text.insert(start, &edit.text);
        }
        removed.reverse();

        let mut delta = 0isize;
        let inverse = self
            .edits
            .iter()
            .zip(removed)
            .map(|(edit, removed)| {
                let start = edit.start.saturating_add_signed(delta);
                delta += edit.text.len() as isize - (edit.end - edit.start) as isize;
                Edit::new(start, start + edit.text.len(), removed)
            })
            .collect();
        ChangeSet { edits: inverse }
    }

    /// Maps a position in the original text to the changed text.
    ///
    /// `assoc` decides where a position ends up when text is inserted exactly
    /// at it, or when it is strictly inside a replaced range. The start of a
    /// replaced range always stays at the start of the replacement.
    pub fn map_pos(&self, pos: usize, assoc: Assoc) -> usize {
        let mut delta = 0isize;
        for edit in &self.edits {
            if pos < edit.start {
                break;
            }
            let start = edit.start.saturating_add_signed(delta);
            if edit.start == edit.end {
                if pos == edit.start && assoc == Assoc::Before {
                    return start;
                }
            } else if pos < edit.end {
                return if pos == edit.start || assoc == Assoc::Before {
                    start
                } else {
                    start + edit.text.len()
                };
            }
            delta += edit.text.len() as isize - (edit.end - edit.start) as isize;
        }
        pos.saturating_add_signed(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, edits: Vec<Edit>) -> (String, String) {
        let mut rope = Rope::from_str(text);
        let changes = ChangeSet::new(edits, &rope).unwrap();
        let inverse = changes.apply(&mut rope);
        let changed = rope.to_string();
        inverse.apply(&mut rope);
        (changed, rope.to_string())
    }

    #[test]
    fn applies_multiple_edits_and_inverts() {
        let (changed, restored) = apply(
            "hello world",
            vec![
                Edit::new(6, 11, "nib"),
                Edit::insert(0, ">> "),
                Edit::delete(4, 5),
            ],
        );
        assert_eq!(changed, ">> hell nib");
        assert_eq!(restored, "hello world");
    }

    #[test]
    fn multibyte_text() {
        let (changed, restored) = apply("あいう", vec![Edit::new(3, 6, "ABC")]);
        assert_eq!(changed, "あABCう");
        assert_eq!(restored, "あいう");
    }

    #[test]
    fn rejects_invalid_edits() {
        let text = Rope::from_str("あいう");
        assert!(matches!(
            ChangeSet::new(vec![Edit::delete(1, 3)], &text),
            Err(Error::InvalidPosition(1))
        ));
        assert!(matches!(
            ChangeSet::new(vec![Edit::delete(0, 10)], &text),
            Err(Error::InvalidPosition(10))
        ));
        assert!(matches!(
            ChangeSet::new(vec![Edit::delete(0, 6), Edit::delete(3, 9)], &text),
            Err(Error::OverlappingEdits)
        ));
    }

    #[test]
    fn insertions_at_same_position_keep_order() {
        let (changed, restored) = apply("ab", vec![Edit::insert(1, "x"), Edit::insert(1, "y")]);
        assert_eq!(changed, "axyb");
        assert_eq!(restored, "ab");
    }

    #[test]
    fn map_positions() {
        let text = Rope::from_str("0123456789");
        // "0123456789" -> "01XYZ4567"
        let changes =
            ChangeSet::new(vec![Edit::new(2, 4, "XYZ"), Edit::delete(8, 10)], &text).unwrap();
        assert_eq!(changes.map_pos(1, Assoc::After), 1);
        assert_eq!(changes.map_pos(2, Assoc::Before), 2);
        assert_eq!(changes.map_pos(2, Assoc::After), 2);
        assert_eq!(changes.map_pos(3, Assoc::Before), 2);
        assert_eq!(changes.map_pos(3, Assoc::After), 5);
        assert_eq!(changes.map_pos(4, Assoc::Before), 5);
        assert_eq!(changes.map_pos(6, Assoc::After), 7);
        assert_eq!(changes.map_pos(9, Assoc::After), 9);
        assert_eq!(changes.map_pos(10, Assoc::After), 9);
    }

    #[test]
    fn map_position_at_insertion() {
        let text = Rope::from_str("abc");
        let changes = ChangeSet::new(vec![Edit::insert(1, "xy")], &text).unwrap();
        assert_eq!(changes.map_pos(1, Assoc::Before), 1);
        assert_eq!(changes.map_pos(1, Assoc::After), 3);
        assert_eq!(changes.map_pos(2, Assoc::Before), 4);
    }
}
