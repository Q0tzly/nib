use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation;

use crate::editor::{Editor, Menu};
use crate::grid::{Cursor, Grid, Style, display_width};

const TAB_WIDTH: u16 = 4;
const MAIN_MENU: &str =
    "[r] restart plugins  [w] save all and quit  [q] quit  [any other key] back";

impl Editor {
    /// Draws the editor into `grid`, resizing it to the editor size.
    /// Returns the cursor if it is on screen.
    pub fn render(&self, grid: &mut Grid) -> Option<Cursor> {
        let (width, height) = self.size();
        grid.reset(width, height);
        if width == 0 || height == 0 {
            return None;
        }
        let text_rows = if height > 1 { height - 1 } else { height };
        let cursor = self.render_text(grid, text_rows);
        if height > 1 {
            self.render_status(grid, height - 1);
        }
        cursor
    }

    fn render_text(&self, grid: &mut Grid, rows: u16) -> Option<Cursor> {
        let width = grid.width();
        let text = self.buffer().text();
        let view = self.view();
        let cursor_pos = view.cursor(text);
        let style = Style::default();
        let mut cursor = None;

        for row in 0..rows {
            let line_idx = view.top_line + row as usize;
            if line_idx >= text.len_lines() {
                break;
            }
            let line: Cow<str> = text.line(line_idx).into();
            let line = line.strip_suffix('\n').unwrap_or(&line);
            let mut offset = text.line_to_byte(line_idx);
            let mut x = 0;
            for grapheme in line.graphemes(true) {
                if x >= width {
                    break;
                }
                if offset == cursor_pos {
                    cursor = Some((x, row));
                }
                if grapheme == "\t" {
                    let end = (x + TAB_WIDTH - x % TAB_WIDTH).min(width);
                    while x < end {
                        x = grid.put_grapheme(x, row, " ", style);
                    }
                } else {
                    x = grid.put_grapheme(x, row, grapheme, style);
                }
                offset += grapheme.len();
            }
            // The cursor can also sit on the line break or at the end of the text.
            if offset == cursor_pos && x < width {
                cursor = Some((x, row));
            }
        }

        cursor.map(|(x, y)| Cursor {
            x,
            y,
            shape: view.cursor_shape,
        })
    }

    fn render_status(&self, grid: &mut Grid, y: u16) {
        let style = Style {
            reverse: true,
            ..Style::default()
        };
        grid.fill_row(0, y, style);
        match self.menu() {
            Some(Menu::Main) => {
                grid.put_str(1, y, MAIN_MENU, style);
                return;
            }
            Some(Menu::ConfirmQuit) => {
                let n = self.modified_buffers();
                let prompt = format!(
                    "quit without saving {n} modified buffer{}? [y] quit  [any other key] back",
                    if n == 1 { "" } else { "s" }
                );
                grid.put_str(1, y, &prompt, style);
                return;
            }
            None => {}
        }

        let buffer = self.buffer();
        let name = buffer
            .path()
            .map_or_else(|| "[scratch]".into(), |path| path.display().to_string());
        let modified = if buffer.is_modified() { " [+]" } else { "" };
        let mut left = format!("{name}{modified}");
        if let Some(message) = self.message() {
            left = format!("{left}  {message}");
        }
        grid.put_str(1, y, &left, style);

        let text = buffer.text();
        let cursor = self.view().cursor(text);
        let line = text.byte_to_line(cursor);
        let before_cursor: Cow<str> = text.byte_slice(text.line_to_byte(line)..cursor).into();
        let column = before_cursor.graphemes(true).count();
        let position = format!("{}:{} ", line + 1, column + 1);
        let right = match self.key_hint() {
            Some(hint) => format!("{hint}  {position}"),
            None => position,
        };
        let right_width: u16 = right.graphemes(true).map(display_width).sum();
        if let Some(x) = grid.width().checked_sub(right_width) {
            grid.put_str(x, y, &right, style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::CursorShape;
    use crate::selection::Selection;

    fn render(editor: &Editor) -> (Vec<String>, Option<Cursor>) {
        let mut grid = Grid::default();
        let cursor = editor.render(&mut grid);
        let rows = (0..grid.height()).map(|y| grid.row_text(y)).collect();
        (rows, cursor)
    }

    #[test]
    fn draws_lines_and_status() {
        let mut editor = Editor::with_text("fn main() {\n\tnib();\n}\n");
        editor.resize(30, 5);
        let (rows, cursor) = render(&editor);
        assert_eq!(rows[0], format!("{:30}", "fn main() {"));
        assert_eq!(rows[1], format!("{:30}", "    nib();"));
        assert_eq!(rows[2], format!("{:30}", "}"));
        assert_eq!(rows[3], " ".repeat(30));
        assert_eq!(rows[4], " [scratch]  Ctrl-g: menu  1:1 ");
        assert_eq!(
            cursor,
            Some(Cursor {
                x: 0,
                y: 0,
                shape: CursorShape::Block
            })
        );
    }

    #[test]
    fn tabs_align_to_tab_stops() {
        let mut editor = Editor::with_text("ab\tc\n\t\td");
        editor.resize(12, 3);
        let (rows, _) = render(&editor);
        assert_eq!(rows[0], "ab  c       ");
        assert_eq!(rows[1], "        d   ");
    }

    #[test]
    fn long_lines_are_cut() {
        let mut editor = Editor::with_text("0123456789");
        editor.resize(6, 2);
        let (rows, _) = render(&editor);
        assert_eq!(rows[0], "012345");
    }

    #[test]
    fn cursor_follows_graphemes_and_tabs() {
        let mut editor = Editor::with_text("\tあx\n");
        editor.resize(20, 3);
        let text = editor.buffer().text().clone();
        // A forward range over "あ": the cursor sits on "あ".
        editor.view_mut().selection =
            Selection::new(vec![crate::Range::new(1, 4)], 0, &text).unwrap();
        let (rows, cursor) = render(&editor);
        let cursor = cursor.unwrap();
        assert_eq!((cursor.x, cursor.y), (4, 0));
        assert!(rows[2].ends_with("1:2 "));

        // At the end of the line.
        editor.view_mut().selection = Selection::point(5);
        let (_, cursor) = render(&editor);
        assert_eq!(cursor.map(|c| (c.x, c.y)), Some((7, 0)));
    }

    #[test]
    fn menu_replaces_the_status_line() {
        let mut editor = Editor::with_text("a");
        editor.resize(100, 2);
        editor.handle_key(crate::KeyEvent::ctrl('g'));
        let (rows, _) = render(&editor);
        assert!(rows[1].starts_with(" [r] restart plugins"), "{}", rows[1]);
    }

    #[test]
    fn tiny_sizes() {
        let mut editor = Editor::with_text("abc");
        editor.resize(0, 0);
        assert_eq!(render(&editor).1, None);
        editor.resize(2, 1);
        assert_eq!(render(&editor).0, vec!["ab".to_string()]);
    }
}
