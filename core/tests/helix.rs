//! Drives the Helix-style keymap plugin in `plugins/helix`. Build it first
//! with `cargo xtask build-plugins`.

use std::{env, fs};

use nib_core::{CursorShape, Editor, KeyCode, KeyEvent, Modifiers, Range, Selection};

mod common;
use common::{plugin_dir, screen, type_keys};

fn editor_with_text(text: &str) -> Editor {
    let mut editor = Editor::default();
    editor.load_plugin(&plugin_dir("helix")).unwrap();
    if !text.is_empty() {
        // Type it in insert mode, then go back to the start.
        type_keys(&mut editor, &format!("i{text}<esc>"));
        editor.view_mut().selection =
            Selection::new(vec![Range::new(0, 1)], 0, editor.buffer().text()).unwrap();
    }
    editor.resize(40, 6);
    editor
}

fn text(editor: &Editor) -> String {
    editor.buffer().text().to_string()
}

fn cursor(editor: &Editor) -> usize {
    editor.view().cursor(editor.buffer().text())
}

fn status_line(editor: &Editor) -> String {
    screen(editor).last().unwrap().clone()
}

#[test]
fn switches_between_normal_and_insert_modes() {
    let mut editor = editor_with_text("");
    assert!(status_line(&editor).starts_with(" NOR "));
    assert_eq!(editor.view().cursor_shape, CursorShape::Block);

    type_keys(&mut editor, "i");
    assert!(status_line(&editor).starts_with(" INS "));
    assert_eq!(editor.view().cursor_shape, CursorShape::Bar);

    type_keys(&mut editor, "hello<esc>");
    assert_eq!(text(&editor), "hello");
    assert!(status_line(&editor).starts_with(" NOR "));
    // The whole insert is one undo step.
    type_keys(&mut editor, "u");
    assert_eq!(text(&editor), "");
}

#[test]
fn moves_by_graphemes_and_keeps_the_column_across_short_lines() {
    let mut editor = editor_with_text("hello world\nhi\nhello again\n");
    type_keys(&mut editor, "llllllll");
    assert_eq!(cursor(&editor), 8);
    type_keys(&mut editor, "j");
    // "hi" is short: the cursor sits on its line break.
    assert_eq!(cursor(&editor), 14);
    type_keys(&mut editor, "j");
    assert_eq!(cursor(&editor), 15 + 8);
    type_keys(&mut editor, "kkh");
    assert_eq!(cursor(&editor), 7);
}

#[test]
fn deletes_and_undoes() {
    let mut editor = editor_with_text("abc");
    type_keys(&mut editor, "ld");
    assert_eq!(text(&editor), "ac");
    assert_eq!(cursor(&editor), 1);
    type_keys(&mut editor, "u");
    assert_eq!(text(&editor), "abc");
    type_keys(&mut editor, "U");
    assert_eq!(text(&editor), "ac");
}

#[test]
fn opens_lines_and_types_like_an_editor() {
    let mut editor = editor_with_text("  foo\nbar");
    type_keys(&mut editor, "ox<esc>");
    assert_eq!(text(&editor), "  foo\n  x\nbar");

    // As in Helix, the block now sits on the line break after "x". Insert
    // before it: Enter keeps the indentation, Backspace removes one char, and
    // Tab inserts the indent from the core settings.
    type_keys(&mut editor, "i<ret>y<bs><bs><tab>z<esc>");
    assert_eq!(text(&editor), "  foo\n  x\n     z\nbar");
}

#[test]
fn leaving_append_moves_back_onto_the_inserted_text() {
    let mut editor = editor_with_text("abc");
    type_keys(&mut editor, "aX<esc>");
    assert_eq!(text(&editor), "aXbc");
    assert_eq!(cursor(&editor), 1);
    type_keys(&mut editor, "iY<esc>");
    assert_eq!(text(&editor), "aYXbc");
    assert_eq!(cursor(&editor), 2);
}

#[test]
fn command_line_saves_and_quits() {
    let path = env::temp_dir().join(format!("nib-{}-helix.txt", std::process::id()));
    fs::write(&path, "one\n").unwrap();
    let mut editor = Editor::default();
    editor.open(&path).unwrap();
    editor.load_plugin(&plugin_dir("helix")).unwrap();
    editor.resize(40, 6);

    type_keys(&mut editor, "ix<esc>:w");
    let rows = screen(&editor);
    assert_eq!(rows[4].trim_end(), ":w");
    type_keys(&mut editor, "<ret>");
    let saved = fs::read_to_string(&path).unwrap();
    assert_eq!(saved, "xone\n");
    assert!(editor.message().unwrap().ends_with("written"));

    type_keys(&mut editor, "iy<esc>:q<ret>");
    assert_eq!(editor.message(), Some("1 buffer has unsaved changes"));
    assert!(!editor.should_quit());
    type_keys(&mut editor, ":nope<ret>");
    assert_eq!(editor.message(), Some("unknown command: nope"));

    type_keys(&mut editor, ":q!<ret>");
    fs::remove_file(&path).unwrap();
    assert!(editor.should_quit());
}

fn primary(editor: &Editor) -> (usize, usize) {
    let range = editor.view().selection.primary();
    (range.anchor, range.head)
}

fn ranges(editor: &Editor) -> Vec<(usize, usize)> {
    editor
        .view()
        .selection
        .ranges()
        .iter()
        .map(|r| (r.anchor, r.head))
        .collect()
}

#[test]
fn word_motions_select_what_they_pass_over() {
    let mut editor = editor_with_text("foo bar baz\n");
    type_keys(&mut editor, "w");
    assert_eq!(primary(&editor), (0, 4));
    type_keys(&mut editor, "w");
    assert_eq!(primary(&editor), (4, 8));
    type_keys(&mut editor, "e");
    assert_eq!(primary(&editor), (8, 11));
    type_keys(&mut editor, "b");
    assert_eq!(primary(&editor), (11, 8));
    // Punctuation is its own word, but not for WORD motions.
    let mut editor = editor_with_text("a.b c\n");
    type_keys(&mut editor, "w");
    assert_eq!(primary(&editor), (1, 2));
    let mut editor = editor_with_text("a.b c\n");
    type_keys(&mut editor, "W");
    assert_eq!(primary(&editor), (0, 4));
    // A count repeats the motion.
    let mut editor = editor_with_text("foo bar baz\n");
    type_keys(&mut editor, "2w");
    assert_eq!(primary(&editor), (4, 8));
}

#[test]
fn finds_chars_on_the_line() {
    let mut editor = editor_with_text("hello world\n");
    type_keys(&mut editor, "fo");
    assert_eq!(primary(&editor), (0, 5));
    type_keys(&mut editor, "to");
    assert_eq!(primary(&editor), (4, 7));
    type_keys(&mut editor, "Fh");
    assert_eq!(primary(&editor), (7, 0));
    // Nothing found: the selection stays.
    type_keys(&mut editor, "fz");
    assert_eq!(primary(&editor), (7, 0));
}

#[test]
fn goes_to_lines_and_line_positions() {
    let mut editor = editor_with_text("one\n  two\nthree\n");
    type_keys(&mut editor, "ge");
    assert_eq!(cursor(&editor), 10);
    type_keys(&mut editor, "gl");
    assert_eq!(cursor(&editor), 14);
    type_keys(&mut editor, "gh");
    assert_eq!(cursor(&editor), 10);
    type_keys(&mut editor, "2gg");
    assert_eq!(cursor(&editor), 4);
    type_keys(&mut editor, "gs");
    assert_eq!(cursor(&editor), 6);
    type_keys(&mut editor, "gg");
    assert_eq!(cursor(&editor), 0);
    type_keys(&mut editor, "3l");
    assert_eq!(cursor(&editor), 3);
}

#[test]
fn select_mode_extends_selections() {
    let mut editor = editor_with_text("abcdef\n");
    type_keys(&mut editor, "lvll");
    assert_eq!(primary(&editor), (1, 4));
    assert!(status_line(&editor).starts_with(" SEL "));
    type_keys(&mut editor, "hhhh");
    assert_eq!(primary(&editor), (2, 0));
    type_keys(&mut editor, "<esc>l");
    assert!(status_line(&editor).starts_with(" NOR "));
    assert_eq!(primary(&editor), (1, 2));
}

#[test]
fn selects_lines_and_everything() {
    let mut editor = editor_with_text("a\nb\nc");
    type_keys(&mut editor, "x");
    assert_eq!(primary(&editor), (0, 2));
    type_keys(&mut editor, "x");
    assert_eq!(primary(&editor), (0, 4));
    type_keys(&mut editor, "%");
    assert_eq!(primary(&editor), (0, 5));
    type_keys(&mut editor, ";");
    assert_eq!(primary(&editor), (4, 5));
}

#[test]
fn copies_cursors_down_and_keeps_or_flips_them() {
    let mut editor = editor_with_text("ab\ncd\nef\n");
    type_keys(&mut editor, "lCC");
    assert_eq!(ranges(&editor), vec![(1, 2), (4, 5), (7, 8)]);
    assert_eq!(primary(&editor), (7, 8));
    type_keys(&mut editor, ",");
    assert_eq!(ranges(&editor), vec![(7, 8)]);
    type_keys(&mut editor, "e");
    let (anchor, head) = primary(&editor);
    editor.handle_key(KeyEvent {
        code: KeyCode::Char(';'),
        modifiers: Modifiers {
            alt: true,
            ..Modifiers::default()
        },
    });
    assert_eq!(primary(&editor), (head, anchor));
}

#[test]
fn half_page_scrolling_moves_view_and_cursor() {
    let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
    let mut editor = editor_with_text(&text);
    editor.resize(40, 12); // 11 text rows: half a page is 5 lines
    let line = |editor: &Editor| editor.buffer().line_of(cursor(editor)).unwrap();
    // The view moves half a page, and the cursor is pushed out of the
    // scroll margin (5 lines) instead of pulling the view back.
    editor.handle_key(KeyEvent::ctrl('d'));
    assert_eq!(editor.view().top_line, 5);
    assert_eq!(line(&editor), 10);
    editor.handle_key(KeyEvent::ctrl('d'));
    assert_eq!(editor.view().top_line, 10);
    assert_eq!(line(&editor), 15);
    editor.handle_key(KeyEvent::ctrl('u'));
    assert_eq!(editor.view().top_line, 5);
    assert_eq!(line(&editor), 10);
    editor.handle_key(KeyEvent::ctrl('u'));
    assert_eq!(editor.view().top_line, 0);
    assert_eq!(line(&editor), 5);
}
