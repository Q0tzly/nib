//! Runs the test plugins in `plugins/test`. Build them first with
//! `cargo xtask build-plugins`.

use std::path::PathBuf;
use std::time::{Duration, Instant};
use std::{env, fs};

use nib_core::{Config, Editor, KeyCode, KeyEvent, Menu, PluginOptions, Range};

fn plugin_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../target/plugins")
        .join(name);
    assert!(
        dir.join("plugin.wasm").is_file(),
        "{} is missing; run `cargo xtask build-plugins` first",
        dir.display()
    );
    dir
}

fn editor_with(name: &str, options: PluginOptions) -> Editor {
    let mut editor = Editor::default();
    editor.set_plugin_options(options);
    editor.load_plugin(&plugin_dir(name)).unwrap();
    editor
}

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c))
}

#[test]
fn plugin_edits_the_buffer() {
    let mut editor = editor_with("test-insert", PluginOptions::default());
    for c in "hi あ".chars() {
        editor.handle_key(key(c));
    }
    assert_eq!(editor.buffer().text().to_string(), "hi あ");
    assert_eq!(editor.view().selection.primary(), Range::point(6));
    assert_eq!(editor.message(), None);

    // Ctrl-g opens the core menu instead of reaching the plugin, and the key
    // that closes the menu does not reach it either.
    editor.handle_key(KeyEvent::ctrl('g'));
    assert_eq!(editor.menu(), Some(Menu::Main));
    editor.handle_key(KeyEvent::new(KeyCode::Escape));
    assert_eq!(editor.menu(), None);
    editor.handle_key(key('!'));
    assert_eq!(editor.buffer().text().to_string(), "hi あ!");

    // Ctrl-g never reaches the plugin: pressed again, it closes the menu.
    editor.handle_key(KeyEvent::ctrl('g'));
    editor.handle_key(KeyEvent::ctrl('g'));
    assert_eq!(editor.menu(), None);
    assert_eq!(editor.buffer().text().to_string(), "hi あ!");

    // Other Ctrl keys do reach it.
    editor.handle_key(KeyEvent::ctrl('x'));
    assert_eq!(editor.buffer().text().to_string(), "hi あ!^X");

    // Escape pops the plugin's layer.
    assert_eq!(editor.key_hint(), None);
    editor.handle_key(KeyEvent::new(KeyCode::Escape));
    assert!(editor.key_hint().is_some());

    // Unsaved changes: quitting asks first.
    editor.handle_key(KeyEvent::ctrl('g'));
    editor.handle_key(key('q'));
    editor.handle_key(key('y'));
    assert!(editor.should_quit());
}

#[test]
fn failing_plugin_is_restarted_then_disabled() {
    let mut editor = editor_with(
        "test-misbehave",
        PluginOptions {
            call_timeout: Duration::from_millis(100),
            memory_limit: 64 << 20,
            ..PluginOptions::default()
        },
    );

    let started = Instant::now();
    editor.handle_key(key('l'));
    assert!(started.elapsed() < Duration::from_secs(1));
    let message = editor.message().unwrap();
    assert!(message.contains("restarted"), "{message}");
    assert!(message.contains("took too long"), "{message}");

    // Restarted: its layer is back.
    assert_eq!(editor.key_hint(), None);

    editor.handle_key(key('p'));
    let message = editor.message().unwrap();
    assert!(message.contains("panicked: asked to panic"), "{message}");

    // Allocating more than the memory limit is the third failure.
    editor.handle_key(key('m'));
    let message = editor.message().unwrap();
    assert!(message.contains("disabled"), "{message}");
    assert!(!editor.plugins()[0].enabled);

    assert!(editor.key_hint().is_some());

    // The core menu brings it back.
    editor.handle_key(KeyEvent::ctrl('g'));
    editor.handle_key(key('r'));
    assert_eq!(editor.message(), Some("plugins restarted"));
    assert!(editor.plugins()[0].enabled);
    assert_eq!(editor.key_hint(), None);
}

#[test]
fn init_error_is_reported() {
    let mut editor = Editor::default();
    editor.apply_config(Config::parse("[plugins.test-misbehave]\nfail = true").unwrap());
    let err = editor
        .load_plugin(&plugin_dir("test-misbehave"))
        .unwrap_err();
    assert!(err.to_string().contains("asked to fail"), "{err}");
    assert!(editor.plugins().is_empty());
    // Whatever the plugin did in `init` was undone.
    assert!(editor.key_hint().is_some());
}

#[test]
fn api_version_mismatch_is_rejected() {
    let dir = env::temp_dir().join(format!("nib-{}-old-plugin", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::copy(
        plugin_dir("test-insert").join("plugin.wasm"),
        dir.join("plugin.wasm"),
    )
    .unwrap();
    fs::write(
        dir.join("plugin.toml"),
        "name = \"old\"\nversion = \"0.0.0\"\napi = \"0.0\"\n",
    )
    .unwrap();

    let err = Editor::default().load_plugin(&dir).unwrap_err();
    fs::remove_dir_all(&dir).unwrap();
    assert!(err.to_string().contains("needs API 0.0"), "{err}");
}

/// Prints the cost of sending one key through a plugin. Run with
/// `cargo test --release -p nib-core --test plugins -- --ignored --nocapture`.
#[test]
#[ignore]
fn key_latency() {
    let mut editor = editor_with("test-insert", PluginOptions::default());
    let keys = 10_000;
    let started = Instant::now();
    for _ in 0..keys {
        editor.handle_key(key('a'));
    }
    let per_key = started.elapsed() / keys;
    println!("{per_key:?} per key (insert through test-insert)");
    assert_eq!(editor.plugins()[0].slow_calls, 0);
}
