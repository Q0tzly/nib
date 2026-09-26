//! File lists made by the core for plugins, with the test plugins in
//! `plugins/test`. Build them first with `cargo xtask build-plugins`.

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};
use std::{env, fs};

use nib_core::Editor;

mod common;
use common::plugin_dir;

fn editor_with(plugins: &[&str]) -> Editor {
    let mut editor = Editor::default();
    for name in plugins {
        editor.load_plugin(&plugin_dir(name)).unwrap();
    }
    editor
}

/// A directory with files, some of them ignored or hidden.
fn tree(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("nib-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("target")).unwrap();
    fs::write(dir.join(".gitignore"), "target/\n").unwrap();
    fs::write(dir.join("src/a.rs"), "").unwrap();
    fs::write(dir.join("b.txt"), "").unwrap();
    fs::write(dir.join("target/out"), "").unwrap();
    fs::write(dir.join(".hidden"), "").unwrap();
    dir
}

/// What test-events wrote down, gathered until `done` holds or a while
/// passes.
fn log_until(editor: &mut Editor, done: impl Fn(&[String]) -> bool, wait: Duration) -> Vec<String> {
    let deadline = Instant::now() + wait;
    let mut log = Vec::new();
    while Instant::now() < deadline && !done(&log) {
        editor.run_background();
        let entries = editor.call_command("test-events.log", "").unwrap();
        log.extend(entries.lines().map(String::from));
        thread::sleep(Duration::from_millis(10));
    }
    log
}

#[test]
fn files_are_listed_honoring_gitignore() {
    let dir = tree("walk");
    let mut editor = editor_with(&["test-events"]);
    let job = editor
        .call_command("test-events.walk", &dir.to_string_lossy())
        .unwrap();
    let log = log_until(
        &mut editor,
        |log| log.iter().any(|l| l.ends_with("done=true")),
        Duration::from_secs(20),
    );
    assert_eq!(log, [format!("files {job} b.txt,src/a.rs done=true")]);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cancelled_lists_send_nothing() {
    let dir = tree("cancel");
    let mut editor = editor_with(&["test-events"]);
    let job = editor
        .call_command("test-events.walk", &dir.to_string_lossy())
        .unwrap();
    editor.call_command("test-events.stop-walk", &job).unwrap();
    let log = log_until(&mut editor, |_| false, Duration::from_millis(300));
    assert!(log.is_empty(), "{log:?}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn listing_files_needs_the_capability() {
    let mut editor = editor_with(&["test-events", "test-misbehave"]);
    let err = editor.call_command("test-misbehave.walk", "").unwrap_err();
    assert_eq!(err, "listing files needs the \"fs-read\" capability");
    let err = editor
        .call_command("test-events.walk", "nib-no-such-dir")
        .unwrap_err();
    assert!(err.contains("not a directory"), "{err}");
}
