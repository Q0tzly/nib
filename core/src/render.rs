use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation;

use crate::editor::{Editor, Menu};
use crate::grid::{Cursor, CursorShape, Grid, Style, display_width};
use crate::ui::{Panel, Side, Span, StyledLine, Theme};

impl Editor {
    /// Draws the editor into `grid`, resizing it to the editor size.
    /// Returns the cursor if it is on screen.
    pub fn render(&self, grid: &mut Grid) -> Option<Cursor> {
        let (width, height) = self.size();
        grid.reset(width, height);
        if width == 0 || height == 0 {
            return None;
        }
        let text_rows = self.text_rows();
        let status_row = (height > 1).then(|| height - 1);
        let mut cursor = self.render_text(grid, text_rows);
        let panel_end = status_row.unwrap_or(height);
        if let Some(panel_cursor) = self.render_panels(grid, text_rows, panel_end) {
            cursor = Some(panel_cursor);
        }
        if let Some(y) = status_row {
            self.render_status(grid, y);
        }
        cursor
    }

    /// Rows left for text above the panels and the status line.
    pub(crate) fn text_rows(&self) -> u16 {
        let menu = self.menu_lines().len().min(u16::MAX as usize) as u16;
        self.state().text_rows().saturating_sub(menu)
    }

    /// The plugin list shown while the core menu is open, below any panels.
    fn menu_lines(&self) -> Vec<StyledLine> {
        let selected = match self.menu() {
            Some(Menu::Main) => None,
            Some(Menu::Plugin(id)) => Some(id),
            Some(Menu::ConfirmQuit) | None => return Vec::new(),
        };
        let plugins = self.plugins();
        if plugins.is_empty() {
            return vec![vec![plain(" no plugins are loaded")]];
        }
        plugins
            .iter()
            .enumerate()
            .map(|(id, plugin)| {
                let state = match (&plugin.last_error, plugin.enabled) {
                    (_, true) if !plugin.has_code => "languages".to_string(),
                    (_, true) => "running".to_string(),
                    (Some(err), false) => format!("disabled: {err}"),
                    (None, false) => "disabled".to_string(),
                };
                let text = format!(
                    " {}  {:<12} {:<8} slow calls: {:<4} {state}",
                    id + 1,
                    plugin.name,
                    plugin.version,
                    plugin.slow_calls,
                );
                let style = if selected == Some(id) {
                    "ui.menu.selected"
                } else {
                    ""
                };
                vec![Span {
                    text,
                    style: style.into(),
                }]
            })
            .collect()
    }

    /// Draws panels from row `top` down to `end`, oldest first. Returns the
    /// cursor of the last panel that has one.
    fn render_panels(&self, grid: &mut Grid, top: u16, end: u16) -> Option<Cursor> {
        let mut cursor = None;
        let mut y = top;
        let menu = self.menu_lines();
        let menu_panel = Panel {
            id: 0,
            owner: 0,
            lines: menu,
            cursor: None,
        };
        for panel in self.state().panels.iter().chain([&menu_panel]) {
            for (i, line) in panel.lines.iter().enumerate() {
                if y >= end {
                    return cursor;
                }
                let x = put_line(grid, &self.state().theme, 0, y, line, Style::default());
                debug_assert!(x <= grid.width());
                if let Some((cursor_line, byte)) = panel.cursor
                    && cursor_line as usize == i
                {
                    let text: String = line.iter().map(|span| span.text.as_str()).collect();
                    let before = text.get(..byte as usize).unwrap_or(&text);
                    let x: u16 = before.graphemes(true).map(display_width).sum();
                    cursor = (x < grid.width()).then_some(Cursor {
                        x,
                        y,
                        shape: CursorShape::Bar,
                    });
                }
                y += 1;
            }
        }
        cursor
    }

    fn render_text(&self, grid: &mut Grid, rows: u16) -> Option<Cursor> {
        let width = u32::from(grid.width());
        let tab_width = u32::from(self.settings().tab_width);
        let text = self.buffer().text();
        let view = self.view();
        let ranges = view.selection.ranges();
        let selected = |offset: usize| {
            let i = ranges.partition_point(|range| range.to() <= offset);
            ranges.get(i).is_some_and(|range| range.from() <= offset)
        };
        let normal = Style::default();
        let selection_bg = self
            .state()
            .theme
            .style("ui.selection")
            .unwrap_or(normal)
            .bg;
        let visible_start = text.line_to_byte(view.top_line.min(text.len_lines() - 1));
        let visible_end = self
            .buffer()
            .line_start(view.top_line + rows as usize)
            .unwrap_or(text.len_bytes());
        let syntax = self.state().syntax_styles(visible_start..visible_end);
        let style_at = |offset: usize| {
            let style = syntax
                .as_ref()
                .and_then(|styles| {
                    styles
                        .get(offset.checked_sub(visible_start)?)
                        .copied()
                        .flatten()
                })
                .unwrap_or(normal);
            if selected(offset) {
                Style {
                    bg: selection_bg,
                    ..style
                }
            } else {
                style
            }
        };
        let cursor_pos = view.cursor(text);
        let left = view.left_col;
        let mut cursor = None;

        for row in 0..rows {
            let line_idx = view.top_line + row as usize;
            if line_idx >= text.len_lines() {
                break;
            }
            let line: Cow<str> = text.line(line_idx).into();
            let line = line.strip_suffix('\n').unwrap_or(&line);
            let mut offset = text.line_to_byte(line_idx);
            // Display column within the line; the screen shows `left..left + width`.
            let mut column = 0u32;
            for grapheme in line.graphemes(true) {
                if column >= left + width {
                    break;
                }
                let next = if grapheme == "\t" {
                    column + tab_width - column % tab_width
                } else {
                    column + u32::from(display_width(grapheme))
                };
                if offset == cursor_pos && column >= left {
                    cursor = Some(((column - left) as u16, row));
                }
                let style = style_at(offset);
                if next > left {
                    let x = column.saturating_sub(left) as u16;
                    // Tabs and graphemes cut by the left edge show as blanks.
                    if grapheme == "\t" || column < left {
                        let end = ((next - left).min(width)) as u16;
                        let mut x = x;
                        while x < end {
                            x = grid.put_grapheme(x, row, " ", style);
                        }
                    } else {
                        grid.put_grapheme(x, row, grapheme, style);
                    }
                }
                column = next;
                offset += grapheme.len();
            }
            // The cursor, and a selected line break, show just past the text.
            if column >= left && column < left + width {
                let x = (column - left) as u16;
                if offset == cursor_pos {
                    cursor = Some((x, row));
                }
                if offset < text.len_bytes() && selected(offset) {
                    grid.put_grapheme(x, row, " ", style_at(offset));
                }
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
                let choose = if self.plugins().is_empty() {
                    ""
                } else {
                    "[1-9] choose a plugin  "
                };
                let keys = format!(
                    "{choose}[r] restart all  [w] save all and quit  [q] quit  [any other key] back"
                );
                grid.put_str(1, y, &keys, style);
                return;
            }
            Some(Menu::Plugin(id)) => {
                let plugin = &self.plugins()[id];
                let toggle = if plugin.enabled { "disable" } else { "enable" };
                let reload = if plugin.reloadable {
                    "  [l] reload from disk"
                } else {
                    ""
                };
                let keys = format!(
                    "{}: [r] restart  [d] {toggle}{reload}  [any other key] back",
                    plugin.name
                );
                grid.put_str(1, y, &keys, style);
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

        let state = self.state();
        let items = |side| {
            let mut items: Vec<_> = state.status.iter().filter(|i| i.side == side).collect();
            items.sort_by_key(|item| item.priority);
            items
        };

        let buffer = self.buffer();
        let text = buffer.text();
        let cursor = self.view().cursor(text);
        let line = text.byte_to_line(cursor);
        let before_cursor: Cow<str> = text.byte_slice(text.line_to_byte(line)..cursor).into();
        let column = before_cursor.graphemes(true).count();
        let mut right: StyledLine = Vec::new();
        for item in items(Side::Right) {
            right.extend(item.content.iter().cloned());
            right.push(plain(" "));
        }
        if let Some(hint) = self.key_hint() {
            right.push(plain(&format!("{hint}  ")));
        }
        right.push(plain(&format!("{}:{} ", line + 1, column + 1)));
        let right_width: u16 = right
            .iter()
            .flat_map(|span| span.text.graphemes(true))
            .map(display_width)
            .sum();
        let right_start = grid.width().saturating_sub(right_width);

        let mut x = 0;
        for item in items(Side::Left) {
            x = put_line(grid, &state.theme, x, y, &item.content, style);
        }
        let name = buffer
            .path()
            .map_or_else(|| "[scratch]".into(), |path| path.display().to_string());
        let modified = if buffer.is_modified() { " [+]" } else { "" };
        // Shorten a long path from the left so the modified mark stays visible.
        let room = right_start.saturating_sub(x + 2 + modified.len() as u16);
        x = grid.put_str(
            x,
            y,
            &format!(" {}{modified}", truncate_left(&name, room)),
            style,
        );
        if let Some(message) = self.message() {
            grid.put_str(x, y, &format!("  {message}"), style);
        }
        if right_start > x || self.message().is_none() {
            put_line(grid, &state.theme, right_start, y, &right, style);
        }
    }
}

/// `text` cut to `width` columns, keeping its end: "…/src/lib.rs".
fn truncate_left(text: &str, width: u16) -> Cow<'_, str> {
    let total: u16 = text.graphemes(true).map(display_width).sum();
    if total <= width {
        return Cow::Borrowed(text);
    }
    let mut kept = 1; // for the ellipsis
    let mut start = text.len();
    for (i, grapheme) in text.grapheme_indices(true).rev() {
        let w = display_width(grapheme);
        if kept + w > width {
            break;
        }
        kept += w;
        start = i;
    }
    Cow::Owned(format!("…{}", &text[start..]))
}

fn plain(text: &str) -> Span {
    Span {
        text: text.into(),
        style: String::new(),
    }
}

/// Puts spans from `x`, styled by the theme, or `base` for names it does not
/// know. Returns the column after the last grapheme put.
fn put_line(grid: &mut Grid, theme: &Theme, mut x: u16, y: u16, line: &[Span], base: Style) -> u16 {
    for span in line {
        let style = theme.style(&span.style).unwrap_or(base);
        x = grid.put_str(x, y, &span.text, style);
    }
    x
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
    fn tab_width_comes_from_config() {
        let mut editor = Editor::with_text("\tx");
        editor.apply_config(crate::Config::parse("[core]\ntab-width = 8").unwrap());
        editor.resize(12, 2);
        let (rows, _) = render(&editor);
        assert_eq!(rows[0], "        x   ");
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
        editor.resize(100, 3);
        editor.handle_key(crate::KeyEvent::ctrl('g'));
        let (rows, _) = render(&editor);
        assert!(rows[1].starts_with(" no plugins are loaded"), "{}", rows[1]);
        assert!(rows[2].starts_with(" [r] restart all"), "{}", rows[2]);
    }

    fn span(text: &str, style: &str) -> Span {
        Span {
            text: text.into(),
            style: style.into(),
        }
    }

    #[test]
    fn panels_sit_above_the_status_line_and_take_the_cursor() {
        let mut editor = Editor::with_text("a\nb\nc\nd");
        editor.resize(20, 5);
        editor.state_mut().panels.push(crate::ui::Panel {
            id: 1,
            owner: 0,
            lines: vec![vec![span(":", ""), span("wq", "")]],
            cursor: Some((0, 3)),
        });
        let (rows, cursor) = render(&editor);
        assert_eq!(editor.text_rows(), 3);
        assert_eq!(rows[2].trim_end(), "c");
        assert_eq!(rows[3].trim_end(), ":wq");
        assert_eq!(
            cursor,
            Some(Cursor {
                x: 3,
                y: 3,
                shape: CursorShape::Bar
            })
        );
    }

    #[test]
    fn status_items_are_ordered_by_priority() {
        let mut editor = Editor::with_text("a");
        editor.resize(40, 2);
        let item = |id: &str, side, priority, text: &str| crate::ui::StatusItem {
            owner: 0,
            id: id.into(),
            side,
            priority,
            content: vec![span(text, "ui.mode.normal")],
        };
        let status = &mut editor.state_mut().status;
        status.push(item("b", Side::Left, 1, "B"));
        status.push(item("a", Side::Left, 0, "A"));
        status.push(item("r", Side::Right, 0, "R"));
        let (rows, _) = render(&editor);
        assert_eq!(rows[1], "AB [scratch]        R Ctrl-g: menu  1:1 ");
    }

    #[test]
    fn scrolls_sideways_to_the_cursor() {
        let mut editor = Editor::with_text("0123456789abcdef\nxy");
        editor.resize(8, 3);
        editor.view_mut().selection = Selection::point(12);
        editor.handle_key(crate::KeyEvent::new(crate::KeyCode::Escape));
        let (rows, cursor) = render(&editor);
        assert_eq!(editor.view().left_col, 5);
        assert_eq!(rows[0], "56789abc");
        assert_eq!(rows[1], "        ");
        assert_eq!(cursor.map(|c| (c.x, c.y)), Some((7, 0)));
    }

    #[test]
    fn selections_are_highlighted_including_line_breaks() {
        let mut editor = Editor::with_text("ab\ncd");
        editor.resize(6, 3);
        let text = editor.buffer().text().clone();
        editor.view_mut().selection =
            Selection::new(vec![crate::Range::new(1, 4)], 0, &text).unwrap();
        let mut grid = Grid::default();
        editor.render(&mut grid);
        let selected =
            |x, y| grid.cell(x, y).style == Theme::default().style("ui.selection").unwrap();
        assert!(!selected(0, 0));
        assert!(selected(1, 0));
        assert!(selected(2, 0), "the line break");
        assert!(selected(0, 1));
        assert!(!selected(1, 1));
    }

    #[test]
    fn long_paths_keep_the_modified_mark() {
        assert_eq!(truncate_left("a/b/c.rs", 20), "a/b/c.rs");
        assert_eq!(truncate_left("very/long/path/c.rs", 8), "…th/c.rs");
        let path =
            std::env::temp_dir().join(format!("nib-{}-{}.txt", std::process::id(), "x".repeat(60)));
        let mut editor = Editor::default();
        editor.open(&path).unwrap();
        let buffer = &mut editor.state_mut().buffers[0];
        let version = buffer.version();
        buffer
            .apply(
                version,
                vec![crate::Edit::insert(0, "a")],
                &Selection::point(0),
                None,
                crate::UndoMode::NewStep,
            )
            .unwrap();
        editor.resize(40, 2);
        let (rows, _) = render(&editor);
        assert!(rows[1].contains("x.txt [+]"), "{}", rows[1]);
        assert!(rows[1].starts_with(" …"), "{}", rows[1]);
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
