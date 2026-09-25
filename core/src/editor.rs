use std::path::PathBuf;

use crate::Error;
use crate::buffer::Buffer;
use crate::input::KeyEvent;
use crate::plugin::{PluginId, Plugins};
use crate::view::View;

/// Everything plugins can see and change. While a plugin runs, it is lent to
/// that plugin's store so host functions can reach it.
pub(crate) struct State {
    pub buffers: Vec<Buffer>,
    pub view: View,
    pub width: u16,
    pub height: u16,
    pub quit: bool,
    /// Plugins taking keys, bottom first.
    pub layers: Vec<PluginId>,
    /// Shown to the user until the next key, e.g. a plugin error.
    pub message: Option<String>,
}

pub struct Editor {
    /// `None` only while lent to a plugin, when nothing else can reach the
    /// editor.
    pub(crate) state: Option<State>,
    pub(crate) plugins: Plugins,
}

const LENT: &str = "editor state is only lent during plugin calls";

impl Default for Editor {
    /// Starts with an empty buffer that has no path.
    fn default() -> Self {
        Self {
            state: Some(State {
                buffers: vec![Buffer::default()],
                view: View::new(0),
                width: 0,
                height: 0,
                quit: false,
                layers: Vec::new(),
                message: None,
            }),
            plugins: Plugins::default(),
        }
    }
}

impl Editor {
    pub(crate) fn state(&self) -> &State {
        self.state.as_ref().expect(LENT)
    }

    pub(crate) fn state_mut(&mut self) -> &mut State {
        self.state.as_mut().expect(LENT)
    }

    /// Opens `path` in the view. The initial empty buffer is replaced if it
    /// was never touched.
    pub fn open(&mut self, path: impl Into<PathBuf>) -> Result<(), Error> {
        let buffer = Buffer::open(path)?;
        let state = self.state_mut();
        let scratch = &state.buffers[0];
        if state.buffers.len() == 1
            && scratch.path().is_none()
            && scratch.is_empty()
            && !scratch.is_modified()
        {
            state.buffers[0] = buffer;
            state.view = View::new(0);
        } else {
            state.buffers.push(buffer);
            state.view = View::new(state.buffers.len() - 1);
        }
        Ok(())
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        let state = self.state_mut();
        state.width = width;
        state.height = height;
    }

    pub fn size(&self) -> (u16, u16) {
        (self.state().width, self.state().height)
    }

    pub fn view(&self) -> &View {
        &self.state().view
    }

    pub fn view_mut(&mut self) -> &mut View {
        &mut self.state_mut().view
    }

    pub fn buffer(&self) -> &Buffer {
        let state = self.state();
        &state.buffers[state.view.buffer]
    }

    pub fn message(&self) -> Option<&str> {
        self.state().message.as_deref()
    }

    /// Sends the key down the input stack until a plugin handles it. With no
    /// plugin taking input, only the emergency keys work.
    pub fn handle_key(&mut self, key: KeyEvent) {
        self.state_mut().message = None;
        let layers = self.state().layers.clone();
        if layers.is_empty() {
            if key == KeyEvent::ctrl('q') {
                self.state_mut().quit = true;
            }
            return;
        }
        for plugin in layers.into_iter().rev() {
            if self.plugin_handle_key(plugin, key) {
                return;
            }
        }
    }

    /// Keys that work only while no plugin takes input, so the editor can
    /// always be left even if the keymap plugin is broken.
    pub fn emergency_keys_hint(&self) -> Option<&'static str> {
        self.state().layers.is_empty().then_some("Ctrl-q: quit")
    }

    pub fn should_quit(&self) -> bool {
        self.state().quit
    }

    #[cfg(test)]
    pub(crate) fn with_text(text: &str) -> Self {
        let mut editor = Self::default();
        editor.state_mut().buffers[0] = Buffer::with_text(text);
        editor
    }
}
