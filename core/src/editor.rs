use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::Error;
use crate::buffer::Buffer;
use crate::config::{Config, Settings};
use crate::input::{KeyCode, KeyEvent};
use crate::plugin::{PluginId, Plugins};
use crate::ui::{Panel, StatusItem};
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
    /// The core menu is open and takes the next key.
    pub menu: Option<Menu>,
    pub settings: Settings,
    pub status: Vec<StatusItem>,
    /// Bottom panels, in the order they were opened.
    pub panels: Vec<Panel>,
    pub last_panel_id: u32,
}

impl State {
    /// Opens `path` in the view. The initial empty buffer is replaced if it
    /// was never touched.
    pub fn open(&mut self, path: impl Into<PathBuf>) -> Result<(), Error> {
        let buffer = Buffer::open(path)?;
        let scratch = &self.buffers[0];
        if self.buffers.len() == 1
            && scratch.path().is_none()
            && scratch.is_empty()
            && !scratch.is_modified()
        {
            self.buffers[0] = buffer;
            self.view = View::new(0);
        } else {
            self.buffers.push(buffer);
            self.view = View::new(self.buffers.len() - 1);
        }
        Ok(())
    }

    pub fn modified_buffers(&self) -> usize {
        self.buffers.iter().filter(|b| b.is_modified()).count()
    }

    /// Runs a core command. Arguments and the result are JSON.
    pub fn run_command(&mut self, name: &str, args: &str) -> Result<String, String> {
        let args: serde_json::Value = match args.trim() {
            "" => serde_json::Value::Null,
            args => serde_json::from_str(args)
                .map_err(|err| format!("{name}: invalid arguments: {err}"))?,
        };
        match name {
            "buffer.save" => {
                let buffer = &mut self.buffers[self.view.buffer];
                buffer.save().map_err(|err| err.to_string())?;
            }
            "buffer.open" => {
                let path = args["path"]
                    .as_str()
                    .ok_or(r#"buffer.open needs {"path": string}"#)?;
                self.open(path).map_err(|err| format!("{path}: {err}"))?;
            }
            "editor.quit" => {
                let force = args["force"].as_bool().unwrap_or(false);
                let modified = self.modified_buffers();
                if modified > 0 && !force {
                    let buffers = if modified == 1 {
                        "buffer has"
                    } else {
                        "buffers have"
                    };
                    return Err(format!("{modified} {buffers} unsaved changes"));
                }
                self.quit = true;
            }
            _ => return Err(format!("no command named {name}")),
        }
        Ok("null".into())
    }

    /// Removes everything `plugin` put into the editor.
    pub fn remove_plugin_parts(&mut self, plugin: PluginId) {
        self.layers.retain(|&layer| layer != plugin);
        self.status.retain(|item| item.owner != plugin);
        self.panels.retain(|panel| panel.owner != plugin);
    }
}

/// The core menu, opened with the reserved menu key. It is drawn and
/// handled by the core alone, so it works however broken the plugins are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    Main,
    /// Quitting would drop unsaved changes.
    ConfirmQuit,
}

pub struct Editor {
    /// `None` only while lent to a plugin, when nothing else can reach the
    /// editor.
    pub(crate) state: Option<State>,
    pub(crate) plugins: Plugins,
    /// Each plugin's table from config.toml as JSON.
    plugin_configs: BTreeMap<String, String>,
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
                menu: None,
                settings: Settings::default(),
                status: Vec::new(),
                panels: Vec::new(),
                last_panel_id: 0,
            }),
            plugins: Plugins::default(),
            plugin_configs: BTreeMap::new(),
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

    /// Applies config.toml. Call before loading plugins, which get their
    /// tables from it.
    pub fn apply_config(&mut self, config: Config) {
        self.state_mut().settings = config.core;
        self.plugin_configs = config.plugins;
    }

    pub fn settings(&self) -> &Settings {
        &self.state().settings
    }

    /// The plugin's table from config.toml as JSON, or `{}`.
    pub fn plugin_config(&self, name: &str) -> &str {
        self.plugin_configs.get(name).map_or("{}", String::as_str)
    }

    /// Opens `path` in the view. The initial empty buffer is replaced if it
    /// was never touched.
    pub fn open(&mut self, path: impl Into<PathBuf>) -> Result<(), Error> {
        self.state_mut().open(path)
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        let state = self.state_mut();
        state.width = width;
        state.height = height;
        self.scroll_to_cursor();
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

    /// Shows `message` until the next key.
    pub fn show_message(&mut self, message: impl Into<String>) {
        self.state_mut().message = Some(message.into());
    }

    /// Sends the key down the input stack until a plugin handles it.
    ///
    /// The menu key never goes to plugins: it opens the core menu, so
    /// plugins can always be managed even if one swallows every key.
    pub fn handle_key(&mut self, key: KeyEvent) {
        self.state_mut().message = None;
        if let Some(menu) = self.state_mut().menu.take() {
            self.handle_menu_key(menu, key);
        } else if key == self.settings().menu_key {
            self.state_mut().menu = Some(Menu::Main);
        } else {
            self.send_to_plugins(key);
        }
        self.scroll_to_cursor();
    }

    /// Scrolls the view so the cursor stays `scroll_margin` lines away from
    /// the top and bottom edges where possible.
    fn scroll_to_cursor(&mut self) {
        let rows = self.text_rows() as usize;
        let state = self.state_mut();
        if rows == 0 {
            return;
        }
        let text = state.buffers[state.view.buffer].text();
        let line = text.byte_to_line(state.view.cursor(text));
        let margin = (state.settings.scroll_margin as usize).min((rows - 1) / 2);
        let view = &mut state.view;
        if line < view.top_line + margin {
            view.top_line = line.saturating_sub(margin);
        } else if line + margin >= view.top_line + rows {
            view.top_line = line + margin + 1 - rows;
        }
    }

    fn send_to_plugins(&mut self, key: KeyEvent) {
        let layers = self.state().layers.clone();
        for plugin in layers.into_iter().rev() {
            if self.plugin_handle_key(plugin, key) {
                return;
            }
        }
    }

    pub fn menu(&self) -> Option<Menu> {
        self.state().menu
    }

    /// Shown while no plugin takes input, when the menu is the only thing
    /// that responds.
    pub fn key_hint(&self) -> Option<String> {
        let state = self.state();
        state
            .layers
            .is_empty()
            .then(|| format!("{}: menu", state.settings.menu_key))
    }

    pub fn modified_buffers(&self) -> usize {
        self.state().modified_buffers()
    }

    fn handle_menu_key(&mut self, menu: Menu, key: KeyEvent) {
        let plain = |c: char| key == KeyEvent::new(KeyCode::Char(c));
        match menu {
            Menu::Main if plain('r') => self.restart_plugins(),
            Menu::Main if plain('w') => match self.save_all() {
                Ok(()) => self.state_mut().quit = true,
                Err(failures) => {
                    self.state_mut().message =
                        Some(format!("not quitting: {}", failures.join("; ")));
                }
            },
            Menu::Main if plain('q') => {
                if self.modified_buffers() == 0 {
                    self.state_mut().quit = true;
                } else {
                    self.state_mut().menu = Some(Menu::ConfirmQuit);
                }
            }
            Menu::ConfirmQuit if plain('y') => self.state_mut().quit = true,
            // Any other key goes back, so a mistyped menu key is harmless.
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
    fn view_follows_the_cursor_with_a_margin() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let mut editor = Editor::with_text(&text);
        editor.resize(20, 10); // 9 text rows, margin 3
        let line_start = |editor: &Editor, line: usize| editor.buffer().line_start(line).unwrap();

        let pos = line_start(&editor, 50);
        editor.view_mut().selection = Selection::point(pos);
        press(&mut editor, &[KeyEvent::new(KeyCode::Escape)]);
        assert_eq!(editor.view().top_line, 45);

        let pos = line_start(&editor, 44);
        editor.view_mut().selection = Selection::point(pos);
        press(&mut editor, &[KeyEvent::new(KeyCode::Escape)]);
        assert_eq!(editor.view().top_line, 41);

        let pos = line_start(&editor, 1);
        editor.view_mut().selection = Selection::point(pos);
        press(&mut editor, &[KeyEvent::new(KeyCode::Escape)]);
        assert_eq!(editor.view().top_line, 0);
    }

    #[test]
    fn menu_key_comes_from_config() {
        let mut editor = Editor::default();
        editor.apply_config(Config::parse("[core]\nmenu-key = \"C-]\"").unwrap());
        assert_eq!(editor.key_hint().as_deref(), Some("Ctrl-]: menu"));
        press(&mut editor, &[KeyEvent::ctrl('g')]);
        assert_eq!(editor.menu(), None);
        press(&mut editor, &[KeyEvent::ctrl(']')]);
        assert_eq!(editor.menu(), Some(Menu::Main));
    }

    #[test]
    fn core_commands() {
        let mut editor = Editor::with_text("a");
        let state = editor.state_mut();
        assert_eq!(
            state.run_command("buffer.save", ""),
            Err("buffer has no path".into())
        );
        assert_eq!(
            state.run_command("nope", "{}"),
            Err("no command named nope".into())
        );
        assert!(state.run_command("buffer.open", "{}").is_err());

        modify(&mut editor);
        let state = editor.state_mut();
        assert_eq!(
            state.run_command("editor.quit", ""),
            Err("1 buffer has unsaved changes".into())
        );
        assert!(!state.quit);
        assert_eq!(
            state.run_command("editor.quit", r#"{"force":true}"#),
            Ok("null".into())
        );
        assert!(state.quit);
    }

    #[test]
    fn menu_quits_or_goes_back() {
        let mut editor = Editor::default();
        press(&mut editor, &[KeyEvent::ctrl('g')]);
        assert_eq!(editor.menu(), Some(Menu::Main));
        press(&mut editor, &[KeyEvent::new(KeyCode::Escape)]);
        assert_eq!(editor.menu(), None);
        assert!(!editor.should_quit());

        press(&mut editor, &[KeyEvent::ctrl('g'), char_key('q')]);
        assert!(editor.should_quit());
    }

    #[test]
    fn quitting_with_unsaved_changes_needs_confirmation() {
        let mut editor = Editor::with_text("a");
        modify(&mut editor);
        press(&mut editor, &[KeyEvent::ctrl('g'), char_key('q')]);
        assert_eq!(editor.menu(), Some(Menu::ConfirmQuit));
        assert!(!editor.should_quit());

        // Anything but "y" goes back, e.g. a second "q" from a typo.
        press(&mut editor, &[char_key('q')]);
        assert_eq!(editor.menu(), None);
        assert!(!editor.should_quit());

        press(
            &mut editor,
            &[KeyEvent::ctrl('g'), char_key('q'), char_key('y')],
        );
        assert!(editor.should_quit());
    }

    #[test]
    fn menu_saves_before_quitting() {
        let path = std::env::temp_dir().join(format!("nib-{}-menu.txt", std::process::id()));
        fs::write(&path, "a").unwrap();
        let mut editor = Editor::default();
        editor.open(&path).unwrap();
        modify(&mut editor);

        press(&mut editor, &[KeyEvent::ctrl('g'), char_key('w')]);
        let saved = fs::read_to_string(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(editor.should_quit());
        assert_eq!(saved, "xa");
    }

    #[test]
    fn menu_does_not_quit_when_saving_fails() {
        let mut editor = Editor::with_text("a");
        modify(&mut editor);
        press(&mut editor, &[KeyEvent::ctrl('g'), char_key('w')]);
        assert!(!editor.should_quit());
        assert_eq!(
            editor.message(),
            Some("not quitting: [scratch] has no path")
        );
    }
}
