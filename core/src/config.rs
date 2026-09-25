//! `config.toml`: `[core]` for the core, `[plugins.<name>]` for each plugin.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::Error;
use crate::grid::{Color, Style};
use crate::input::KeyEvent;
use crate::ui::Theme;

/// Settings of the core.
///
/// Editing behavior (`tab_width`, `indent`, `scroll_margin`) is for plugins
/// to read and, later, to override per buffer. Safety settings (`menu_key`,
/// `plugin_dirs`, and the plugin limits) are for the user alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub tab_width: u16,
    pub indent: Indent,
    /// Lines kept visible above and below the cursor.
    pub scroll_margin: u16,
    pub menu_key: KeyEvent,
    /// Plugin directories to load besides the built-in plugins.
    pub plugin_dirs: Vec<PathBuf>,
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
            plugin_dirs: Vec::new(),
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
    pub theme: Theme,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    core: RawCore,
    #[serde(default)]
    plugins: BTreeMap<String, toml::Table>,
    #[serde(default)]
    theme: BTreeMap<String, RawStyle>,
}

/// A color alone, or a table with colors and attributes.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawStyle {
    Color(RawColor),
    Table(RawStyleTable),
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStyleTable {
    fg: Option<RawColor>,
    bg: Option<RawColor>,
    #[serde(default)]
    bold: bool,
    #[serde(default)]
    italic: bool,
    #[serde(default)]
    underline: bool,
    #[serde(default)]
    reverse: bool,
}

/// A name such as "blue" or "bright-black", "#rrggbb", or 0 to 255.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawColor {
    Index(u8),
    Name(String),
}

const COLOR_NAMES: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

fn parse_color(color: &RawColor) -> Result<Color, String> {
    let name = match color {
        RawColor::Index(n) => return Ok(Color::Indexed(*n)),
        RawColor::Name(name) => name.as_str(),
    };
    if name == "default" {
        return Ok(Color::Reset);
    }
    if let Some(hex) = name.strip_prefix('#')
        && hex.len() == 6
        && let Ok(rgb) = u32::from_str_radix(hex, 16)
    {
        return Ok(Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8));
    }
    let (bright, base) = match name.strip_prefix("bright-") {
        Some(base) => (8, base),
        None => (0, name),
    };
    COLOR_NAMES
        .iter()
        .position(|c| *c == base)
        .map(|i| Color::Indexed(i as u8 + bright))
        .ok_or_else(|| format!("unknown color {name:?}"))
}

fn parse_style(style: &RawStyle) -> Result<Style, String> {
    let table = match style {
        RawStyle::Color(color) => {
            return Ok(Style {
                fg: parse_color(color)?,
                ..Style::default()
            });
        }
        RawStyle::Table(table) => table,
    };
    let color = |c: &Option<RawColor>| c.as_ref().map_or(Ok(Color::Reset), parse_color);
    Ok(Style {
        fg: color(&table.fg)?,
        bg: color(&table.bg)?,
        bold: table.bold,
        italic: table.italic,
        underline: table.underline,
        reverse: table.reverse,
    })
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawCore {
    tab_width: Option<u16>,
    indent: Option<RawIndent>,
    scroll_margin: Option<u16>,
    menu_key: Option<String>,
    plugin_dirs: Option<Vec<PathBuf>>,
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
        if let Some(dirs) = raw_core.plugin_dirs {
            core.plugin_dirs = dirs;
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
        let theme = raw
            .theme
            .iter()
            .map(|(name, style)| {
                let style =
                    parse_style(style).map_err(|err| fail(format!("theme.{name}: {err}")))?;
                Ok((name.clone(), style))
            })
            .collect::<Result<_, Error>>()?;
        Ok(Self {
            core,
            plugins,
            theme: Theme::new(theme),
        })
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
            plugin-dirs = ["~/dev/my-plugin"]
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
        assert_eq!(
            config.core.plugin_dirs,
            vec![PathBuf::from("~/dev/my-plugin")]
        );
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
    fn parses_the_theme() {
        let config = Config::parse(
            r##"
            [theme]
            keyword = "magenta"
            "function.macro" = "#8be9fd"
            comment = { fg = "bright-black", italic = true }
            "ui.selection" = { bg = 238 }
            "##,
        )
        .unwrap();
        let theme = &config.theme;
        assert_eq!(theme.style("keyword").unwrap().fg, Color::Indexed(5));
        assert_eq!(
            theme.style("function.macro").unwrap().fg,
            Color::Rgb(0x8b, 0xe9, 0xfd)
        );
        let comment = theme.style("comment").unwrap();
        assert_eq!(comment.fg, Color::Indexed(8));
        assert!(comment.italic);
        assert_eq!(theme.style("ui.selection").unwrap().bg, Color::Indexed(238));

        let err = Config::parse("[theme]\nkeyword = \"purple\"").unwrap_err();
        assert!(
            err.to_string().contains("theme.keyword: unknown color"),
            "{err}"
        );
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
