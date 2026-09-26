//! Programs started by plugins, with the test plugins in `plugins/test`.
//! Build them first with `cargo xtask build-plugins`. The programs used are
//! on Linux, macOS, and Windows alike.

use std::thread;
use std::time::{Duration, Instant};

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

/// Hands background work to the plugin until test-events writes down that
/// a program exited, and returns that line.
fn wait_for_exit(editor: &mut Editor) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        editor.run_background();
        let log = editor.call_command("test-events.log", "").unwrap();
        if let Some(line) = log.lines().find(|line| line.starts_with("exit ")) {
            return line.to_string();
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("no exit within 20 seconds");
}

/// A shell command line as `spawn` takes it: the program and its arguments,
/// one per line.
fn shell(script: &str) -> String {
    if cfg!(windows) {
        format!("cmd\n/C\n{script}")
    } else {
        format!("sh\n-c\n{script}")
    }
}

/// A program that prints its input back, as `spawn` takes it. Windows'
/// sort writes its output in another encoding, so it is not used.
fn echo_input() -> &'static str {
    if cfg!(windows) { "findstr\n^" } else { "cat" }
}

#[test]
fn programs_report_output_and_exit() {
    let mut editor = editor_with(&["test-events"]);
    let script = if cfg!(windows) {
        "echo out& echo err 1>&2& exit 3"
    } else {
        "echo out; echo err >&2; exit 3"
    };
    let id = editor
        .call_command("test-events.spawn", &shell(script))
        .unwrap();
    assert_eq!(
        wait_for_exit(&mut editor),
        format!("exit {id} Some(3) stdout=out stderr=err")
    );
}

#[test]
fn programs_read_what_plugins_write() {
    let mut editor = editor_with(&["test-events"]);
    let id = editor
        .call_command("test-events.spawn", echo_input())
        .unwrap();
    editor
        .call_command("test-events.write", &format!("{id} b\na\n"))
        .unwrap();
    editor.call_command("test-events.close", &id).unwrap();
    assert_eq!(
        wait_for_exit(&mut editor),
        format!("exit {id} Some(0) stdout=b|a stderr=")
    );
}

#[test]
fn killed_programs_still_report_their_exit() {
    let mut editor = editor_with(&["test-events"]);
    // Waits for input that never comes.
    let id = editor
        .call_command("test-events.spawn", echo_input())
        .unwrap();
    editor.call_command("test-events.kill", &id).unwrap();
    let exit = wait_for_exit(&mut editor);
    assert!(exit.starts_with(&format!("exit {id} ")), "{exit}");
}

#[test]
fn starting_programs_needs_the_capability() {
    let mut editor = editor_with(&["test-events", "test-misbehave"]);
    let err = editor.call_command("test-misbehave.spawn", "").unwrap_err();
    assert_eq!(
        err,
        "sort: starting programs needs the \"process\" capability"
    );
    let events = editor
        .plugins()
        .into_iter()
        .find(|p| p.name == "test-events")
        .unwrap();
    assert_eq!(events.capabilities, ["process"]);
}

#[test]
fn missing_programs_fail_to_start() {
    let mut editor = editor_with(&["test-events"]);
    let err = editor
        .call_command("test-events.spawn", "nib-no-such-program")
        .unwrap_err();
    assert!(err.starts_with("nib-no-such-program: "), "{err}");
}
