//! `config.toml`: `[core]` for the core, `[plugins.<name>]` for each plugin.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::Error;
use crate::input::KeyEvent;

/// Settings of the core.
///
/// Editing behavior (`tab_width`, `indent`, `scroll_margin`) is for plugins
/// to read and, later, to override per buffer. Safety settings (`menu_key`,
/// `plugins`, and the plugin limits) are for the user alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub tab_width: u16,
    pub indent: Indent,
    /// Lines kept visible above and below the cursor.
    pub scroll_margin: u16,
    pub menu_key: KeyEvent,
    /// Plugin directories to load besides the built-in plugins.
    pub plugins: Vec<PathBuf>,
    /// A plugin call taking longer is stopped.
    pub plugin_timeout: Duration,
    pub plugin_init_timeout: Duration,
    /// Maximum size of a plugin's memory, in bytes.
    pub plugin_memory: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indent {
    Tab,
    Spaces(u8),
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tab_width: 4,
            indent: Indent::Spaces(4),
            scroll_margin: 5,
            menu_key: KeyEvent::ctrl('g'),
            plugins: Vec::new(),
            plugin_timeout: Duration::from_secs(1),
            plugin_init_timeout: Duration::from_secs(5),
            plugin_memory: 256 << 20,
        }
    }
}

impl Settings {
    /// The value of an editing setting as JSON, for plugins.
    pub fn get_json(&self, key: &str) -> Option<String> {
        let value = match key {
            "tab-width" => serde_json::json!(self.tab_width),
            "indent" => match self.indent {
                Indent::Tab => serde_json::json!("tab"),
                Indent::Spaces(n) => serde_json::json!(n),
            },
            "scroll-margin" => serde_json::json!(self.scroll_margin),
            _ => return None,
        };
        Some(value.to_string())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub core: Settings,
    /// Each plugin's table as JSON, passed to its `init` as is.
    pub plugins: BTreeMap<String, String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    core: RawCore,
    #[serde(default)]
    plugins: BTreeMap<String, toml::Table>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawCore {
    tab_width: Option<u16>,
    indent: Option<RawIndent>,
    scroll_margin: Option<u16>,
    menu_key: Option<String>,
    plugins: Option<Vec<PathBuf>>,
    plugin_timeout_ms: Option<u64>,
    plugin_init_timeout_ms: Option<u64>,
    plugin_memory_mib: Option<usize>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawIndent {
    Spaces(u8),
    Name(String),
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, Error> {
        let fail = |message: String| Error::Config(message);
        let raw: RawConfig = toml::from_str(text).map_err(|err| fail(err.to_string()))?;
        let mut core = Settings::default();
        let raw_core = raw.core;

        if let Some(width) = raw_core.tab_width {
            if !(1..=16).contains(&width) {
                return Err(fail(format!("tab-width must be 1 to 16, not {width}")));
            }
            core.tab_width = width;
        }
        if let Some(indent) = raw_core.indent {
            core.indent = match indent {
                RawIndent::Spaces(n @ 1..=16) => Indent::Spaces(n),
                RawIndent::Name(name) if name == "tab" => Indent::Tab,
                RawIndent::Spaces(n) => {
                    return Err(fail(format!("indent must be 1 to 16 or \"tab\", not {n}")));
                }
                RawIndent::Name(name) => {
                    return Err(fail(format!(
                        "indent must be 1 to 16 or \"tab\", not {name:?}"
                    )));
                }
            };
        }
        if let Some(margin) = raw_core.scroll_margin {
            core.scroll_margin = margin;
        }
        if let Some(key) = raw_core.menu_key {
            core.menu_key = key
                .parse()
                .map_err(|err| fail(format!("menu-key: {err}")))?;
        }
        if let Some(plugins) = raw_core.plugins {
            core.plugins = plugins;
        }
        let timeout = |name: &str, ms: u64| {
            if ms < 10 {
                return Err(fail(format!("{name} must be at least 10, not {ms}")));
            }
            Ok(Duration::from_millis(ms))
        };
        if let Some(ms) = raw_core.plugin_timeout_ms {
            core.plugin_timeout = timeout("plugin-timeout-ms", ms)?;
        }
        if let Some(ms) = raw_core.plugin_init_timeout_ms {
            core.plugin_init_timeout = timeout("plugin-init-timeout-ms", ms)?;
        }
        if let Some(mib) = raw_core.plugin_memory_mib {
            if !(16..=4096).contains(&mib) {
                return Err(fail(format!(
                    "plugin-memory-mib must be 16 to 4096, not {mib}"
                )));
            }
            core.plugin_memory = mib << 20;
        }

        let plugins = raw
            .plugins
            .into_iter()
            .map(|(name, table)| {
                let json = serde_json::to_string(&table).map_err(|err| fail(err.to_string()))?;
                Ok((name, json))
            })
            .collect::<Result<_, Error>>()?;
        Ok(Self { core, plugins })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::KeyCode;

    #[test]
    fn empty_config_is_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn parses_core_and_plugin_tables() {
        let config = Config::parse(
            r#"
            [core]
            tab-width = 8
            indent = "tab"
            menu-key = "C-]"
            plugins = ["~/dev/my-plugin"]
            plugin-timeout-ms = 2000
            plugin-memory-mib = 512

            [plugins.helix]
            keys.normal = { "C-s" = "buffer.save" }
            "#,
        )
        .unwrap();
        assert_eq!(config.core.tab_width, 8);
        assert_eq!(config.core.indent, Indent::Tab);
        assert_eq!(config.core.menu_key, KeyEvent::ctrl(']'));
        assert_eq!(config.core.plugins, vec![PathBuf::from("~/dev/my-plugin")]);
        assert_eq!(config.core.plugin_timeout, Duration::from_secs(2));
        assert_eq!(config.core.plugin_init_timeout, Duration::from_secs(5));
        assert_eq!(config.core.plugin_memory, 512 << 20);
        assert_eq!(
            config.plugins["helix"],
            r#"{"keys":{"normal":{"C-s":"buffer.save"}}}"#
        );
    }

    #[test]
    fn rejects_mistakes() {
        for (text, expected) in [
            ("[core]\ntabwidth = 4", "unknown field"),
            ("[core]\ntab-width = 0", "tab-width must be"),
            ("[core]\nindent = \"spaces\"", "indent must be"),
            ("[core]\nmenu-key = \"C-nope\"", "menu-key"),
            ("[core]\nplugin-timeout-ms = 0", "plugin-timeout-ms must be"),
            ("[core]\nplugin-memory-mib = 1", "plugin-memory-mib must be"),
            ("[editor]\nx = 1", "unknown field"),
        ] {
            let err = Config::parse(text).unwrap_err().to_string();
            assert!(err.contains(expected), "{text:?}: {err}");
        }
    }

    #[test]
    fn plugins_read_editing_settings_as_json() {
        let settings = Settings::default();
        assert_eq!(settings.get_json("tab-width").as_deref(), Some("4"));
        assert_eq!(settings.get_json("indent").as_deref(), Some("4"));
        assert_eq!(settings.get_json("menu-key"), None);
        assert_eq!(settings.menu_key.code, KeyCode::Char('g'));
    }
}
