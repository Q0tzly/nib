//! Where nib's settings live, and which plugins they load.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use nib_core::{Config, plugin_name};

use crate::builtin;
use crate::install::Store;

/// `~/.config/nib`, or under `$XDG_CONFIG_HOME` or `%APPDATA%`.
pub fn config_dir() -> Option<PathBuf> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .or_else(|| env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("nib"))
}

/// `~/.local/share/nib`, or under `$XDG_DATA_HOME` or `%LOCALAPPDATA%`:
/// installed plugins and what plugins keep.
pub fn data_dir() -> Option<PathBuf> {
    let base = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })
        .or_else(|| env::var_os("LOCALAPPDATA").map(PathBuf::from))?;
    Some(base.join("nib"))
}

/// Where installed plugins are kept.
pub fn store() -> Option<Store> {
    data_dir().map(|data| Store { data })
}

/// Where compiled plugins are cached, so later starts skip compiling.
pub fn cache_dir() -> Option<PathBuf> {
    let base = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .or_else(|| env::var_os("LOCALAPPDATA").map(PathBuf::from))?;
    Some(base.join("nib"))
}

/// Reads `config.toml` and `plugins/*.toml` in `dir`. Missing files mean
/// the defaults.
pub fn load(dir: &Path) -> Result<Config, String> {
    let mut config = match fs::read_to_string(dir.join("config.toml")) {
        Ok(text) => Config::parse(&text).map_err(|err| err.to_string())?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => Config::default(),
        Err(err) => return Err(format!("config.toml: {err}")),
    };
    for (name, path) in plugin_files(dir)? {
        let text = fs::read_to_string(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        let plugin = Config::parse_plugin(&name, &text).map_err(|err| err.to_string())?;
        config.plugins.insert(name, plugin);
    }
    Ok(config)
}

/// The `plugins/<name>.toml` files, sorted by name.
pub fn plugin_files(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let dir = dir.join("plugins");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("{}: {err}", dir.display())),
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "toml"))
        .filter_map(|path| Some((path.file_stem()?.to_str()?.to_string(), path)))
        .collect();
    files.sort();
    Ok(files)
}

pub enum Source {
    /// Index into `builtin::PLUGINS`.
    Builtin(usize),
    Dir(PathBuf),
}

/// A plugin nib knows about, from its build or the settings.
pub struct Entry {
    pub name: String,
    pub enabled: bool,
    pub source: Source,
    /// Its `plugins/<name>.toml`, if it has one.
    pub file: Option<PathBuf>,
    /// Where it was installed from, for installed ones.
    pub origin: Option<String>,
}

/// Every plugin, in load order: built-in ones first, so the keymap is at
/// the bottom of the input stack, then installed ones, then the ones with a
/// `path`. A `path` replaces the plugin of the same name.
pub fn entries(
    config: &Config,
    dir: Option<&Path>,
    store: Option<&Store>,
) -> Result<Vec<Entry>, String> {
    let file = |name: &str| {
        let path = dir?.join("plugins").join(format!("{name}.toml"));
        config.plugins.contains_key(name).then_some(path)
    };
    let mut entries = Vec::new();
    for (i, (name, _, _)) in builtin::PLUGINS.iter().enumerate() {
        let settings = config.plugin(name);
        if settings.path.is_none() {
            entries.push(Entry {
                name: name.to_string(),
                enabled: settings.enabled,
                source: Source::Builtin(i),
                file: file(name),
                origin: None,
            });
        }
    }
    if let Some(store) = store {
        for record in store.records()? {
            let settings = config.plugin(&record.name);
            if settings.path.is_none() {
                entries.push(Entry {
                    enabled: settings.enabled,
                    source: Source::Dir(store.dir(&record.name)),
                    file: file(&record.name),
                    origin: Some(record.source),
                    name: record.name,
                });
            }
        }
    }
    for (name, settings) in &config.plugins {
        if let Some(path) = &settings.path {
            entries.push(Entry {
                name: name.clone(),
                enabled: settings.enabled,
                source: Source::Dir(expand_home(path)),
                file: file(name),
                origin: None,
            });
        }
    }
    Ok(entries)
}

/// Checks that the plugin in `dir` is the one its settings file is named
/// after.
pub fn check_name(name: &str, dir: &Path) -> Result<(), String> {
    let found = plugin_name(dir).map_err(|err| err.to_string())?;
    if found != name {
        return Err(format!(
            "plugins/{name}.toml loads {}, a plugin named {found}",
            dir.display()
        ));
    }
    Ok(())
}

/// Expands a leading `~/`, as plugin paths are often written.
pub fn expand_home(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.to_path_buf(),
    }
}

pub const CONFIG_TEMPLATE: &str = r##"# nib's settings. Every line is optional; the values shown are the defaults.
# Each plugin has its own file in plugins/<name>.toml.

[core]
# tab-width = 4
# Spaces per indent, or "tab".
# indent = 4
# Lines kept visible above and below the cursor.
# scroll-margin = 5
# Opens the core menu to manage plugins; plugins never see this key.
# menu-key = "C-g"
# Limits for every plugin; plugins/<name>.toml can set its own.
# plugin-timeout-ms = 1000
# plugin-init-timeout-ms = 5000
# plugin-memory-mib = 256

# Styles by name: UI parts such as "ui.selection", and syntax such as
# "keyword" or "function.method", which falls back to "function". A color
# alone sets the text color; a table can set fg, bg, bold, italic, underline,
# and reverse. Colors: black, red, green, yellow, blue, magenta, cyan, white,
# bright-<color>, "#rrggbb", 0 to 255, or "default".
[theme]
# keyword = "magenta"
# comment = { fg = "bright-black", italic = true }
"##;

/// A `plugins/<name>.toml` with every key commented out.
pub fn plugin_template(name: &str) -> String {
    format!(
        r##"# How nib runs the {name} plugin, and the settings it gets. Every line is
# optional.

# false to not load it.
# enabled = true
# A directory to load it from, instead of the built-in {name}.
# path = "~/dev/{name}"
# Limits instead of the ones in config.toml's [core]. "none" for no time
# limit; Ctrl-g still stops it.
# timeout-ms = 1000
# init-timeout-ms = 5000
# memory-mib = 256
# "lazy" to start it when one of its commands is called or one of its
# events comes, rather than when nib starts.
# load = "start"

# Given to the plugin when it starts.
[settings]
"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_parse_to_the_defaults() {
        assert_eq!(Config::parse(CONFIG_TEMPLATE).unwrap(), Config::default());
        let plugin = Config::parse_plugin("helix", &plugin_template("helix")).unwrap();
        assert_eq!(plugin, nib_core::PluginConfig::default());
    }
}
