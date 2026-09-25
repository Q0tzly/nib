mod draw;
mod terminal;

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use nib_core::{Editor, PluginOptions};

const USAGE: &str = "usage: nib [--plugin DIR]... [FILE]...";

fn main() -> ExitCode {
    let mut files = Vec::new();
    let mut plugins = Vec::new();
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--plugin" {
            let Some(dir) = args.next() else {
                eprintln!("nib: --plugin needs a directory\n{USAGE}");
                return ExitCode::FAILURE;
            };
            plugins.push(PathBuf::from(dir));
        } else {
            files.push(PathBuf::from(arg));
        }
    }

    let mut editor = Editor::default();
    for path in &files {
        if let Err(err) = editor.open(path) {
            eprintln!("nib: {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
    }
    editor.set_plugin_options(PluginOptions {
        cache_dir: cache_dir(),
        ..PluginOptions::default()
    });
    for dir in &plugins {
        if let Err(err) = editor.load_plugin(dir, "{}") {
            eprintln!("nib: {err}");
            return ExitCode::FAILURE;
        }
    }

    if let Err(err) = terminal::run(&mut editor) {
        eprintln!("nib: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Where compiled plugins are cached, so later starts skip compiling.
fn cache_dir() -> Option<PathBuf> {
    let base = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .or_else(|| env::var_os("LOCALAPPDATA").map(PathBuf::from))?;
    Some(base.join("nib"))
}
