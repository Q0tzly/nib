//! Writes the difference between two grids to the terminal.

use std::io::{self, Write};

use crossterm::style::{self, Attribute, SetAttribute};
use crossterm::{cursor, queue, terminal};
use nib_core::{Color, Cursor, CursorShape, Grid, Style, Symbol};

/// Positions of cells in `next` that differ from `prev`. A double-width
/// grapheme is reported once, at its left half. Every cell is reported
/// when the size changed. Rows are compared whole first: most of them are
/// the same from one frame to the next.
pub fn changed_cells(prev: &Grid, next: &Grid) -> Vec<(u16, u16)> {
    let resized = prev.width() != next.width() || prev.height() != next.height();
    let mut changed = Vec::new();
    for y in 0..next.height() {
        let row = next.row(y);
        let old = (!resized).then(|| prev.row(y));
        if old == Some(row) {
            continue;
        }
        for (x, cell) in row.iter().enumerate() {
            if old.is_some_and(|old| old[x] == *cell) {
                continue;
            }
            let x = x as u16;
            let head = match cell.symbol {
                Symbol::Continuation => x - 1,
                _ => x,
            };
            if changed.last() != Some(&(head, y)) {
                changed.push((head, y));
            }
        }
    }
    changed
}

/// Writes the `changed` cells of `next`, from `changed_cells`, and puts the
/// cursor.
pub fn draw(
    out: &mut impl Write,
    next: &Grid,
    changed: &[(u16, u16)],
    cursor: Option<Cursor>,
) -> io::Result<()> {
    queue!(out, terminal::BeginSynchronizedUpdate, cursor::Hide)?;
    let mut style = None;
    let mut pos = None;
    for &(x, y) in changed {
        let cell = next.cell(x, y);
        if pos != Some((x, y)) {
            queue!(out, cursor::MoveTo(x, y))?;
        }
        if style != Some(cell.style) {
            set_style(out, cell.style)?;
            style = Some(cell.style);
        }
        match &cell.symbol {
            Symbol::Char(c) => queue!(out, style::Print(c))?,
            Symbol::Str(s) => queue!(out, style::Print(s))?,
            Symbol::Continuation => unreachable!("changed_cells reports left halves only"),
        }
        let wide = x + 1 < next.width() && next.cell(x + 1, y).symbol == Symbol::Continuation;
        pos = Some((x + if wide { 2 } else { 1 }, y));
    }
    queue!(out, SetAttribute(Attribute::Reset))?;
    if let Some(c) = cursor {
        queue!(
            out,
            cursor::MoveTo(c.x, c.y),
            cursor_style(c.shape),
            cursor::Show
        )?;
    }
    queue!(out, terminal::EndSynchronizedUpdate)?;
    Ok(())
}

fn set_style(out: &mut impl Write, s: Style) -> io::Result<()> {
    queue!(
        out,
        SetAttribute(Attribute::Reset),
        style::SetForegroundColor(color(s.fg)),
        style::SetBackgroundColor(color(s.bg)),
    )?;
    for (on, attribute) in [
        (s.bold, Attribute::Bold),
        (s.italic, Attribute::Italic),
        (s.underline, Attribute::Underlined),
        (s.reverse, Attribute::Reverse),
    ] {
        if on {
            queue!(out, SetAttribute(attribute))?;
        }
    }
    Ok(())
}

fn color(c: Color) -> style::Color {
    match c {
        Color::Reset => style::Color::Reset,
        Color::Indexed(n) => style::Color::AnsiValue(n),
        Color::Rgb(r, g, b) => style::Color::Rgb { r, g, b },
    }
}

fn cursor_style(shape: CursorShape) -> cursor::SetCursorStyle {
    match shape {
        CursorShape::Block => cursor::SetCursorStyle::SteadyBlock,
        CursorShape::Bar => cursor::SetCursorStyle::SteadyBar,
        CursorShape::Underline => cursor::SetCursorStyle::SteadyUnderScore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(width: u16, text: &str) -> Grid {
        let mut grid = Grid::new(width, 1);
        grid.put_str(0, 0, text, Style::default());
        grid
    }

    #[test]
    fn reports_only_changed_cells() {
        assert_eq!(
            changed_cells(&grid(4, "abcd"), &grid(4, "abXd")),
            vec![(2, 0)]
        );
        assert!(changed_cells(&grid(4, "abcd"), &grid(4, "abcd")).is_empty());
    }

    #[test]
    fn reports_every_cell_after_resize() {
        assert_eq!(changed_cells(&grid(2, "ab"), &grid(3, "ab")).len(), 3);
    }

    #[test]
    fn wide_graphemes_are_reported_once_at_the_left_half() {
        // "b" becomes the right half of "あ".
        assert_eq!(
            changed_cells(&grid(3, "abc"), &grid(3, "aあ")),
            vec![(1, 0)]
        );
        // "あ" at the same place, but the cell after it changed.
        assert_eq!(
            changed_cells(&grid(4, "あcd"), &grid(4, "あxd")),
            vec![(2, 0)]
        );
    }

    #[test]
    fn writes_changed_text_and_cursor() {
        let mut out = Vec::new();
        let cursor = Cursor {
            x: 1,
            y: 0,
            shape: CursorShape::Bar,
        };
        let (prev, next) = (grid(4, "abcd"), grid(4, "abXY"));
        draw(&mut out, &next, &changed_cells(&prev, &next), Some(cursor)).unwrap();
        let out = String::from_utf8(out).unwrap();
        // One move to column 3 (1-based), then both cells without moving again.
        assert!(out.contains("\x1b[1;3H"), "{out:?}");
        assert!(out.contains("XY"), "{out:?}");
        assert!(out.contains("\x1b[1;2H"), "{out:?}");
        assert!(!out.contains('a'));
    }
}
