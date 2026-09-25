mod draw;
mod terminal;

mod builtin {
    include!(concat!(env!("OUT_DIR"), "/builtin_plugins.rs"));
}

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use nib_core::{Config, Editor, plugin_name};

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
    // A broken config should not keep the editor from starting: fall back to
    // the defaults and say why.
    let config_error = match load_config() {
        Ok(config) => {
            editor.apply_config(config);
            None
        }
        Err(err) => Some(err),
    };
    for path in &files {
        if let Err(err) = editor.open(path) {
            eprintln!("nib: {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
    }
    editor.set_plugin_cache_dir(cache_dir());
    let configured: Vec<PathBuf> = editor
        .settings()
        .plugins
        .iter()
        .map(|dir| expand_home(dir))
        .chain(plugins)
        .collect();
    if let Err(err) = load_plugins(&mut editor, &configured) {
        eprintln!("nib: {err}");
        return ExitCode::FAILURE;
    }
    if let Some(err) = config_error {
        editor.show_message(format!("{err}; using the defaults"));
    }
    // The keymap takes every key, so nothing else tells people the menu key.
    if editor.message().is_none() {
        editor.show_message(format!("{}: plugin menu", editor.settings().menu_key));
    }

    if let Err(err) = terminal::run(&mut editor) {
        eprintln!("nib: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Loads the built-in plugins first, so the keymap is at the bottom of the
/// input stack, then `dirs`. A plugin in `dirs` replaces a built-in one of
/// the same name, e.g. while working on it.
fn load_plugins(editor: &mut Editor, dirs: &[PathBuf]) -> Result<(), nib_core::Error> {
    let replaced = dirs
        .iter()
        .map(|dir| plugin_name(dir))
        .collect::<Result<Vec<_>, _>>()?;
    for (name, manifest, files) in builtin::PLUGINS {
        if !replaced.iter().any(|r| r == name) {
            editor.load_builtin_plugin(manifest, files)?;
        }
    }
    for dir in dirs {
        editor.load_plugin(dir)?;
    }
    if builtin::PLUGINS.is_empty() {
        editor.show_message("built without the standard plugins; run `cargo xtask build-plugins`");
    }
    Ok(())
}

fn load_config() -> Result<Config, String> {
    let Some(path) = config_dir().map(|dir| dir.join("config.toml")) else {
        return Ok(Config::default());
    };
    match fs::read_to_string(&path) {
        Ok(text) => Config::parse(&text).map_err(|err| err.to_string()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

fn config_dir() -> Option<PathBuf> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .or_else(|| env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("nib"))
}

/// Where compiled plugins are cached, so later starts skip compiling.
fn cache_dir() -> Option<PathBuf> {
    let base = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .or_else(|| env::var_os("LOCALAPPDATA").map(PathBuf::from))?;
    Some(base.join("nib"))
}

/// Expands a leading `~/`, as plugin paths in config.toml are often written.
fn expand_home(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.to_path_buf(),
    }
}
