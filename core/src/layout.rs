//! Display columns within a line, counted the way the text is drawn: tabs
//! reach the next tab stop and wide graphemes take two cells.

use std::borrow::Cow;

use ropey::Rope;

use crate::grid::{display_width, grapheme_indices, graphemes};

/// The text of `line` without its line break.
fn line_text(text: &Rope, line: usize) -> Cow<'_, str> {
    let line: Cow<str> = text.line(line).into();
    match line {
        Cow::Borrowed(s) => Cow::Borrowed(s.strip_suffix('\n').unwrap_or(s)),
        Cow::Owned(mut s) => {
            if s.ends_with('\n') {
                s.pop();
            }
            Cow::Owned(s)
        }
    }
}

fn advance(column: u32, grapheme: &str, tab_width: u16) -> u32 {
    if grapheme == "\t" {
        let tab = u32::from(tab_width);
        column + tab - column % tab
    } else {
        column + u32::from(display_width(grapheme))
    }
}

/// The display column at which the grapheme starting at `pos` is drawn.
pub fn column_of(text: &Rope, pos: usize, tab_width: u16) -> u32 {
    let line = text.byte_to_line(pos);
    let start = text.line_to_byte(line);
    let before: Cow<str> = text.byte_slice(start..pos).into();
    graphemes(&before).fold(0, |column, grapheme| advance(column, grapheme, tab_width))
}

/// The position in `line` whose grapheme covers display `column`, or the end
/// of the line when it is shorter.
pub fn pos_at_column(text: &Rope, line: usize, column: u32, tab_width: u16) -> usize {
    let start = text.line_to_byte(line);
    let content = line_text(text, line);
    let mut current = 0;
    for (offset, grapheme) in grapheme_indices(&content) {
        let next = advance(current, grapheme, tab_width);
        if column < next {
            return start + offset;
        }
        current = next;
    }
    start + content.len()
}

/// Moves `lines` lines from `pos`, aiming for `column` or the column of
/// `pos`. Returns the new position and the column aimed for.
pub fn move_vertically(
    text: &Rope,
    pos: usize,
    lines: i32,
    column: Option<u32>,
    tab_width: u16,
) -> (usize, u32) {
    let column = column.unwrap_or_else(|| column_of(text, pos, tab_width));
    let last_line = text.len_lines() - 1;
    let line = text
        .byte_to_line(pos)
        .saturating_add_signed(lines as isize)
        .min(last_line);
    (pos_at_column(text, line, column, tab_width), column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_count_tabs_and_wide_graphemes() {
        let text = Rope::from_str("\tあx\n");
        assert_eq!(column_of(&text, 0, 4), 0);
        assert_eq!(column_of(&text, 1, 4), 4);
        assert_eq!(column_of(&text, 4, 4), 6);
        assert_eq!(pos_at_column(&text, 0, 2, 4), 0);
        assert_eq!(pos_at_column(&text, 0, 5, 4), 1);
        assert_eq!(pos_at_column(&text, 0, 99, 4), 5);
    }

    #[test]
    fn keeps_the_column_across_short_lines() {
        let text = Rope::from_str("hello world\nhi\nhello again");
        let (pos, column) = move_vertically(&text, 8, 1, None, 4);
        // "hi" is shorter: the cursor sits at its end.
        assert_eq!((pos, column), (14, 8));
        let (pos, _) = move_vertically(&text, pos, 1, Some(column), 4);
        assert_eq!(pos, 15 + 8);
    }

    #[test]
    fn stops_at_the_first_and_last_lines() {
        let text = Rope::from_str("a\nb\nc");
        assert_eq!(move_vertically(&text, 2, -5, None, 4).0, 0);
        assert_eq!(move_vertically(&text, 2, 5, None, 4).0, 4);
    }
}
