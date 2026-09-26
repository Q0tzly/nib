//! Indentation by language: when a buffer opens, sets its "indent" and
//! "tab-width" from `[settings.languages.<name>]` in plugins/indent.toml,
//! over the defaults below. The core keeps no per-language settings.

use std::cell::RefCell;

use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::editor::{self, Buffer};
use nib_plugin::nib::plugin::events::Event;
use nib_plugin::nib::plugin::types::KeyEvent;
use nib_plugin::nib::plugin::{settings, syntax, ui};
use serde_json::{Map, Value, json};

/// The settings each language needs, where it differs from config.toml's.
fn defaults() -> Value {
    json!({
        "go": {"indent": "tab"},
        "yaml": {"indent": 2},
        "json": {"indent": 2},
    })
}

const KEYS: [&str; 2] = ["indent", "tab-width"];

thread_local! {
    /// Settings by language.
    static LANGUAGES: RefCell<Map<String, Value>> = RefCell::new(Map::new());
}

struct Plugin;

impl Guest for Plugin {
    fn init(config: String) -> Result<(), String> {
        let config: Value = serde_json::from_str(&config).map_err(|err| err.to_string())?;
        let Value::Object(mut languages) = defaults() else {
            unreachable!("an object");
        };
        // A language's keys in the settings replace the defaults' one by one.
        for (language, keys) in config["languages"].as_object().into_iter().flatten() {
            let entry = languages
                .entry(language.clone())
                .or_insert_with(|| json!({}));
            for (key, value) in keys.as_object().into_iter().flatten() {
                entry[key] = value.clone();
            }
        }
        LANGUAGES.with_borrow_mut(|l| *l = languages);
        // Buffers opened before, e.g. when restarted from the menu.
        for buffer in editor::buffers() {
            apply(&buffer);
        }
        Ok(())
    }

    fn handle_key(_ev: KeyEvent) -> KeyResult {
        KeyResult::Pass
    }

    fn run_command(name: String, _args: String) -> Result<String, String> {
        Err(format!("no command {name}"))
    }

    fn on_event(ev: Event) {
        if let Event::BufferOpened(buffer) = ev {
            apply(&buffer);
        }
    }
}

/// Sets the buffer's settings for its language.
fn apply(buffer: &Buffer) {
    let Some(language) = syntax::language(buffer) else {
        return;
    };
    let Some(keys) = LANGUAGES.with_borrow(|l| l.get(&language).cloned()) else {
        return;
    };
    for key in KEYS {
        if let Some(value) = keys.get(key)
            && let Err(err) = settings::set_for(buffer, key, Some(&value.to_string()))
        {
            ui::show_message(&format!("indent.toml: languages.{language}.{err}"));
        }
    }
}

nib_plugin::export!(Plugin);
