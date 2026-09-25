//! Helix-style modal keymap: normal and insert modes, and a `:` command line.
//!
//! In normal mode every range is a block over one grapheme, as in Helix. The
//! core knows nothing about modes; they live here.

use std::cell::RefCell;

use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::editor::{Buffer, View};
use nib_plugin::nib::plugin::types::{
    CursorShape, Edit, KeyCode, KeyEvent, Modifiers, SelRange, Selection, Span, UndoMode,
};
use nib_plugin::nib::plugin::ui::{Panel, Side};
use nib_plugin::nib::plugin::{commands, editor, input, settings, ui};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Insert,
}

struct CommandLine {
    panel: Panel,
    input: String,
}

struct Helix {
    mode: Mode,
    /// The display column `j` and `k` aim for, kept across short lines.
    column: Option<u32>,
    /// Whether the current insert session has edited yet, so later edits
    /// join the same undo step.
    inserted: bool,
    /// Insert mode was entered with `a`: leaving it moves the cursor back
    /// onto the last inserted grapheme, as Helix does.
    appending: bool,
    command_line: Option<CommandLine>,
}

thread_local! {
    static HELIX: RefCell<Helix> = const {
        RefCell::new(Helix {
            mode: Mode::Normal,
            column: None,
            inserted: false,
            appending: false,
            command_line: None,
        })
    };
}

struct Plugin;

impl Guest for Plugin {
    fn init(_config: String) -> Result<(), String> {
        input::push_layer();
        HELIX.with_borrow_mut(|helix| {
            helix.set_mode(Mode::Normal);
            to_blocks(&editor::active_view());
        });
        Ok(())
    }

    fn handle_key(ev: KeyEvent) -> KeyResult {
        HELIX.with_borrow_mut(|helix| helix.handle_key(ev))
    }
}

nib_plugin::export!(Plugin);

impl Helix {
    fn handle_key(&mut self, ev: KeyEvent) -> KeyResult {
        if self.command_line.is_some() {
            self.command_line_key(ev);
            return KeyResult::Handled;
        }
        let view = editor::active_view();
        let handled = match self.mode {
            Mode::Normal => self.normal_key(&view, ev),
            Mode::Insert => self.insert_key(&view, ev),
        };
        if handled {
            KeyResult::Handled
        } else {
            KeyResult::Pass
        }
    }

    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        let (label, style, shape) = match mode {
            Mode::Normal => (" NOR ", "ui.mode.normal", CursorShape::Block),
            Mode::Insert => (" INS ", "ui.mode.insert", CursorShape::Bar),
        };
        editor::active_view().set_cursor_shape(shape);
        ui::set_status("mode", Side::Left, 0, &[span(label, style)]);
    }

    fn normal_key(&mut self, view: &View, ev: KeyEvent) -> bool {
        if !ev.modifiers.is_empty() {
            return false;
        }
        let vertical = matches!(
            ev.code,
            KeyCode::Char('j' | 'k') | KeyCode::Down | KeyCode::Up
        );
        if !vertical {
            self.column = None;
        }
        match ev.code {
            KeyCode::Char('h') | KeyCode::Left => {
                move_blocks(view, |buffer, pos| buffer.prev_grapheme(pos).unwrap_or(pos))
            }
            KeyCode::Char('l') | KeyCode::Right => move_blocks(view, |buffer, pos| {
                let next = buffer.next_grapheme(pos).unwrap_or(pos);
                if next < buffer.len() { next } else { pos }
            }),
            KeyCode::Char('j') | KeyCode::Down => self.move_lines(view, 1),
            KeyCode::Char('k') | KeyCode::Up => self.move_lines(view, -1),
            KeyCode::Char('i') => self.insert_at(view, |range| range.anchor.min(range.head)),
            KeyCode::Char('a') => {
                self.insert_at(view, |range| range.anchor.max(range.head));
                self.appending = true;
            }
            KeyCode::Char('o') => self.open_below(view),
            KeyCode::Char('d') => delete_blocks(view),
            KeyCode::Char('u') => {
                if view.undo() {
                    to_blocks(view);
                }
            }
            KeyCode::Char('U') => {
                if view.redo() {
                    to_blocks(view);
                }
            }
            KeyCode::Char(':') => self.open_command_line(),
            _ => return false,
        }
        true
    }

    fn move_lines(&mut self, view: &View, lines: i32) {
        let buffer = view.buffer();
        let mut column = self.column;
        let ranges = view
            .selection()
            .ranges
            .iter()
            .map(|range| {
                let pos = cursor(&buffer, range);
                match view.move_vertically(pos, lines, column) {
                    Ok((pos, aimed)) => {
                        column = Some(aimed);
                        block(&buffer, pos)
                    }
                    Err(_) => *range,
                }
            })
            .collect();
        set_ranges(view, ranges);
        self.column = column;
    }

    fn insert_at(&mut self, view: &View, at: impl Fn(&SelRange) -> u64) {
        let ranges = view
            .selection()
            .ranges
            .iter()
            .map(|range| point(at(range)))
            .collect();
        set_ranges(view, ranges);
        self.start_insert();
    }

    fn start_insert(&mut self) {
        self.inserted = false;
        self.appending = false;
        self.set_mode(Mode::Insert);
    }

    /// Opens a line below each cursor, indented like the cursor's line.
    fn open_below(&mut self, view: &View) {
        let buffer = view.buffer();
        let edits: Vec<Edit> = view
            .selection()
            .ranges
            .iter()
            .map(|range| {
                let pos = cursor(&buffer, range);
                let end = line_end(&buffer, pos);
                let text = format!("\n{}", indentation(&buffer, pos));
                // Put the cursor after the insertion by making it the edit
                // point; the mapped point moves past inserted text.
                Edit {
                    start: end,
                    end,
                    text,
                }
            })
            .collect();
        let ranges = edits
            .iter()
            .map(|edit| point(edit.start))
            .collect::<Vec<_>>();
        set_ranges(view, ranges);
        self.start_insert();
        self.edit(view, &edits);
    }

    fn insert_key(&mut self, view: &View, ev: KeyEvent) -> bool {
        let plain = ev.modifiers.is_empty();
        match ev.code {
            KeyCode::Escape => {
                if self.appending {
                    move_blocks(view, |buffer, pos| buffer.prev_grapheme(pos).unwrap_or(pos));
                } else {
                    to_blocks(view);
                }
                self.set_mode(Mode::Normal);
            }
            KeyCode::Char(c) if plain => self.insert_text(view, &c.to_string()),
            KeyCode::Enter => {
                let buffer = view.buffer();
                let edits: Vec<Edit> = view
                    .selection()
                    .ranges
                    .iter()
                    .map(|range| {
                        let text = format!("\n{}", indentation(&buffer, range.head));
                        Edit {
                            start: range.head,
                            end: range.head,
                            text,
                        }
                    })
                    .collect();
                self.edit(view, &edits);
            }
            KeyCode::Tab => {
                let indent = match settings::get("indent").as_deref() {
                    Some("\"tab\"") => "\t".to_string(),
                    Some(n) => " ".repeat(n.parse().unwrap_or(4)),
                    None => "    ".to_string(),
                };
                self.insert_text(view, &indent);
            }
            KeyCode::Backspace => {
                let buffer = view.buffer();
                let edits = deletions(view, |range| {
                    let start = buffer.prev_grapheme(range.head).ok()?;
                    (start < range.head).then_some((start, range.head))
                });
                self.edit(view, &edits);
            }
            KeyCode::Delete => {
                let buffer = view.buffer();
                let edits = deletions(view, |range| {
                    let end = buffer.next_grapheme(range.head).ok()?;
                    (end > range.head).then_some((range.head, end))
                });
                self.edit(view, &edits);
            }
            KeyCode::Left | KeyCode::Right => {
                let buffer = view.buffer();
                let left = ev.code == KeyCode::Left;
                let ranges = view
                    .selection()
                    .ranges
                    .iter()
                    .map(|range| {
                        let moved = if left {
                            buffer.prev_grapheme(range.head)
                        } else {
                            buffer.next_grapheme(range.head)
                        };
                        point(moved.unwrap_or(range.head))
                    })
                    .collect();
                set_ranges(view, ranges);
            }
            KeyCode::Up | KeyCode::Down => {
                let lines = if ev.code == KeyCode::Up { -1 } else { 1 };
                let ranges = view
                    .selection()
                    .ranges
                    .iter()
                    .map(
                        |range| match view.move_vertically(range.head, lines, None) {
                            Ok((pos, _)) => point(pos),
                            Err(_) => *range,
                        },
                    )
                    .collect();
                set_ranges(view, ranges);
            }
            _ => return false,
        }
        true
    }

    fn insert_text(&mut self, view: &View, text: &str) {
        let edits: Vec<Edit> = view
            .selection()
            .ranges
            .iter()
            .map(|range| Edit {
                start: range.head,
                end: range.head,
                text: text.to_string(),
            })
            .collect();
        self.edit(view, &edits);
    }

    /// Applies edits of this insert session as one undo step.
    fn edit(&mut self, view: &View, edits: &[Edit]) {
        if edits.is_empty() {
            return;
        }
        let undo = if self.inserted {
            UndoMode::Merge
        } else {
            UndoMode::NewStep
        };
        let version = view.buffer().version();
        if view.apply(version, edits, None, undo).is_ok() {
            self.inserted = true;
        }
    }

    fn open_command_line(&mut self) {
        let panel = Panel::new(&[vec![span(":", "")]]);
        panel.set_cursor(Some((0, 1)));
        self.command_line = Some(CommandLine {
            panel,
            input: String::new(),
        });
    }

    fn command_line_key(&mut self, ev: KeyEvent) {
        let Some(line) = self.command_line.as_mut() else {
            return;
        };
        match ev.code {
            KeyCode::Escape => self.command_line = None,
            KeyCode::Enter => {
                let input = std::mem::take(&mut line.input);
                self.command_line = None;
                run_command_line(input.trim());
            }
            KeyCode::Backspace if line.input.is_empty() => self.command_line = None,
            KeyCode::Backspace => {
                line.input.pop();
            }
            KeyCode::Char(c) if ev.modifiers.is_empty() || ev.modifiers == Modifiers::SHIFT => {
                line.input.push(c);
            }
            _ => {}
        }
        if let Some(line) = &self.command_line {
            let text = format!(":{}", line.input);
            line.panel.update(&[vec![span(&text, "")]]);
            line.panel.set_cursor(Some((0, text.len() as u32)));
        }
    }
}

fn run_command_line(input: &str) {
    let (command, arg) = input.split_once(' ').unwrap_or((input, ""));
    let arg = arg.trim();
    let result = match command {
        "w" | "write" => save(),
        "q" | "quit" => quit(false),
        "q!" | "quit!" => quit(true),
        "wq" | "x" => save().and_then(|()| quit(false)),
        "o" | "open" | "e" | "edit" if !arg.is_empty() => {
            let args = format!(r#"{{"path":{}}}"#, json_string(arg));
            commands::call("buffer.open", &args).map(|_| ())
        }
        "o" | "open" | "e" | "edit" => Err(format!(":{command} needs a path")),
        "" => Ok(()),
        _ => Err(format!("unknown command: {command}")),
    };
    if let Err(err) = result {
        ui::show_message(&err);
    }
}

fn save() -> Result<(), String> {
    commands::call("buffer.save", "{}")?;
    let path = editor::active_view().buffer().path().unwrap_or_default();
    ui::show_message(&format!("{path} written"));
    Ok(())
}

fn quit(force: bool) -> Result<(), String> {
    commands::call("editor.quit", &format!(r#"{{"force":{force}}}"#)).map(|_| ())
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn span(text: &str, style: &str) -> Span {
    Span {
        text: text.into(),
        style: style.into(),
    }
}

fn point(pos: u64) -> SelRange {
    SelRange {
        anchor: pos,
        head: pos,
    }
}

/// A block over the grapheme at `pos`, or a point at the end of the text.
fn block(buffer: &Buffer, pos: u64) -> SelRange {
    let end = buffer.next_grapheme(pos).unwrap_or(pos);
    SelRange {
        anchor: pos,
        head: end,
    }
}

/// Where the cursor of `range` is drawn: on the last grapheme of a forward
/// range, as the core draws it.
fn cursor(buffer: &Buffer, range: &SelRange) -> u64 {
    if range.head > range.anchor {
        buffer.prev_grapheme(range.head).unwrap_or(range.head)
    } else {
        range.head
    }
}

fn move_blocks(view: &View, to: impl Fn(&Buffer, u64) -> u64) {
    let buffer = view.buffer();
    let ranges = view
        .selection()
        .ranges
        .iter()
        .map(|range| block(&buffer, to(&buffer, cursor(&buffer, range))))
        .collect();
    set_ranges(view, ranges);
}

/// Turns every range into a block at its cursor, as after leaving insert
/// mode or undoing.
fn to_blocks(view: &View) {
    move_blocks(view, |_, pos| pos);
}

fn set_ranges(view: &View, ranges: Vec<SelRange>) {
    let primary = view.selection().primary;
    let _ = view.set_selection(&Selection { ranges, primary });
}

fn delete_blocks(view: &View) {
    let edits = deletions(view, |range| {
        let (start, end) = (range.anchor.min(range.head), range.anchor.max(range.head));
        (start < end).then_some((start, end))
    });
    if edits.is_empty() {
        return;
    }
    let version = view.buffer().version();
    if view.apply(version, &edits, None, UndoMode::NewStep).is_ok() {
        to_blocks(view);
    }
}

fn deletions(view: &View, range_to_delete: impl Fn(&SelRange) -> Option<(u64, u64)>) -> Vec<Edit> {
    view.selection()
        .ranges
        .iter()
        .filter_map(range_to_delete)
        .map(|(start, end)| Edit {
            start,
            end,
            text: String::new(),
        })
        .collect()
}

/// The position of the line break ending the line of `pos`, or the end of
/// the text.
fn line_end(buffer: &Buffer, pos: u64) -> u64 {
    let line = buffer.line_of(pos).unwrap_or(0);
    match buffer.line_start(line + 1) {
        Some(next) => next - 1,
        None => buffer.len(),
    }
}

/// The leading whitespace of the line of `pos`.
fn indentation(buffer: &Buffer, pos: u64) -> String {
    let line = buffer.line_of(pos).unwrap_or(0);
    let start = buffer.line_start(line).unwrap_or(0);
    let text = buffer
        .slice(start, line_end(buffer, pos))
        .unwrap_or_default();
    text.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}
