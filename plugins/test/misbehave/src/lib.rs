//! Test plugin that breaks on purpose, to check that the host survives it.

use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::input;
use nib_plugin::nib::plugin::types::{KeyCode, KeyEvent};

struct Misbehave;

impl Guest for Misbehave {
    fn init(config: String) -> Result<(), String> {
        if config.contains("\"fail\":true") {
            return Err("asked to fail".into());
        }
        input::push_layer();
        Ok(())
    }

    fn handle_key(ev: KeyEvent) -> KeyResult {
        match ev.code {
            KeyCode::Char('l') => loop {
                std::hint::spin_loop();
            },
            KeyCode::Char('m') => {
                let huge = vec![1u8; 1 << 30];
                std::hint::black_box(huge);
                KeyResult::Handled
            }
            KeyCode::Char('p') => panic!("asked to panic"),
            _ => KeyResult::Pass,
        }
    }
}

nib_plugin::export!(Misbehave);
