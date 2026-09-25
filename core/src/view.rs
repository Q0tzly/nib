use ropey::Rope;

use crate::grapheme;
use crate::grid::CursorShape;
use crate::selection::Selection;

/// A buffer shown on screen, with its own selection and scroll position.
pub struct View {
    pub buffer: usize,
    pub selection: Selection,
    /// The first line shown.
    pub top_line: usize,
    /// The first display column shown, for lines wider than the screen.
    pub left_col: u32,
    pub cursor_shape: CursorShape,
}

impl View {
    pub fn new(buffer: usize) -> Self {
        Self {
            buffer,
            selection: Selection::point(0),
            top_line: 0,
            left_col: 0,
            cursor_shape: CursorShape::Block,
        }
    }

    /// Where the cursor is drawn. The head of a forward range is just past
    /// its last grapheme, so the cursor sits on that grapheme, as in Helix.
    pub fn cursor(&self, text: &Rope) -> usize {
        let range = self.selection.primary();
        if range.head > range.anchor {
            grapheme::prev_boundary(text, range.head)
        } else {
            range.head
        }
    }
}
