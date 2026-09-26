//! The screen as a grid of monospace cells. Frontends draw it; nothing here
//! knows about terminals.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
    #[default]
    Reset,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
}

impl Style {
    /// `over` on top of `self`: its colors where it has them, and its
    /// attributes added.
    pub fn patch(self, over: Style) -> Style {
        let pick = |base: Color, top: Color| if top == Color::Reset { base } else { top };
        Style {
            fg: pick(self.fg, over.fg),
            bg: pick(self.bg, over.bg),
            bold: self.bold || over.bold,
            italic: self.italic || over.italic,
            underline: self.underline || over.underline,
            reverse: self.reverse || over.reverse,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Symbol {
    Char(char),
    /// A grapheme made of several chars.
    Str(Box<str>),
    /// The right half of a double-width grapheme.
    Continuation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub symbol: Symbol,
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            symbol: Symbol::Char(' '),
            style: Style::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub x: u16,
    pub y: u16,
    pub shape: CursorShape,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grid {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
}

impl Grid {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            cells: vec![Cell::default(); width as usize * height as usize],
        }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn cell(&self, x: u16, y: u16) -> &Cell {
        &self.cells[self.index(x, y)]
    }

    /// Resizes the grid and resets every cell.
    pub fn reset(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.cells.clear();
        self.cells
            .resize(width as usize * height as usize, Cell::default());
    }

    /// Fills cells from `x` to the end of the row with blanks in `style`.
    pub fn fill_row(&mut self, x: u16, y: u16, style: Style) {
        for x in x..self.width {
            let i = self.index(x, y);
            self.cells[i] = Cell {
                symbol: Symbol::Char(' '),
                style,
            };
        }
    }

    /// Puts one grapheme at `x` and returns the column after it. A grapheme
    /// that does not fit before the end of the row is replaced with blanks.
    pub fn put_grapheme(&mut self, x: u16, y: u16, grapheme: &str, style: Style) -> u16 {
        let width = display_width(grapheme);
        if x as usize + width as usize > self.width as usize {
            self.fill_row(x, y, style);
            return self.width;
        }
        let i = self.index(x, y);
        self.cells[i] = Cell {
            symbol: symbol(grapheme),
            style,
        };
        if width == 2 {
            self.cells[i + 1] = Cell {
                symbol: Symbol::Continuation,
                style,
            };
        }
        x + width
    }

    /// Puts `text` from `x`, cut off at the end of the row. Returns the
    /// column after the last grapheme put.
    pub fn put_str(&mut self, mut x: u16, y: u16, text: &str, style: Style) -> u16 {
        for grapheme in graphemes(text) {
            if x >= self.width {
                break;
            }
            x = self.put_grapheme(x, y, grapheme, style);
        }
        x
    }

    fn index(&self, x: u16, y: u16) -> usize {
        assert!(x < self.width && y < self.height, "({x}, {y}) out of grid");
        y as usize * self.width as usize + x as usize
    }

    #[cfg(test)]
    pub(crate) fn row_text(&self, y: u16) -> String {
        (0..self.width)
            .filter_map(|x| match &self.cell(x, y).symbol {
                Symbol::Char(c) => Some(c.to_string()),
                Symbol::Str(s) => Some(s.to_string()),
                Symbol::Continuation => None,
            })
            .collect()
    }
}

/// Control characters are never sent to the terminal as-is, since they
/// could move the cursor or start escape sequences.
const REPLACEMENT: char = '\u{fffd}';

/// The graphemes of `text`, split as `UnicodeSegmentation::graphemes`
/// splits them, but with a shortcut: two ASCII characters other than
/// "\r\n" are always two graphemes, and code is mostly ASCII. Splitting
/// with the Unicode tables was 40% of drawing a screen of code.
pub fn graphemes(text: &str) -> impl Iterator<Item = &str> {
    grapheme_indices(text).map(|(_, grapheme)| grapheme)
}

/// `graphemes` with where each starts in `text`.
pub fn grapheme_indices(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let rest = &text[at..];
        let bytes = rest.as_bytes();
        let first = *bytes.first()?;
        let ascii_pair = match bytes.get(1) {
            None => first.is_ascii(),
            Some(&next) => first.is_ascii() && next.is_ascii() && (first, next) != (b'\r', b'\n'),
        };
        let len = if ascii_pair {
            1
        } else {
            rest.graphemes(true).next().map_or(rest.len(), str::len)
        };
        let start = at;
        at += len;
        Some((start, &rest[..len]))
    })
}

pub fn display_width(grapheme: &str) -> u16 {
    // One byte is ASCII: a control, drawn as one replacement character, or
    // a one-cell character.
    if grapheme.len() == 1 || is_control(grapheme) {
        return 1;
    }
    // Zero-width graphemes still take a cell so the cursor can sit on them.
    grapheme.width().clamp(1, 2) as u16
}

fn symbol(grapheme: &str) -> Symbol {
    if is_control(grapheme) {
        return Symbol::Char(REPLACEMENT);
    }
    let mut chars = grapheme.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Symbol::Char(c),
        _ => Symbol::Str(grapheme.into()),
    }
}

fn is_control(grapheme: &str) -> bool {
    grapheme.chars().next().is_some_and(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphemes_split_as_unicode_says() {
        for text in [
            "",
            "fn main() {}",
            "a\r\nb\n",
            "e\u{301}x",
            "日本語 and ascii",
            "👨‍👩‍👧 family",
            "🇯🇵🇺🇸",
            "x\u{200d}y",
            "\t\u{7}",
        ] {
            let ours: Vec<_> = grapheme_indices(text).collect();
            let unicode: Vec<_> = text.grapheme_indices(true).collect();
            assert_eq!(ours, unicode, "{text:?}");
        }
    }

    #[test]
    fn wide_graphemes_take_two_cells() {
        let mut grid = Grid::new(5, 1);
        let end = grid.put_str(0, 0, "あいう", Style::default());
        assert_eq!(end, 5);
        // "う" does not fit in the last cell.
        assert_eq!(grid.row_text(0), "あい ");
        assert_eq!(grid.cell(1, 0).symbol, Symbol::Continuation);
    }

    #[test]
    fn control_characters_are_replaced() {
        let mut grid = Grid::new(4, 1);
        grid.put_str(0, 0, "a\x1b[b", Style::default());
        assert_eq!(grid.row_text(0), "a\u{fffd}[b");
    }

    #[test]
    fn multi_char_graphemes_stay_in_one_cell() {
        let mut grid = Grid::new(3, 1);
        grid.put_str(0, 0, "e\u{301}x", Style::default());
        assert_eq!(grid.cell(0, 0).symbol, Symbol::Str("e\u{301}".into()));
        assert_eq!(grid.row_text(0), "e\u{301}x ");
    }
}
