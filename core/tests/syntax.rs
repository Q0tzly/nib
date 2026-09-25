//! Syntax trees with the grammars from `plugins/languages`. Build them
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

#[test]
fn highlights_the_matching_bracket() {
    let (mut editor, path) = rust_editor("highlight");
    let underlined = |editor: &Editor, x, y| {
        let mut grid = Grid::default();
        editor.render(&mut grid);
        grid.cell(x, y).style.underline
    };
    put_cursor(&mut editor, "{\n    let");
    // Updated after each key the keymap handles.
    type_keys(&mut editor, ";");
    assert!(underlined(&editor, 0, 5), "the closing brace");
    assert!(!underlined(&editor, 13, 3), "the brace in the string");
    type_keys(&mut editor, "j");
    assert!(!underlined(&editor, 0, 5));
    fs::remove_file(&path).unwrap();
}

/// Every standard language: its grammar loads, its queries compile, and a
/// sample gets some color.
#[test]
fn every_language_loads_its_queries() {
    let samples = [
        ("rust", "rs", "fn main() {} // c\n"),
        ("python", "py", "def f(a, b):\n    return 'x'  # c\n"),
        ("go", "go", "package main\n\nfunc f(a int) {} // c\n"),
        ("bash", "sh", "f() { echo \"x\"; } # c\n"),
        ("json", "json", "{\"a\": [1, true]}\n"),
        ("toml", "toml", "[a]\nb = \"x\" # c\n"),
        ("yaml", "yaml", "a: [1, \"x\"] # c\n"),
        ("markdown", "md", "# Title\n\ntext\n"),
    ];
    for (name, extension, sample) in samples {
        let path = env::temp_dir().join(format!("nib-{}-sample.{extension}", std::process::id()));
        fs::write(&path, sample).unwrap();
        let mut editor = Editor::default();
        editor.open(&path).unwrap();
        editor.load_plugin(&plugin_dir("helix")).unwrap();
        editor.load_plugin(&plugin_dir(name)).unwrap();
        editor.resize(40, 6);
        editor.catch_up();
        assert_eq!(editor.message(), None, "{name}");
        let mut grid = Grid::default();
        editor.render(&mut grid);
        let colored = (0..grid.width()).any(|x| grid.cell(x, 0).style.fg != Color::Reset);
        assert!(colored, "{name}: {:?}", screen(&editor)[0]);
        // Compiles the text objects query.
        type_keys(&mut editor, "maf");
        assert_eq!(editor.message(), None, "{name}");
        fs::remove_file(&path).unwrap();
    }
}

#[test]
fn text_objects_in_other_languages() {
    let cases = [
        // (language, extension, text, keys, selected)
        (
            "python",
            "py",
            "def f(a, b):\n    return a\n",
            "jmaf",
            "def f(a, b):\n    return a",
        ),
        ("python", "py", "def f(a, b):\n    return a\n", "fbmia", "b"),
        (
            "toml",
            "toml",
            "[a]\nx = 1\n\n[b]\ny = 2\n",
            "]t]t",
            "[b]\ny = 2\n",
        ),
        (
            "markdown",
            "md",
            "# One\n\ntext\n\n# Two\n\nmore\n",
            "]t]t",
            "# Two\n\nmore\n",
        ),
        (
            "go",
            "go",
            "package p\n\nfunc TestX(t int) {}\n",
            "]T",
            "func TestX(t int) {}",
        ),
    ];
    for (name, extension, text, keys, expected) in cases {
        let path = env::temp_dir().join(format!("nib-{}-objects.{extension}", std::process::id()));
        fs::write(&path, text).unwrap();
        let mut editor = Editor::default();
        editor.open(&path).unwrap();
        editor.load_plugin(&plugin_dir("helix")).unwrap();
        editor.load_plugin(&plugin_dir(name)).unwrap();
        editor.resize(40, 12);
        editor.catch_up();
        type_keys(&mut editor, keys);
        assert_eq!(selected(&editor), expected, "{name}: {keys}");
        fs::remove_file(&path).unwrap();
    }
}
