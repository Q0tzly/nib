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
