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

#[test]
fn yanks_and_pastes_by_chars_and_by_lines() {
    let mut editor = editor_with_text("hello world\n");
    type_keys(&mut editor, "wye");
    assert_eq!(primary(&editor), (6, 11));
    type_keys(&mut editor, "p");
    assert_eq!(text(&editor), "hello worldhello \n");
    assert_eq!(primary(&editor), (11, 17));
    type_keys(&mut editor, "u");
    assert_eq!(text(&editor), "hello world\n");

    let mut editor = editor_with_text("a\nb\n");
    type_keys(&mut editor, "xyjp");
    assert_eq!(text(&editor), "a\nb\na\n");
    assert_eq!(primary(&editor), (4, 6));
    type_keys(&mut editor, "ggP");
    assert_eq!(text(&editor), "a\na\nb\na\n");

    // After a last line without a line break.
    let mut editor = editor_with_text("a\nb");
    type_keys(&mut editor, "xyjp");
    assert_eq!(text(&editor), "a\nb\na");
    assert_eq!(primary(&editor), (4, 5));
}

#[test]
fn delete_yanks_and_change_undoes_as_one_step() {
    let mut editor = editor_with_text("abc");
    type_keys(&mut editor, "dp");
    assert_eq!(text(&editor), "bac");

    let mut editor = editor_with_text("abc");
    type_keys(&mut editor, "wcX<esc>");
    assert_eq!(text(&editor), "X");
    type_keys(&mut editor, "u");
    assert_eq!(text(&editor), "abc");
}

#[test]
fn replaces_indents_and_joins() {
    let mut editor = editor_with_text("ab\ncd");
    type_keys(&mut editor, "xrZ");
    assert_eq!(text(&editor), "ZZ\ncd");

    let mut editor = editor_with_text("a\n  b\n\nc");
    type_keys(&mut editor, "xx>");
    assert_eq!(text(&editor), "    a\n      b\n\nc");
    type_keys(&mut editor, "<<");
    assert_eq!(text(&editor), "a\nb\n\nc");

    let mut editor = editor_with_text("a\n  b\n\nc");
    type_keys(&mut editor, "J");
    assert_eq!(text(&editor), "a b\n\nc");
    type_keys(&mut editor, "J");
    assert_eq!(text(&editor), "a b\nc");
}

#[test]
fn inserts_at_line_ends_and_opens_above() {
    let mut editor = editor_with_text("  ab\n");
    type_keys(&mut editor, "AX<esc>");
    assert_eq!(text(&editor), "  abX\n");
    type_keys(&mut editor, "IY<esc>");
    assert_eq!(text(&editor), "  YabX\n");
    type_keys(&mut editor, "OZ<esc>");
    assert_eq!(text(&editor), "  Z\n  YabX\n");
    type_keys(&mut editor, "u");
    assert_eq!(text(&editor), "  YabX\n");
}

#[test]
fn searches_forward_and_backward_wrapping_around() {
    let mut editor = editor_with_text("one two one two\n");
    type_keys(&mut editor, "/two<ret>");
    assert_eq!(primary(&editor), (4, 7));
    type_keys(&mut editor, "n");
    assert_eq!(primary(&editor), (12, 15));
    type_keys(&mut editor, "n");
    assert_eq!(primary(&editor), (4, 7));
    assert_eq!(editor.message(), Some("search wrapped around"));
    type_keys(&mut editor, "N");
    assert_eq!(primary(&editor), (12, 15));
    type_keys(&mut editor, "?one<ret>");
    assert_eq!(primary(&editor), (8, 11));

    // `*` searches for the selection as plain text.
    let mut editor = editor_with_text("a.b axb a.b\n");
    type_keys(&mut editor, "E*n");
    assert_eq!(primary(&editor), (8, 11));

    type_keys(&mut editor, "/(<ret>");
    assert!(
        editor.message().unwrap().contains("unclosed"),
        "{:?}",
        editor.message()
    );
    type_keys(&mut editor, "/zzz<ret>");
    assert_eq!(editor.message(), Some("no matches for zzz"));
}

#[test]
fn selects_matches_inside_selections() {
    let mut editor = editor_with_text("foo bar foo\nfoo\n");
    type_keys(&mut editor, "xsfoo<ret>");
    assert_eq!(ranges(&editor), vec![(0, 3), (8, 11)]);
    type_keys(&mut editor, "cX<esc>");
    assert_eq!(text(&editor), "X bar X\nfoo\n");
}

#[test]
fn matches_brackets_and_selects_pairs() {
    let mut editor = editor_with_text("f(a, (b), c) \"q s\"\n");
    type_keys(&mut editor, "lmm");
    assert_eq!(cursor(&editor), 11);
    type_keys(&mut editor, "mm");
    assert_eq!(cursor(&editor), 1);
    type_keys(&mut editor, "llllllmi(");
    assert_eq!(primary(&editor), (6, 7));
    type_keys(&mut editor, "ma(");
    assert_eq!(primary(&editor), (5, 8));
    type_keys(&mut editor, "gglllllllllllllllma\"");
    assert_eq!(primary(&editor), (13, 18));
}

#[test]
fn switches_buffers() {
    let dir = env::temp_dir();
    let a = dir.join(format!("nib-{}-ga.txt", std::process::id()));
    let b = dir.join(format!("nib-{}-gb.txt", std::process::id()));
    fs::write(&a, "aaa\n").unwrap();
    fs::write(&b, "bbb\n").unwrap();
    let mut editor = Editor::default();
    editor.open(&a).unwrap();
    editor.load_plugin(&plugin_dir("helix")).unwrap();
    editor.resize(40, 6);
    type_keys(&mut editor, &format!(":o {}<ret>", b.display()));
    assert_eq!(text(&editor), "bbb\n");
    assert_eq!(primary(&editor), (0, 1));
    type_keys(&mut editor, "gp");
    assert_eq!(text(&editor), "aaa\n");
    type_keys(&mut editor, "gn");
    assert_eq!(text(&editor), "bbb\n");
    fs::remove_file(&a).unwrap();
    fs::remove_file(&b).unwrap();
}

#[test]
fn shows_key_hints_while_a_key_is_pending() {
    let mut editor = editor_with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n");
    editor.resize(40, 12);
    let shows = |editor: &Editor, text: &str| screen(editor).iter().any(|row| row.contains(text));
    type_keys(&mut editor, "g");
    assert!(shows(&editor, "Goto"));
    assert!(shows(&editor, "e  last line"));
    type_keys(&mut editor, "e");
    assert!(!shows(&editor, "Goto"));

    type_keys(&mut editor, "m");
    assert!(shows(&editor, "Match"));
    type_keys(&mut editor, "a");
    assert!(shows(&editor, "Select around"));
    assert!(!shows(&editor, "Match"));
    type_keys(&mut editor, "<esc>");
    assert!(!shows(&editor, "Select around"));

    // Keys that wait for any char show nothing.
    let before = screen(&editor);
    type_keys(&mut editor, "f");
    assert_eq!(screen(&editor), before);
}

/// An editor on `text` whose helix plugin has `settings` from helix.toml.
fn editor_with_settings(text: &str, settings: &str) -> Editor {
    let mut config = nib_core::Config::default();
    config.plugins.insert(
        "helix".into(),
        nib_core::Config::parse_plugin("helix", settings).unwrap(),
    );
    let mut editor = Editor::default();
    editor.apply_config(config);
    editor.load_plugin(&plugin_dir("helix")).unwrap();
    type_keys(&mut editor, &format!("i{text}<esc>gg"));
    editor.resize(40, 12);
    editor
}

#[test]
fn keys_can_be_remapped() {
    let settings = r#"
        [settings.keys.normal]
        q = "goto_last_line"
        "C-t" = "buffer.next"
        z = { z = "goto_file_start", n = "no_op" }

        [settings.keys.insert]
        j = { k = "normal_mode" }
    "#;
    let mut editor = editor_with_settings("one\ntwo\nthree", settings);
    // A Helix command name replays its keys.
    type_keys(&mut editor, "q");
    assert_eq!(primary(&editor).0, 8);
    // A table is a prefix, with hints.
    type_keys(&mut editor, "z");
    assert!(screen(&editor).iter().any(|row| row.contains("as g g")));
    type_keys(&mut editor, "z");
    assert_eq!(primary(&editor).0, 0);
    // A dotted name calls the command.
    editor.handle_key(KeyEvent::ctrl('t'));
    assert_eq!(editor.message(), None);

    // jk leaves insert mode; j and another key are text.
    type_keys(&mut editor, "ijk");
    assert!(status_line(&editor).starts_with(" NOR "));
    assert_eq!(text(&editor), "one\ntwo\nthree");
    type_keys(&mut editor, "ijx<esc>");
    assert_eq!(text(&editor), "jxone\ntwo\nthree");
}

#[test]
fn wrong_mappings_are_reported_and_left_out() {
    let settings = "[settings.keys.normal]\nq = \"no_such_command\"\nw = \"move_char_right\"";
    let mut editor = editor_with_settings("abc", settings);
    // The rest of the keymap still works, the good mapping too.
    type_keys(&mut editor, "w");
    assert_eq!(primary(&editor), (1, 2));
    let mut editor = Editor::default();
    let mut config = nib_core::Config::default();
    config.plugins.insert(
        "helix".into(),
        nib_core::Config::parse_plugin("helix", settings).unwrap(),
    );
    editor.apply_config(config);
    editor.load_plugin(&plugin_dir("helix")).unwrap();
    let message = editor.message().unwrap();
    assert!(
        message.contains("keys.normal.q: unknown command \"no_such_command\""),
        "{message}"
    );
}

#[test]
fn dot_repeats_the_last_insert() {
    let mut editor = editor_with_text("a\nb\n");
    type_keys(&mut editor, "Ax<esc>j.");
    assert_eq!(text(&editor), "ax\nbx\n");
    // With a count, and for an insert that opened a line.
    type_keys(&mut editor, "ohi<esc>2.");
    assert_eq!(text(&editor), "ax\nbx\nhi\nhi\nhi\n");
    // Undone one repeat at a time.
    type_keys(&mut editor, "u");
    assert_eq!(text(&editor), "ax\nbx\nhi\nhi\n");
}

#[test]
fn registers_keep_what_they_are_given() {
    let mut editor = editor_with_text("one\ntwo\n");
    type_keys(&mut editor, "x\"ay");
    // Deleted into `_`, the line is gone for good.
    type_keys(&mut editor, "jx\"_d");
    assert_eq!(text(&editor), "one\n");
    type_keys(&mut editor, "k\"ap");
    assert_eq!(text(&editor), "one\none\n");
    // The default register was never given anything.
    type_keys(&mut editor, "p");
    assert_eq!(editor.message(), Some("register \" is empty"));
    // A chosen register lasts for one command.
    type_keys(&mut editor, "\"ajp");
    assert_eq!(editor.message(), Some("register \" is empty"));
}

/// A clipboard the test can look into, as a frontend would give one.
#[derive(Clone, Default)]
struct SharedClipboard(std::sync::Arc<std::sync::Mutex<String>>);

impl nib_core::Clipboard for SharedClipboard {
    fn get(&mut self) -> Result<String, String> {
        Ok(self.0.lock().unwrap().clone())
    }

    fn set(&mut self, text: &str) -> Result<(), String> {
        *self.0.lock().unwrap() = text.to_string();
        Ok(())
    }
}

#[test]
fn the_clipboard_is_the_plus_register() {
    let mut editor = editor_with_text("one\ntwo\n");
    let clipboard = SharedClipboard::default();
    editor.set_clipboard(Box::new(clipboard.clone()));
    type_keys(&mut editor, "x y");
    assert_eq!(*clipboard.0.lock().unwrap(), "one\n");
    type_keys(&mut editor, "j p");
    assert_eq!(text(&editor), "one\ntwo\none\n");
    // Copied elsewhere, then pasted with "+P; other registers are apart.
    *clipboard.0.lock().unwrap() = "zero\n".into();
    type_keys(&mut editor, "gg\"+P");
    assert_eq!(text(&editor), "zero\none\ntwo\none\n");
    type_keys(&mut editor, "p");
    assert_eq!(editor.message(), Some("register \" is empty"));
}

fn window(editor: &mut Editor, key: &str) {
    editor.handle_key(KeyEvent::ctrl('w'));
    type_keys(editor, key);
}

#[test]
fn views_split_and_follow_each_other() {
    let mut editor = editor_with_text("one\ntwo\n");
    editor.resize(41, 10);
    window(&mut editor, "v");
    // Side by side, a line between.
    let rows = screen(&editor);
    let top: String = rows[0].chars().take(24).collect();
    assert_eq!(top, "one                 │one");
    // Typed in the new view, shown in both.
    type_keys(&mut editor, "jix<esc>");
    let rows = screen(&editor);
    assert!(
        rows[1].starts_with("xtwo") && rows[1].contains("│xtwo"),
        "{rows:#?}"
    );
    // The other view kept its place, moved by the edit only.
    window(&mut editor, "h");
    assert_eq!(primary(&editor), (0, 1));
    window(&mut editor, "l");
    assert_eq!(primary(&editor).0, 5, "after the x typed");

    window(&mut editor, "q");
    assert!(!screen(&editor)[0].contains('│'));
    window(&mut editor, "q");
    assert_eq!(editor.message(), Some("the last view cannot be closed"));
}

#[test]
fn views_one_above_another_show_their_names() {
    let mut editor = editor_with_text("one\ntwo\n");
    editor.resize(30, 10);
    type_keys(&mut editor, " ws");
    let rows = screen(&editor);
    assert_eq!(rows[0].trim_end(), "one");
    assert!(rows[4].starts_with("── [scratch] ─"), "{rows:#?}");
    assert_eq!(rows[5].trim_end(), "one");
    // Undo in one view moves the other's selection too.
    type_keys(&mut editor, "ggdk");
    window(&mut editor, "k");
    type_keys(&mut editor, "u");
    window(&mut editor, "o");
    assert_eq!(screen(&editor)[4].trim_end(), "");
}
