mod commands;
mod draw;
mod settings;
mod terminal;

mod builtin {
    include!(concat!(env!("OUT_DIR"), "/builtin_plugins.rs"));
}

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use nib_core::{Config, Editor, plugin_name};

use settings::{Entry, Source};

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match args.first().and_then(|a| a.to_str()) {
        Some("config") => return commands::config(&args[1..]),
        Some("plugin") => return commands::plugin(&args[1..]),
        Some("--help" | "-h") => {
            println!("{}", commands::USAGE);
            return ExitCode::SUCCESS;
        }
        _ => {}
    }

    let mut files = Vec::new();
    let mut plugins = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--plugin" {
            let Some(dir) = args.next() else {
                eprintln!("nib: --plugin needs a directory\n{}", commands::USAGE);
                return ExitCode::FAILURE;
            };
            plugins.push(PathBuf::from(dir));
        } else {
            files.push(PathBuf::from(arg));
        }
    }

    let mut editor = Editor::default();
    // Broken settings should not keep the editor from starting: fall back to
    // the defaults and say why.
    let dir = settings::config_dir();
    let (config, config_error) = match dir.as_deref().map(settings::load) {
        Some(Ok(config)) => (config, None),
        Some(Err(err)) => (Config::default(), Some(err)),
        None => (Config::default(), None),
    };
    let entries = settings::entries(&config, dir.as_deref());
    editor.apply_config(config);
    for path in &files {
        if let Err(err) = editor.open(path) {
            eprintln!("nib: {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
    }
    editor.set_plugin_cache_dir(settings::cache_dir());
    if let Err(err) = load_plugins(&mut editor, entries, &plugins) {
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

/// Loads the enabled plugins in order, then the `--plugin` ones, which
/// replace a plugin of the same name, e.g. while working on it.
fn load_plugins(editor: &mut Editor, entries: Vec<Entry>, extra: &[PathBuf]) -> Result<(), String> {
    let replaced = extra
        .iter()
        .map(|dir| plugin_name(dir))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())?;
    for entry in entries {
        if !entry.enabled || replaced.contains(&entry.name) {
            continue;
        }
        match entry.source {
            Source::Builtin(i) => {
                let (_, manifest, files) = builtin::PLUGINS[i];
                editor.load_builtin_plugin(manifest, files)
            }
            Source::Dir(dir) => {
                settings::check_name(&entry.name, &dir)?;
                editor.load_plugin(&dir)
            }
        }
        .map_err(|err| err.to_string())?;
    }
    for dir in extra {
        editor.load_plugin(dir).map_err(|err| err.to_string())?;
    }
    if builtin::PLUGINS.is_empty() {
        editor.show_message("built without the standard plugins; run `cargo xtask build-plugins`");
    }
    Ok(())
}
