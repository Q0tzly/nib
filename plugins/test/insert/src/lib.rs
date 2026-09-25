//! Test plugin: inserts typed chars at every cursor. Escape removes its
//! input layer.

use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::types::{Edit, KeyCode, KeyEvent, UndoMode};
use nib_plugin::nib::plugin::{editor, input};

struct Insert;

impl Guest for Insert {
    fn init(_config: String) -> Result<(), String> {
        input::push_layer();
        Ok(())
    }

    fn handle_key(ev: KeyEvent) -> KeyResult {
        match ev.code {
            KeyCode::Char(c) if ev.modifiers.is_empty() => {
                let view = editor::active_view();
                let edits: Vec<Edit> = view
                    .selection()
                    .ranges
                    .iter()
                    .map(|range| Edit {
                        start: range.head,
                        end: range.head,
                        text: c.to_string(),
                    })
                    .collect();
                let version = view.buffer().version();
                if view.apply(version, &edits, None, UndoMode::Merge).is_err() {
                    return KeyResult::Pass;
                }
                KeyResult::Handled
            }
            KeyCode::Escape => {
                input::pop_layer();
                KeyResult::Handled
            }
            _ => KeyResult::Pass,
        }
    }
}

nib_plugin::export!(Insert);
