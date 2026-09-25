use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use crate::Error;
use crate::buffer::Buffer;
use crate::config::{Config, Settings};
use crate::input::{KeyCode, KeyEvent};
use crate::layout;
use crate::plugin::{PluginId, Plugins};
use crate::syntax::{BufferSyntax, Languages};
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
    /// Views of buffers not shown, so switching back restores the selection
    /// and scroll position.
    pub hidden_views: HashMap<usize, View>,
    pub languages: Languages,
}

impl State {
    /// Opens `path` in the view. The initial empty buffer is replaced if it
    /// was never touched.
    pub fn open(&mut self, path: impl Into<PathBuf>) -> Result<(), Error> {
        let path = path.into();
        let same_file = |other: &std::path::Path| {
            other == path
                || matches!(
                    (other.canonicalize(), path.canonicalize()),
                    (Ok(a), Ok(b)) if a == b
                )
        };
        if let Some(open) = self
            .buffers
            .iter()
            .position(|b| b.path().is_some_and(same_file))
        {
            self.switch_to(open);
            return Ok(());
        }
        let mut buffer = Buffer::open(path)?;
        if let Some(language) = buffer.path().and_then(|p| self.languages.for_path(p)) {
            buffer.syntax = Some(BufferSyntax::new(language));
        }
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
            self.switch_to(self.buffers.len() - 1);
        }
        Ok(())
    }

    /// Gives buffers without a language the one for their file type.
    pub fn attach_syntax(&mut self) {
        for buffer in &mut self.buffers {
            if buffer.syntax.is_none()
                && let Some(language) = buffer.path().and_then(|p| self.languages.for_path(p))
            {
                buffer.syntax = Some(BufferSyntax::new(language));
            }
        }
    }

    /// Parses the shown buffer if it changed since the last parse, loading
    /// its grammar the first time. Hidden buffers wait until they are shown.
    /// Returns whether it parsed.
    pub fn update_syntax(&mut self) -> bool {
        let buffer = &mut self.buffers[self.view.buffer];
        let text = buffer.text().clone();
        let Some(syntax) = buffer.syntax.as_mut().filter(|s| s.dirty) else {
            return false;
        };
        match self
            .languages
            .parse(syntax.language, &text, syntax.tree.as_ref())
        {
            Ok(tree) => {
                syntax.tree = tree;
                syntax.dirty = false;
            }
            Err(err) => {
                // Shown without highlighting from now on.
                buffer.syntax = None;
                self.message = Some(format!("syntax: {err}"));
            }
        }
        true
    }

    /// Highlight styles for the bytes in `range` of the shown buffer, if it
    /// has a parsed syntax tree.
    pub fn syntax_styles(
        &self,
        range: std::ops::Range<usize>,
    ) -> Option<Vec<Option<crate::grid::Style>>> {
        let buffer = &self.buffers[self.view.buffer];
        let syntax = buffer.syntax.as_ref()?;
        let tree = syntax.tree.as_ref()?;
        Some(
            self.languages
                .highlight(syntax.language, tree, buffer.text(), range),
        )
    }

    /// Shows buffer `index`, keeping the view of the current one for later.
    pub fn switch_to(&mut self, index: usize) {
        if index == self.view.buffer {
            return;
        }
        let view = self
            .hidden_views
            .remove(&index)
            .unwrap_or_else(|| View::new(index));
        let old = std::mem::replace(&mut self.view, view);
        self.hidden_views.insert(old.buffer, old);
    }

    /// Rows left for text above the panels and the status line.
    pub fn text_rows(&self) -> u16 {
        let status = u16::from(self.height > 1);
        let panels: usize = self.panels.iter().map(|p| p.lines.len()).sum();
        self.height
            .saturating_sub(status)
            .saturating_sub(panels.min(u16::MAX as usize) as u16)
    }

    /// From the start of the first line shown to the end of the last one.
    pub fn visible_range(&self) -> (usize, usize) {
        let buffer = &self.buffers[self.view.buffer];
        let top = self.view.top_line;
        let start = buffer.line_start(top).unwrap_or(buffer.len());
        let end = buffer
            .line_start(top + self.text_rows() as usize)
            .unwrap_or(buffer.len());
        (start, end)
    }

    /// Scrolls the view without moving the cursor. Returns the number of
    /// lines `amount` stands for, so a keymap can move the cursor as far.
    pub fn scroll(&mut self, amount: ScrollAmount) -> i32 {
        let rows = i32::from(self.text_rows().max(1));
        let lines = match amount {
            ScrollAmount::Lines(n) => n,
            ScrollAmount::HalfPage(n) => n.saturating_mul((rows / 2).max(1)),
            ScrollAmount::Page(n) => n.saturating_mul(rows),
        };
        let last = self.buffers[self.view.buffer]
            .line_count()
            .saturating_sub(1);
        let top = self.view.top_line as i64 + i64::from(lines);
        self.view.top_line = top.clamp(0, last as i64) as usize;
        lines
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
            "buffer.next" | "buffer.previous" => {
                let count = self.buffers.len();
                let step = if name == "buffer.next" { 1 } else { count - 1 };
                self.switch_to((self.view.buffer + step) % count);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollAmount {
    Lines(i32),
    HalfPage(i32),
    Page(i32),
}

/// The core menu, opened with the reserved menu key. It is drawn and
/// handled by the core alone, so it works however broken the plugins are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    /// Lists the plugins.
    Main,
    /// Actions on one plugin.
    Plugin(PluginId),
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
                hidden_views: HashMap::new(),
                languages: Languages::default(),
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
        let options = &mut self.plugins.options;
        options.call_timeout = config.core.plugin_timeout;
        options.init_timeout = config.core.plugin_init_timeout;
        options.memory_limit = config.core.plugin_memory;
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

    /// Does work left for after a frame, such as the first parse of a
    /// buffer, so opening a file shows it before its highlighting. Returns
    /// whether the screen needs drawing again.
    pub fn catch_up(&mut self) -> bool {
        self.state_mut().update_syntax()
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
        self.state_mut().update_syntax();
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
        let cursor = state.view.cursor(text);
        let line = text.byte_to_line(cursor);
        let column = layout::column_of(text, cursor, state.settings.tab_width);
        let width = u32::from(state.width.max(1));
        let margin = (state.settings.scroll_margin as usize).min((rows - 1) / 2);
        let view = &mut state.view;
        if line < view.top_line + margin {
            view.top_line = line.saturating_sub(margin);
        } else if line + margin >= view.top_line + rows {
            view.top_line = line + margin + 1 - rows;
        }
        if column < view.left_col {
            view.left_col = column;
        } else if column >= view.left_col + width {
            view.left_col = column + 1 - width;
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
            Menu::Main => {
                let chosen = match key.code {
                    KeyCode::Char(c @ '1'..='9') if key.modifiers == Default::default() => {
                        Some(c as usize - '1' as usize)
                    }
                    _ => None,
                };
                if let Some(id) = chosen.filter(|&id| id < self.plugins().len()) {
                    self.state_mut().menu = Some(Menu::Plugin(id));
                }
            }
            Menu::Plugin(id) if plain('r') || plain('d') || plain('l') => {
                let plugin = &self.plugins()[id];
                let name = plugin.name.clone();
                let message = if plain('r') {
                    match self.restart_plugin(id) {
                        Ok(()) => format!("{name} restarted"),
                        Err(err) => format!("{name}: restarting failed: {err}"),
                    }
                } else if plain('d') && plugin.enabled {
                    self.disable_plugin(id);
                    format!("{name} disabled")
                } else if plain('d') {
                    match self.restart_plugin(id) {
                        Ok(()) => format!("{name} enabled"),
                        Err(err) => format!("{name}: enabling failed: {err}"),
                    }
                } else {
                    match self.reload_plugin(id) {
                        Ok(()) => format!("{name} reloaded"),
                        Err(err) => format!("{name}: reloading failed: {err}"),
                    }
                };
                self.state_mut().message = Some(message);
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
        editor.resize(20, 10); // 9 text rows: the margin of 5 shrinks to 4
        let line_start = |editor: &Editor, line: usize| editor.buffer().line_start(line).unwrap();

        let pos = line_start(&editor, 50);
        editor.view_mut().selection = Selection::point(pos);
        press(&mut editor, &[KeyEvent::new(KeyCode::Escape)]);
        assert_eq!(editor.view().top_line, 46);

        let pos = line_start(&editor, 44);
        editor.view_mut().selection = Selection::point(pos);
        press(&mut editor, &[KeyEvent::new(KeyCode::Escape)]);
        assert_eq!(editor.view().top_line, 40);

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
    fn switching_buffers_keeps_each_view() {
        let dir = std::env::temp_dir();
        let a = dir.join(format!("nib-{}-a.txt", std::process::id()));
        let b = dir.join(format!("nib-{}-b.txt", std::process::id()));
        fs::write(&a, "aaa").unwrap();
        fs::write(&b, "bbb").unwrap();
        let mut editor = Editor::default();
        editor.open(&a).unwrap();
        editor.view_mut().selection = Selection::point(2);
        editor.open(&b).unwrap();
        assert_eq!(editor.buffer().text().to_string(), "bbb");
        assert_eq!(editor.view().selection, Selection::point(0));

        let state = editor.state_mut();
        state.run_command("buffer.previous", "").unwrap();
        assert_eq!(editor.buffer().text().to_string(), "aaa");
        assert_eq!(editor.view().selection, Selection::point(2));
        editor.state_mut().run_command("buffer.next", "").unwrap();
        assert_eq!(editor.buffer().text().to_string(), "bbb");

        // Opening a file that is open switches to it.
        editor.open(&a).unwrap();
        assert_eq!(editor.state().buffers.len(), 2);
        assert_eq!(editor.view().selection, Selection::point(2));
        fs::remove_file(&a).unwrap();
        fs::remove_file(&b).unwrap();
    }

    #[test]
    fn scrolling_reports_lines_and_stops_at_the_ends() {
        let text: String = (0..50).map(|i| format!("{i}\n")).collect();
        let mut editor = Editor::with_text(&text);
        editor.resize(10, 11); // 10 text rows
        let state = editor.state_mut();
        assert_eq!(state.scroll(ScrollAmount::HalfPage(1)), 5);
        assert_eq!(state.view.top_line, 5);
        assert_eq!(state.scroll(ScrollAmount::Page(-1)), -10);
        assert_eq!(state.view.top_line, 0);
        state.scroll(ScrollAmount::Lines(100));
        assert_eq!(state.view.top_line, 50);
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
