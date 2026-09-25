//! Highlights Rust with the grammar from `plugins/languages/rust`. Build it
//! first with `cargo xtask build-plugins`.

use std::{env, fs};

use nib_core::{Color, Editor, Grid, KeyCode, KeyEvent, Modifiers, Range, Selection};

mod common;
use common::{plugin_dir, screen, type_keys};

fn fg(editor: &Editor, x: u16, y: u16) -> Color {
    let mut grid = Grid::default();
    editor.render(&mut grid);
    grid.cell(x, y).style.fg
}

const KEYWORD: Color = Color::Indexed(5);
const FUNCTION: Color = Color::Indexed(4);
const COMMENT: Color = Color::Indexed(8);

#[test]
fn highlights_and_follows_edits() {
    let path = env::temp_dir().join(format!("nib-{}-syntax.rs", std::process::id()));
    fs::write(&path, "fn main() {}\nfn other() {}\n").unwrap();
    let mut editor = Editor::default();
    editor.open(&path).unwrap();
    editor.load_plugin(&plugin_dir("helix")).unwrap();
    // The language comes after the file is open, as when nib starts.
    editor.load_plugin(&plugin_dir("rust")).unwrap();
    editor.resize(40, 6);

    // The first parse waits until after the first frame.
    assert_eq!(fg(&editor, 0, 0), Color::Reset);
    assert!(editor.catch_up());
    assert!(!editor.catch_up());
    assert_eq!(fg(&editor, 0, 0), KEYWORD);
    assert_eq!(fg(&editor, 3, 0), FUNCTION);
    assert_eq!(fg(&editor, 3, 1), FUNCTION);

    // Reparsed from the edit: the new comment and the lines after it.
    type_keys(&mut editor, "i// x<ret><esc>");
    assert_eq!(fg(&editor, 0, 0), COMMENT);
    assert_eq!(fg(&editor, 0, 1), KEYWORD);
    assert_eq!(fg(&editor, 3, 2), FUNCTION);

    // Undo drops the tree; it is parsed again from scratch.
    type_keys(&mut editor, "u");
    assert_eq!(fg(&editor, 0, 0), KEYWORD);

    editor.resize(80, 6);
    editor.handle_key(KeyEvent::ctrl('g'));
    let rows = screen(&editor);
    assert!(
        rows.iter()
            .any(|row| row.contains("rust") && row.contains("languages")),
        "{rows:#?}"
    );
    fs::remove_file(&path).unwrap();
}

/// Prints where startup time goes, without and with the compile cache. Run
/// with `cargo test --release -p nib-core --test syntax -- --ignored --nocapture`.
#[test]
#[ignore]
fn startup_breakdown() {
    use std::time::Instant;
    let cache = env::temp_dir().join(format!("nib-{}-cache", std::process::id()));
    let file = env!("CARGO_MANIFEST_DIR").to_string() + "/src/render.rs";
    for round in ["cold cache", "warm cache"] {
        let started = Instant::now();
        let mut editor = Editor::default();
        editor.set_plugin_cache_dir(Some(cache.clone()));
        editor.resize(120, 40);
        let mut last = Instant::now();
        let mut step = |name: &str| {
            println!("{round}: {name}: {:?}", last.elapsed());
            last = Instant::now();
        };
        editor.load_plugin(&plugin_dir("helix")).unwrap();
        step("load helix");
        editor.load_plugin(&plugin_dir("rust")).unwrap();
        step("load rust (grammar, query)");
        editor.open(&file).unwrap();
        step("open");
        let mut grid = Grid::default();
        editor.render(&mut grid);
        step("first frame, without highlighting");
        editor.catch_up();
        editor.render(&mut grid);
        step("load grammar, parse, and draw highlighted");
        println!("{round}: total {:?}", started.elapsed());
    }
    let _ = fs::remove_dir_all(&cache);
}

const SOURCE: &str = "\
// one
// two
fn add(a: u8, b: u8) -> u8 {
    let s = \"}\";
    a + b
}

fn other() {}
";

/// An editor on a Rust file with the keymap and the language, parsed.
fn rust_editor(name: &str) -> (Editor, std::path::PathBuf) {
    rust_editor_with(name, SOURCE)
}

fn rust_editor_with(name: &str, source: &str) -> (Editor, std::path::PathBuf) {
    let path = env::temp_dir().join(format!("nib-{}-{name}.rs", std::process::id()));
    fs::write(&path, source).unwrap();
    let mut editor = Editor::default();
    editor.open(&path).unwrap();
    editor.load_plugin(&plugin_dir("helix")).unwrap();
    editor.load_plugin(&plugin_dir("rust")).unwrap();
    editor.resize(40, 12);
    editor.catch_up();
    (editor, path)
}

/// Puts a block cursor on the first `at` in the text.
fn put_cursor(editor: &mut Editor, at: &str) {
    let pos = SOURCE.find(at).unwrap();
    editor.view_mut().selection =
        Selection::new(vec![Range::new(pos, pos + 1)], 0, editor.buffer().text()).unwrap();
}

fn selected(editor: &Editor) -> String {
    let range = editor.view().selection.primary();
    editor
        .buffer()
        .text()
        .slice(range.from()..range.to())
        .to_string()
}

fn alt(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: Modifiers {
            alt: true,
            ..Modifiers::default()
        },
    }
}

#[test]
fn selects_text_objects() {
    let (mut editor, path) = rust_editor("objects");
    put_cursor(&mut editor, "a + b");
    type_keys(&mut editor, "maf");
    assert!(selected(&editor).starts_with("fn add(") && selected(&editor).ends_with("b\n}"));
    put_cursor(&mut editor, "a + b");
    type_keys(&mut editor, "mif");
    assert!(selected(&editor).starts_with("{\n    let"));

    put_cursor(&mut editor, "a: u8");
    type_keys(&mut editor, "maa");
    assert_eq!(selected(&editor), "a: u8,");
    put_cursor(&mut editor, "b: u8");
    type_keys(&mut editor, "mia");
    assert_eq!(selected(&editor), "b: u8");

    // Consecutive line comments are one comment.
    put_cursor(&mut editor, "one");
    type_keys(&mut editor, "mac");
    assert_eq!(selected(&editor), "// one\n// two");

    // Not in a function: nothing changes.
    put_cursor(&mut editor, "one");
    type_keys(&mut editor, "maf");
    assert_eq!(selected(&editor), "o");
    fs::remove_file(&path).unwrap();
}

#[test]
fn jumps_between_text_objects() {
    let (mut editor, path) = rust_editor("jumps");
    put_cursor(&mut editor, "one");
    type_keys(&mut editor, "]f");
    assert!(selected(&editor).starts_with("fn add("));
    type_keys(&mut editor, "]f");
    assert_eq!(selected(&editor), "fn other() {}");
    type_keys(&mut editor, "[f");
    assert!(selected(&editor).starts_with("fn add("));
    // Backward jumps put the cursor at the start.
    let range = editor.view().selection.primary();
    assert!(range.head < range.anchor);

    put_cursor(&mut editor, "one");
    type_keys(&mut editor, "2]a");
    assert_eq!(selected(&editor), "b: u8");
    fs::remove_file(&path).unwrap();
}

#[test]
fn jumps_across_long_gaps() {
    // Farther apart than the first window the keymap searches.
    let gap = "// padding\n".repeat(2000);
    let source = format!("fn first() {{}}\n{gap}fn second() {{}}\n{gap}");
    let (mut editor, path) = rust_editor_with("gaps", &source);
    type_keys(&mut editor, "]f");
    assert_eq!(selected(&editor), "fn second() {}");
    type_keys(&mut editor, "]f");
    assert_eq!(selected(&editor), "fn second() {}");
    type_keys(&mut editor, "[f");
    assert_eq!(selected(&editor), "fn first() {}");
    fs::remove_file(&path).unwrap();
}

#[test]
fn expands_shrinks_and_walks_nodes() {
    let (mut editor, path) = rust_editor("nodes");
    // Without an Alt-o to undo, Alt-i goes to the first named child.
    let start = SOURCE.find("(a: u8").unwrap();
    let end = SOURCE.find(" -> u8").unwrap();
    editor.view_mut().selection =
        Selection::new(vec![Range::new(start, end)], 0, editor.buffer().text()).unwrap();
    editor.handle_key(alt(KeyCode::Char('i')));
    assert_eq!(selected(&editor), "a: u8");

    put_cursor(&mut editor, "a + b");
    editor.handle_key(alt(KeyCode::Char('o')));
    assert_eq!(selected(&editor), "a + b");
    editor.handle_key(alt(KeyCode::Up));
    assert!(selected(&editor).starts_with("{\n    let"));
    editor.handle_key(alt(KeyCode::Char('i')));
    assert_eq!(selected(&editor), "a + b");
    editor.handle_key(alt(KeyCode::Down));
    assert_eq!(selected(&editor), "a");

    put_cursor(&mut editor, "(a: u8");
    editor.handle_key(alt(KeyCode::Char('o')));
    assert_eq!(selected(&editor), "(a: u8, b: u8)");

    put_cursor(&mut editor, "a: u8");
    editor.handle_key(alt(KeyCode::Char('o')));
    assert_eq!(selected(&editor), "a: u8");
    editor.handle_key(alt(KeyCode::Char('n')));
    assert_eq!(selected(&editor), "b: u8");
    editor.handle_key(alt(KeyCode::Char('p')));
    assert_eq!(selected(&editor), "a: u8");
    fs::remove_file(&path).unwrap();
}

#[test]
fn matches_pairs_with_the_tree() {
    let (mut editor, path) = rust_editor("pairs");
    // The text alone would stop at the brace in the string.
    put_cursor(&mut editor, "{\n    let");
    type_keys(&mut editor, "mm");
    let close = SOURCE.find("}\n\nfn other").unwrap();
    assert_eq!(editor.view().cursor(editor.buffer().text()), close);
    type_keys(&mut editor, "mm");
    assert_eq!(
        editor.view().cursor(editor.buffer().text()),
        SOURCE.find("{\n    let").unwrap()
    );

    put_cursor(&mut editor, "a + b");
    type_keys(&mut editor, "mi{");
    assert!(selected(&editor).starts_with("\n    let s = \"}\";"));
    put_cursor(&mut editor, "}\";");
    type_keys(&mut editor, "ma\"");
    assert_eq!(selected(&editor), "\"}\"");
    fs::remove_file(&path).unwrap();
}
