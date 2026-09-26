use ropey::Rope;

use crate::Error;
use crate::change::{Assoc, ChangeSet};
use crate::grapheme;

/// A range between `anchor` and `head`. The head is where the cursor is, so
/// `head` may be before `anchor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub anchor: usize,
    pub head: usize,
}

impl Range {
    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    pub fn point(pos: usize) -> Self {
        Self::new(pos, pos)
    }

    pub fn from(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn to(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    fn with_bounds(&self, from: usize, to: usize) -> Self {
        if self.head < self.anchor {
            Self::new(to, from)
        } else {
            Self::new(from, to)
        }
    }

    fn map(&self, changes: &ChangeSet) -> Self {
        if self.is_empty() {
            return Self::point(changes.map_pos(self.head, Assoc::After));
        }
        // Text inserted at either edge stays outside the range.
        let from = changes.map_pos(self.from(), Assoc::After);
        let to = changes.map_pos(self.to(), Assoc::Before);
        self.with_bounds(from.min(to), from.max(to))
    }
}

/// One or more ranges, sorted and non-overlapping, one of which is primary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    ranges: Vec<Range>,
    primary: usize,
}

impl Selection {
    /// Validates `ranges` against `text`, expands them to grapheme
    /// boundaries, and merges overlapping ones.
    pub fn new(ranges: Vec<Range>, primary: usize, text: &Rope) -> Result<Self, Error> {
        if primary >= ranges.len() {
            return Err(Error::InvalidSelection);
        }
        for range in &ranges {
            grapheme::check_position(text, range.anchor)?;
            grapheme::check_position(text, range.head)?;
        }
        Ok(Self::normalized(ranges, primary, text))
    }

    pub fn point(pos: usize) -> Self {
        Self {
            ranges: vec![Range::point(pos)],
            primary: 0,
        }
    }

    pub fn ranges(&self) -> &[Range] {
        &self.ranges
    }

    pub fn primary(&self) -> Range {
        self.ranges[self.primary]
    }

    pub fn primary_index(&self) -> usize {
        self.primary
    }

    /// Maps the selection through `changes` made one after another. `text`
    /// is the text after all of them.
    pub fn map_all(&self, changes: &[ChangeSet], text: &Rope) -> Self {
        let ranges = self
            .ranges
            .iter()
            .map(|range| changes.iter().fold(*range, |range, set| range.map(set)))
            .collect();
        Self::normalized(ranges, self.primary, text)
    }

    /// Maps the selection through `changes`. `text` is the changed text.
    pub fn map(&self, changes: &ChangeSet, text: &Rope) -> Self {
        let ranges = self.ranges.iter().map(|range| range.map(changes)).collect();
        Self::normalized(ranges, self.primary, text)
    }

    fn normalized(ranges: Vec<Range>, primary: usize, text: &Rope) -> Self {
        let mut ranges: Vec<(Range, bool)> = ranges
            .into_iter()
            .enumerate()
            .map(|(i, range)| {
                let range = if range.is_empty() {
                    Range::point(grapheme::floor(text, range.head))
                } else {
                    range.with_bounds(
                        grapheme::floor(text, range.from()),
                        grapheme::ceil(text, range.to()),
                    )
                };
                (range, i == primary)
            })
            .collect();
        ranges.sort_by_key(|(range, _)| range.from());

        let mut merged: Vec<(Range, bool)> = Vec::with_capacity(ranges.len());
        for (range, is_primary) in ranges {
            match merged.last_mut() {
                Some((last, last_primary))
                    if range.from() < last.to() || range.from() == last.from() =>
                {
                    *last = last.with_bounds(last.from(), last.to().max(range.to()));
                    *last_primary |= is_primary;
                }
                _ => merged.push((range, is_primary)),
            }
        }

        let primary = merged.iter().position(|(_, p)| *p).unwrap_or(0);
        Self {
            ranges: merged.into_iter().map(|(range, _)| range).collect(),
            primary,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::Edit;

    #[test]
    fn rejects_invalid_selections() {
        let text = Rope::from_str("あい");
        assert!(matches!(
            Selection::new(vec![], 0, &text),
            Err(Error::InvalidSelection)
        ));
        assert!(matches!(
            Selection::new(vec![Range::point(0)], 1, &text),
            Err(Error::InvalidSelection)
        ));
        assert!(matches!(
            Selection::new(vec![Range::new(0, 1)], 0, &text),
            Err(Error::InvalidPosition(1))
        ));
    }

    #[test]
    fn merges_overlapping_ranges_and_keeps_primary() {
        let text = Rope::from_str("0123456789");
        let sel = Selection::new(
            vec![
                Range::new(6, 8),
                Range::new(0, 3),
                Range::new(2, 5),
                Range::point(8),
            ],
            2,
            &text,
        )
        .unwrap();
        assert_eq!(
            sel.ranges(),
            &[Range::new(0, 5), Range::new(6, 8), Range::point(8)]
        );
        assert_eq!(sel.primary(), Range::new(0, 5));
    }

    #[test]
    fn expands_to_grapheme_boundaries_keeping_direction() {
        let s = "ae\u{301}b";
        let text = Rope::from_str(s);
        let e = 1;
        let b = s.len() - 1;
        // Ends at the start of the combining mark, inside the grapheme "é".
        let sel = Selection::new(vec![Range::new(b, e + 1)], 0, &text).unwrap();
        assert_eq!(sel.primary(), Range::new(b, e));
        let sel = Selection::new(vec![Range::new(0, e + 1)], 0, &text).unwrap();
        assert_eq!(sel.primary(), Range::new(0, b));
    }

    #[test]
    fn maps_through_changes() {
        let mut text = Rope::from_str("abcdef");
        let sel = Selection::new(
            vec![Range::point(1), Range::new(2, 4), Range::new(5, 4)],
            1,
            &text,
        )
        .unwrap();
        // Insert at the cursor and at the start of the second range.
        let changes =
            ChangeSet::new(vec![Edit::insert(1, "XX"), Edit::insert(2, "Y")], &text).unwrap();
        changes.apply(&mut text);
        assert_eq!(text.to_string(), "aXXbYcdef");
        let mapped = sel.map(&changes, &text);
        assert_eq!(
            mapped.ranges(),
            &[Range::point(3), Range::new(5, 7), Range::new(8, 7)]
        );
        assert_eq!(mapped.primary_index(), 1);
    }

    #[test]
    fn range_covering_replaced_text_covers_the_replacement() {
        let mut text = Rope::from_str("abcdef");
        let sel = Selection::new(vec![Range::new(1, 3)], 0, &text).unwrap();
        let changes = ChangeSet::new(vec![Edit::new(1, 3, "XYZW")], &text).unwrap();
        changes.apply(&mut text);
        assert_eq!(sel.map(&changes, &text).primary(), Range::new(1, 5));
    }
}
