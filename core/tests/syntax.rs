//! Highlights Rust with the grammar from `plugins/languages/rust`. Build it
//! first with `cargo xtask build-plugins`.

use std::{env, fs};

use nib_core::{Color, Editor, Grid, KeyEvent};

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
