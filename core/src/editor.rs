use std::path::PathBuf;

use crate::Error;
use crate::buffer::Buffer;
use crate::input::{KeyCode, KeyEvent, Modifiers};
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
    /// The rescue menu is open and takes the next key.
    pub rescue: bool,
}

pub struct Editor {
    /// `None` only while lent to a plugin, when nothing else can reach the
    /// editor.
    pub(crate) state: Option<State>,
    pub(crate) plugins: Plugins,
}

const LENT: &str = "editor state is only lent during plugin calls";

/// Reserved by the core; plugins never see it.
const RESCUE_KEY: KeyEvent = KeyEvent::ctrl('q');

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
                rescue: false,
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

    /// Sends the key down the input stack until a plugin handles it.
    ///
    /// Ctrl-q never reaches plugins: it opens the rescue menu, so the editor
    /// can always be left even if a plugin swallows every key.
    pub fn handle_key(&mut self, key: KeyEvent) {
        self.state_mut().message = None;
        if self.state().rescue {
            self.state_mut().rescue = false;
            self.handle_rescue_key(key);
            return;
        }
        if key == RESCUE_KEY {
            self.state_mut().rescue = true;
            return;
        }
        let layers = self.state().layers.clone();
        for plugin in layers.into_iter().rev() {
            if self.plugin_handle_key(plugin, key) {
                return;
            }
        }
    }

    pub fn in_rescue_menu(&self) -> bool {
        self.state().rescue
    }

    /// Shown while no plugin takes input, when the rescue menu is the only
    /// thing that responds.
    pub fn key_hint(&self) -> Option<&'static str> {
        self.state().layers.is_empty().then_some("Ctrl-q: menu")
    }

    fn handle_rescue_key(&mut self, key: KeyEvent) {
        if key.modifiers != Modifiers::default() {
            return;
        }
        match key.code {
            KeyCode::Char('w') => match self.save_all() {
                Ok(()) => self.state_mut().quit = true,
                Err(failures) => {
                    self.state_mut().message =
                        Some(format!("not quitting: {}", failures.join("; ")));
                }
            },
            KeyCode::Char('q') => self.state_mut().quit = true,
            KeyCode::Char('r') => self.restart_plugins(),
            _ => {}
        }
    }

    /// Saves every modified buffer. Returns what could not be saved.
    fn save_all(&mut self) -> Result<(), Vec<String>> {
        let mut failures = Vec::new();
        for buffer in &mut self.state_mut().buffers {
            if !buffer.is_modified() {
                continue;
            }
            let Some(path) = buffer.path().map(|path| path.display().to_string()) else {
                failures.push("[scratch] has no path".to_string());
                continue;
            };
            if let Err(err) = buffer.save() {
                failures.push(format!("{path}: {err}"));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures)
        }
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::Edit;
    use crate::history::UndoMode;
    use crate::selection::Selection;

    fn press(editor: &mut Editor, keys: &[KeyEvent]) {
        for &key in keys {
            editor.handle_key(key);
        }
    }

    fn char_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c))
    }

    fn modify(editor: &mut Editor) {
        let buffer = &mut editor.state_mut().buffers[0];
        let version = buffer.version();
        buffer
            .apply(
                version,
                vec![Edit::insert(0, "x")],
                &Selection::point(0),
                None,
                UndoMode::NewStep,
            )
            .unwrap();
    }

    #[test]
    fn rescue_menu_quits_or_goes_back() {
        let mut editor = Editor::default();
        press(&mut editor, &[KeyEvent::ctrl('q')]);
        assert!(editor.in_rescue_menu());
        press(&mut editor, &[KeyEvent::new(KeyCode::Escape)]);
        assert!(!editor.in_rescue_menu());
        assert!(!editor.should_quit());

        press(&mut editor, &[KeyEvent::ctrl('q'), char_key('q')]);
        assert!(editor.should_quit());
    }

    #[test]
    fn rescue_menu_saves_before_quitting() {
        let path = std::env::temp_dir().join(format!("nib-{}-rescue.txt", std::process::id()));
        fs::write(&path, "a").unwrap();
        let mut editor = Editor::default();
        editor.open(&path).unwrap();
        modify(&mut editor);

        press(&mut editor, &[KeyEvent::ctrl('q'), char_key('w')]);
        let saved = fs::read_to_string(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(editor.should_quit());
        assert_eq!(saved, "xa");
    }

    #[test]
    fn rescue_menu_does_not_quit_when_saving_fails() {
        let mut editor = Editor::with_text("a");
        modify(&mut editor);
        press(&mut editor, &[KeyEvent::ctrl('q'), char_key('w')]);
        assert!(!editor.should_quit());
        assert_eq!(
            editor.message(),
            Some("not quitting: [scratch] has no path")
        );
    }
}
