//! The Go SDK, through the test plugin in `plugins/test/go`, which
//! `cargo xtask build-plugins` builds when TinyGo is installed. Without it
//! the tests say so and pass, unless NIB_REQUIRE_GO is set, as in CI.

use std::env;
use std::path::PathBuf;

use nib_core::Editor;

mod common;
use common::{key, plugin_dir, screen};

/// The built Go plugin, if TinyGo built it.
fn go_plugin() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/plugins/test-go");
    if dir.join("plugin.wasm").is_file() {
        return Some(dir);
    }
    assert!(
        env::var_os("NIB_REQUIRE_GO").is_none(),
        "the Go test plugin is not built; install TinyGo and run cargo xtask build-plugins"
    );
    eprintln!("skipped: the Go test plugin is not built (TinyGo is not installed)");
    None
}

#[test]
fn go_plugins_take_keys_run_commands_and_get_events() {
    let Some(dir) = go_plugin() else {
        return;
    };
    let mut editor = Editor::default();
    editor.load_plugin(&dir).unwrap();
    editor.load_plugin(&plugin_dir("test-events")).unwrap();
    editor.resize(40, 4);
    for c in "hi".chars() {
        editor.handle_key(key(c));
    }
    assert_eq!(editor.buffer().text().to_string(), "hi");
    assert_eq!(editor.call_command("test-go.echo", "x"), Ok("x".into()));

    editor.call_command("test-events.emit", "ping").unwrap();
    let status = screen(&editor).last().unwrap().clone();
    assert!(status.contains("pinged"), "{status}");
}
