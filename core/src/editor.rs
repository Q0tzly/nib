use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use ropey::Rope;
use tree_sitter::Tree;

use crate::Error;
use crate::background::{Inbox, Message, Waker};
use crate::buffer::Buffer;
use crate::clipboard::{self, Clipboard};
use crate::config::{Config, Indent, PluginConfig, Settings};
use crate::events::{Command, Event, Timer};
use crate::files::FileJobs;
use crate::input::{KeyCode, KeyEvent};
use crate::layout;
use crate::plugin::{PluginId, Plugins};
use crate::process::Processes;
use crate::syntax::{BufferSyntax, Languages};
use crate::ui::{Panel, Popup, StatusItem, Theme};
use crate::view::View;
use std::sync::Arc;

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
    /// Popups, drawn in the order they were opened.
    pub popups: Vec<Popup>,
    pub last_popup_id: u32,
    /// Events waiting for the current call to end, with the plugin they
    /// are for, or `None` for every plugin that listens to their kind.
    pub events: VecDeque<(Option<PluginId>, Event)>,
    /// Commands plugins registered.
    pub commands: Vec<Command>,
    pub timers: Vec<Timer>,
    pub last_timer_id: u64,
    /// Where background threads queue their messages.
    pub inbox: Arc<Inbox>,
    /// Programs plugins started.
    pub processes: Processes,
    /// File lists being made for plugins.
    pub files: FileJobs,
    pub clipboard: Box<dyn Clipboard>,
    /// Views of buffers not shown, so switching back restores the selection
    /// and scroll position.
    pub hidden_views: HashMap<usize, View>,
    pub languages: Languages,
    pub theme: Theme,
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
            self.push_event(None, Event::BufferOpened(0));
        } else {
            self.buffers.push(buffer);
            self.switch_to(self.buffers.len() - 1);
            self.push_event(None, Event::BufferOpened(self.buffers.len() - 1));
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

    /// Parses the shown buffer if it changed since the last parse. Hidden
    /// buffers wait until they are shown. Returns whether it parsed.
    pub fn update_syntax(&mut self) -> bool {
        self.parse(self.view.buffer)
    }

    /// Parses buffer `index` if it changed since the last parse, loading its
    /// grammar the first time. Returns whether it parsed.
    fn parse(&mut self, index: usize) -> bool {
        let buffer = &mut self.buffers[index];
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

    /// Runs `f` with the up-to-date syntax tree of buffer `index`, if it has
    /// one, and the id of its language.
    pub(crate) fn with_tree<R>(
        &mut self,
        index: usize,
        f: impl FnOnce(&mut Languages, usize, &Tree, &Rope) -> R,
    ) -> Option<R> {
        self.parse(index);
        let buffer = &self.buffers[index];
        let syntax = buffer.syntax.as_ref()?;
        Some(f(
            &mut self.languages,
            syntax.language,
            syntax.tree.as_ref()?,
            buffer.text(),
        ))
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
                .highlight(&self.theme, syntax.language, tree, buffer.text(), range),
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
                let index = self.view.buffer;
                self.buffers[index].save().map_err(|err| err.to_string())?;
                self.push_event(None, Event::BufferSaved(index));
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

    /// The tab width of buffer `index`.
    pub fn tab_width(&self, index: usize) -> u16 {
        self.buffers[index]
            .overrides
            .tab_width
            .map_or(self.settings.tab_width, |(_, width)| width)
    }

    /// An editing setting of buffer `index` as JSON.
    pub fn setting_json(&self, index: usize, key: &str) -> Option<String> {
        let overrides = &self.buffers[index].overrides;
        match key {
            "tab-width" => Some(self.tab_width(index).to_string()),
            "indent" => {
                let indent = overrides
                    .indent
                    .map_or(self.settings.indent, |(_, indent)| indent);
                Some(indent.to_json().to_string())
            }
            _ => self.settings.get_json(key),
        }
    }

    /// Sets an editing setting for buffer `index` alone, or with `None`
    /// goes back to config.toml's value.
    pub fn set_setting(
        &mut self,
        index: usize,
        owner: PluginId,
        key: &str,
        value: Option<&str>,
    ) -> Result<(), String> {
        let value: Option<serde_json::Value> = value
            .map(serde_json::from_str)
            .transpose()
            .map_err(|err| format!("{key}: {err}"))?;
        let overrides = &mut self.buffers[index].overrides;
        match key {
            "tab-width" => {
                overrides.tab_width = value
                    .map(|v| {
                        let width = v
                            .as_u64()
                            .filter(|w| (1..=16).contains(w))
                            .ok_or("tab-width must be 1 to 16")?;
                        Ok::<_, String>((owner, width as u16))
                    })
                    .transpose()?;
            }
            "indent" => {
                overrides.indent = value
                    .map(|v| {
                        let indent = Indent::from_json(&v)
                            .ok_or("indent must be \"tab\" or 1 to 16 spaces")?;
                        Ok::<_, String>((owner, indent))
                    })
                    .transpose()?;
            }
            _ => return Err(format!("{key} cannot be set per buffer")),
        }
        Ok(())
    }

    /// Removes everything `plugin` put into the editor.
    pub fn remove_plugin_parts(&mut self, plugin: PluginId) {
        self.layers.retain(|&layer| layer != plugin);
        self.status.retain(|item| item.owner != plugin);
        self.panels.retain(|panel| panel.owner != plugin);
        self.popups.retain(|popup| popup.owner != plugin);
        for buffer in &mut self.buffers {
            buffer.remove_decorations(plugin);
        }
        self.commands.retain(|command| command.owner != plugin);
        self.timers.retain(|timer| timer.owner != plugin);
        self.processes.remove_owner(plugin);
        self.files.remove_owner(plugin);
        self.events.retain(|(target, _)| *target != Some(plugin));
    }

    /// Queues an event behind the buffer changes made so far, so events
    /// arrive in the order things happened.
    pub fn push_event(&mut self, target: Option<PluginId>, event: Event) {
        self.flush_changes();
        self.events.push_back((target, event));
    }

    pub fn pop_event(&mut self) -> Option<(Option<PluginId>, Event)> {
        self.flush_changes();
        self.events.pop_front()
    }

    /// Turns the changes buffers logged into `buffer-changed` events.
    fn flush_changes(&mut self) {
        for (index, buffer) in self.buffers.iter_mut().enumerate() {
            for (version, changes) in buffer.change_log.drain(..) {
                self.events.push_back((
                    None,
                    Event::BufferChanged {
                        buffer: index,
                        version,
                        changes,
                    },
                ));
            }
        }
    }

    /// The timers that are due, earliest first, removed from the list.
    pub fn take_due_timers(&mut self, now: Instant) -> Vec<Timer> {
        let (mut due, waiting): (Vec<Timer>, Vec<Timer>) =
            self.timers.drain(..).partition(|timer| timer.due <= now);
        self.timers = waiting;
        due.sort_by_key(|timer| (timer.due, timer.id));
        due
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
    /// From `plugins/<name>.toml`, by plugin name.
    plugin_configs: BTreeMap<String, PluginConfig>,
}

const LENT: &str = "editor state is only lent during plugin calls";

/// The commands the core runs itself, with their descriptions.
pub(crate) const CORE_COMMANDS: &[(&str, &str)] = &[
    ("buffer.save", "Save the current buffer"),
    ("buffer.open", "Open a file: {\"path\": string}"),
    ("buffer.next", "Show the next buffer"),
    ("buffer.previous", "Show the previous buffer"),
    (
        "editor.quit",
        "Quit; {\"force\": true} drops unsaved changes",
    ),
];

impl Default for Editor {
    /// Starts with an empty buffer that has no path.
    fn default() -> Self {
        let inbox = Arc::new(Inbox::default());
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
                popups: Vec::new(),
                last_popup_id: 0,
                events: VecDeque::new(),
                commands: Vec::new(),
                timers: Vec::new(),
                last_timer_id: 0,
                processes: Processes::new(inbox.clone()),
                files: FileJobs::new(inbox.clone()),
                clipboard: Box::new(clipboard::Internal::default()),
                inbox,
                hidden_views: HashMap::new(),
                languages: Languages::default(),
                theme: Theme::default(),
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
        self.state_mut().theme = config.theme;
        self.plugin_configs = config.plugins;
    }

    pub fn settings(&self) -> &Settings {
        &self.state().settings
    }

    /// The plugin's `plugins/<name>.toml`, or the defaults.
    pub fn plugin_config(&self, name: &str) -> PluginConfig {
        self.plugin_configs.get(name).cloned().unwrap_or_default()
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
        let delivered = self.deliver_events();
        let parsed = self.state_mut().update_syntax();
        if delivered {
            self.scroll_to_cursor();
        }
        delivered || parsed
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
        self.after_plugins_ran();
    }

    /// Gives plugins the system clipboard. Without it, they get one inside
    /// the editor.
    pub fn set_clipboard(&mut self, clipboard: Box<dyn Clipboard>) {
        self.state_mut().clipboard = clipboard;
    }

    /// Sets what background threads call after queueing work, such as a
    /// program's output, so the frontend can wake up and call
    /// `run_background`.
    pub fn set_waker(&mut self, waker: Option<Waker>) {
        self.state_mut().inbox.set_waker(waker);
    }

    /// Hands what background threads queued to the plugins. Returns whether
    /// there was anything.
    pub fn run_background(&mut self) -> bool {
        let messages = self.state().inbox.take();
        if messages.is_empty() {
            return false;
        }
        for message in messages {
            let state = self.state_mut();
            let (owner, event) = match message {
                Message::Output { id, stream, data } => (
                    state.processes.owner(id, false),
                    Event::ProcessOutput {
                        process: id,
                        stream,
                        data,
                    },
                ),
                Message::Exit { id, code } => (
                    state.processes.owner(id, true),
                    Event::ProcessExit { process: id, code },
                ),
                Message::Files { job, paths, done } => (
                    state.files.owner(job, done),
                    Event::FilesListed { job, paths, done },
                ),
            };
            // Messages of what was cancelled or stopped are dropped.
            if let Some(owner) = owner {
                state.push_event(Some(owner), event);
            }
        }
        self.after_plugins_ran();
        true
    }

    /// Delivers the events plugins caused, then brings the syntax tree and
    /// the scroll position up to date with what they did.
    pub(crate) fn after_plugins_ran(&mut self) {
        self.deliver_events();
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
        let column = layout::column_of(text, cursor, state.tab_width(state.view.buffer));
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
        let state = self.state_mut();
        for index in 0..state.buffers.len() {
            let buffer = &mut state.buffers[index];
            if !buffer.is_modified() {
                continue;
            }
            let Some(path) = buffer.path().map(|path| path.display().to_string()) else {
                failures.push("[scratch] has no path".to_string());
                continue;
            };
            match buffer.save() {
                Ok(()) => state.push_event(None, Event::BufferSaved(index)),
                Err(err) => failures.push(format!("{path}: {err}")),
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
