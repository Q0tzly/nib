//! The LSP plugin in `plugins/lsp`, with the Helix keymap, against a fake
//! language server: this test binary, started with `--fake-lsp`. Build the
//! plugins first with `cargo xtask build-plugins`.
//!
//! The fake server reports "found error" at every "error" in a file, says
//! where the cursor is when hovered, and puts every definition at line 0,
//! character 3 of the same file.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use std::{env, fs, panic, process, thread};

use nib_core::{Config, Editor};
use serde_json::{Value, json};

mod common;
use common::{plugin_dir, screen, type_keys};

fn main() {
    if env::args().any(|arg| arg == "--fake-lsp") {
        fake_server();
        return;
    }
    // A server that is not really there, as rustup's proxy when the
    // component is missing.
    if env::args().any(|arg| arg == "--fake-lsp-gone") {
        eprintln!("\nerror: not installed\nmore detail");
        process::exit(1);
    }
    let tests: [(&str, fn()); 5] = [
        ("diagnostics_follow_edits", diagnostics_follow_edits),
        (
            "hover_shows_until_the_next_key",
            hover_shows_until_the_next_key,
        ),
        ("definition_moves_the_cursor", definition_moves_the_cursor),
        ("missing_servers_are_reported", missing_servers_are_reported),
        ("servers_that_stop_say_why", servers_that_stop_say_why),
    ];
    let mut failed = 0;
    for (name, test) in tests {
        let passed = panic::catch_unwind(test).is_ok();
        println!("test {name} ... {}", if passed { "ok" } else { "FAILED" });
        failed += usize::from(!passed);
    }
    println!(
        "\ntest result: {}. {} passed; {failed} failed",
        if failed == 0 { "ok" } else { "FAILED" },
        tests.len() - failed
    );
    if failed > 0 {
        process::exit(1);
    }
}

/// An editor with the keymap, Rust, and the LSP plugin talking to `server`,
/// on a file with `text`.
fn editor(name: &str, text: &str, server: &[String]) -> (Editor, PathBuf) {
    let dir = env::temp_dir().join(format!("nib-lsp-{}-{name}", process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.rs");
    fs::write(&path, text).unwrap();
    let mut editor = Editor::default();
    editor.open(&path).unwrap();
    let command = serde_json::to_string(server).unwrap();
    let settings = format!("[settings.servers.rust]\ncommand = {command}\n");
    let mut config = Config::default();
    config.plugins.insert(
        "lsp".into(),
        Config::parse_plugin("lsp", &settings).unwrap(),
    );
    editor.apply_config(config);
    for plugin in ["helix", "rust", "lsp"] {
        editor.load_plugin(&plugin_dir(plugin)).unwrap();
    }
    editor.resize(60, 10);
    editor.catch_up();
    (editor, dir)
}

fn fake(name: &str, text: &str) -> (Editor, PathBuf) {
    let exe = env::current_exe().unwrap().to_string_lossy().into_owned();
    editor(name, text, &[exe, "--fake-lsp".into()])
}

/// Hands the servers' output to the plugin until `done` holds.
fn wait_until(editor: &mut Editor, what: &str, mut done: impl FnMut(&mut Editor) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(editor) {
        assert!(Instant::now() < deadline, "waited 20 seconds for {what}");
        editor.run_background();
        thread::sleep(Duration::from_millis(10));
    }
}

fn shows(editor: &Editor, text: &str) -> bool {
    screen(editor).iter().any(|row| row.contains(text))
}

fn cursor(editor: &Editor) -> usize {
    editor.view().cursor(editor.buffer().text())
}

fn diagnostics_follow_edits() {
    let (mut editor, dir) = fake("diagnostics", "fn main() {}\nlet error = 1;\n");
    wait_until(&mut editor, "the diagnostic", |e| shows(e, "found error"));
    let rows = screen(&editor);
    assert!(
        rows[1].starts_with("let error = 1;  found error"),
        "{rows:#?}"
    );
    assert!(rows.last().unwrap().contains("E1"), "{rows:#?}");
    // The server gets the deletion as an edit and finds nothing left.
    type_keys(&mut editor, "jxd");
    wait_until(&mut editor, "the diagnostic to go", |e| {
        !shows(e, "found error") && !screen(e).last().unwrap().contains("E1")
    });
    fs::remove_dir_all(dir).unwrap();
}

fn hover_shows_until_the_next_key() {
    let (mut editor, dir) = fake("hover", "fn main() {}\n");
    type_keys(&mut editor, "l");
    // The server needs to have the file first.
    wait_until(&mut editor, "the server", |e| {
        e.call_command("lsp.hover", "").is_ok()
    });
    wait_until(&mut editor, "the hover", |e| shows(e, "hover at 0:1"));
    // The key closes it and still moves the cursor.
    type_keys(&mut editor, "l");
    assert!(!shows(&editor, "hover at"));
    assert_eq!(cursor(&editor), 2);
    fs::remove_dir_all(dir).unwrap();
}

fn definition_moves_the_cursor() {
    let (mut editor, dir) = fake("definition", "fn main() {}\nmain();\n");
    type_keys(&mut editor, "j");
    // Fails until the server has the file.
    wait_until(&mut editor, "the server", |e| {
        e.call_command("lsp.definition", "").is_ok()
    });
    wait_until(&mut editor, "the definition", |e| cursor(e) == 3);
    // Now from the keymap.
    type_keys(&mut editor, "jgd");
    wait_until(&mut editor, "the definition from gd", |e| cursor(e) == 3);
    fs::remove_dir_all(dir).unwrap();
}

fn missing_servers_are_reported() {
    let (editor, dir) = editor("missing", "fn main() {}\n", &["nib-no-such-server".into()]);
    let message = editor.message().unwrap_or_default().to_string();
    assert!(
        message.starts_with("lsp: nib-no-such-server: "),
        "{message}"
    );
    fs::remove_dir_all(dir).unwrap();
}

fn servers_that_stop_say_why() {
    let exe = env::current_exe().unwrap().to_string_lossy().into_owned();
    let server = [exe.clone(), "--fake-lsp-gone".into()];
    let (mut editor, dir) = editor("gone", "fn main() {}\n", &server);
    let expected = format!("lsp: {exe} --fake-lsp-gone stopped: error: not installed");
    wait_until(&mut editor, "the server to stop", |e| {
        e.message() == Some(expected.as_str())
    });
    fs::remove_dir_all(dir).unwrap();
}

/// A language server that knows just enough for the tests.
fn fake_server() {
    let mut input = BufReader::new(io::stdin().lock());
    let mut documents: HashMap<String, String> = HashMap::new();
    while let Some(message) = read_message(&mut input) {
        let params = &message["params"];
        let uri = params["textDocument"]["uri"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        match message["method"].as_str().unwrap_or_default() {
            "initialize" => reply(
                &message,
                json!({"capabilities": {
                    "positionEncoding": "utf-8",
                    "textDocumentSync": 2,
                    "hoverProvider": true,
                    "definitionProvider": true,
                }}),
            ),
            "textDocument/didOpen" => {
                let text = params["textDocument"]["text"].as_str().unwrap_or_default();
                documents.insert(uri.clone(), text.to_string());
                publish(&uri, &documents[&uri]);
            }
            "textDocument/didChange" => {
                let text = documents.entry(uri.clone()).or_default();
                for change in params["contentChanges"].as_array().unwrap() {
                    let new = change["text"].as_str().unwrap();
                    match change.get("range") {
                        Some(range) => {
                            let start = offset(text, &range["start"]);
                            let end = offset(text, &range["end"]);
                            text.replace_range(start..end, new);
                        }
                        None => *text = new.to_string(),
                    }
                }
                publish(&uri, &documents[&uri]);
            }
            "textDocument/hover" => {
                let at = &params["position"];
                let value = format!("hover at {}:{}", at["line"], at["character"]);
                reply(
                    &message,
                    json!({"contents": {"kind": "plaintext", "value": value}}),
                );
            }
            "textDocument/definition" => reply(
                &message,
                json!({"uri": uri, "range": {
                    "start": {"line": 0, "character": 3},
                    "end": {"line": 0, "character": 7},
                }}),
            ),
            "shutdown" => reply(&message, Value::Null),
            "exit" => return,
            _ => {}
        }
    }
}

fn read_message(input: &mut impl BufRead) -> Option<Value> {
    let mut length = 0;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length: ") {
            length = value.parse().ok()?;
        }
    }
    let mut body = vec![0; length];
    input.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn send(message: &Value) {
    let body = message.to_string();
    let mut out = io::stdout().lock();
    write!(out, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    out.flush().unwrap();
}

fn reply(request: &Value, result: Value) {
    send(&json!({"jsonrpc": "2.0", "id": request["id"], "result": result}));
}

fn publish(uri: &str, text: &str) {
    let diagnostics: Vec<Value> = text
        .lines()
        .enumerate()
        .filter_map(|(line, content)| {
            let character = content.find("error")?;
            Some(json!({
                "range": {
                    "start": {"line": line, "character": character},
                    "end": {"line": line, "character": character + 5},
                },
                "severity": 1,
                "message": "found error",
            }))
        })
        .collect();
    send(&json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {"uri": uri, "diagnostics": diagnostics},
    }));
}

/// The byte offset of an LSP position counted in bytes.
fn offset(text: &str, position: &Value) -> usize {
    let line = position["line"].as_u64().unwrap() as usize;
    let character = position["character"].as_u64().unwrap() as usize;
    let line_start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
    line_start + character
}
