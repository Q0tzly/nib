//! Host side of `nib:plugin`: what plugins can call.

use wasmtime::component::Resource;
use wasmtime_wasi::{WasiCtxView, WasiView};

use super::PluginData;
use crate::buffer::Buffer;
use crate::editor::{ScrollAmount, State};
use crate::grapheme;
use crate::grid::CursorShape;
use crate::history::UndoMode;
use crate::input::{KeyCode, KeyEvent};
use crate::layout;
use crate::selection::{Range, Selection};
use crate::ui::{Panel, Side, Span, StatusItem, StyledLine};
use crate::{Edit, Error};

pub(crate) mod bindings {
    wasmtime::component::bindgen!({
        path: "../api/wit",
        world: "plugin",
        // Host functions trap when a plugin misuses the API, e.g. with a
        // handle to a buffer that no longer exists.
        imports: { default: trappable },
        with: {
            "nib:plugin/editor.buffer": super::BufferHandle,
            "nib:plugin/editor.view": super::ViewHandle,
            "nib:plugin/ui.panel": super::PanelHandle,
        },
    });
}

use bindings::nib::plugin::{commands, editor, input, settings, types as wit, ui as wit_ui};

/// A buffer as seen by a plugin. The resource's rep is the buffer index.
pub struct BufferHandle;

/// A view as seen by a plugin. There is one view for now, with rep 0.
pub struct ViewHandle;

/// A panel as seen by a plugin. The resource's rep is the panel id.
pub struct PanelHandle;

type HostResult<T> = wasmtime::Result<T>;

impl WasiView for PluginData {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl PluginData {
    fn state(&mut self) -> HostResult<&mut State> {
        self.state
            .as_mut()
            .ok_or_else(|| wasmtime::Error::msg("the editor is only reachable during a call"))
    }

    fn buffer(&mut self, handle: &Resource<BufferHandle>) -> HostResult<&mut Buffer> {
        self.state()?
            .buffers
            .get_mut(handle.rep() as usize)
            .ok_or_else(|| wasmtime::Error::msg("the buffer no longer exists"))
    }

    fn view_state(&mut self, handle: &Resource<ViewHandle>) -> HostResult<&mut State> {
        if handle.rep() != 0 {
            return Err(wasmtime::Error::msg("the view no longer exists"));
        }
        self.state()
    }
}

impl wit::Host for PluginData {}

impl editor::Host for PluginData {
    fn active_view(&mut self) -> HostResult<Resource<ViewHandle>> {
        Ok(Resource::new_own(0))
    }
}

impl editor::HostBuffer for PluginData {
    fn version(&mut self, buffer: Resource<BufferHandle>) -> HostResult<u64> {
        Ok(self.buffer(&buffer)?.version())
    }

    fn len(&mut self, buffer: Resource<BufferHandle>) -> HostResult<u64> {
        Ok(self.buffer(&buffer)?.len() as u64)
    }

    fn slice(
        &mut self,
        buffer: Resource<BufferHandle>,
        start: u64,
        end: u64,
    ) -> HostResult<Result<String, editor::Error>> {
        let buffer = self.buffer(&buffer)?;
        wit_result(pos(start).and_then(|start| buffer.slice(start, pos(end)?)))
    }

    fn line_count(&mut self, buffer: Resource<BufferHandle>) -> HostResult<u64> {
        Ok(self.buffer(&buffer)?.line_count() as u64)
    }

    fn line_start(&mut self, buffer: Resource<BufferHandle>, line: u64) -> HostResult<Option<u64>> {
        let buffer = self.buffer(&buffer)?;
        Ok(usize::try_from(line)
            .ok()
            .and_then(|line| buffer.line_start(line))
            .map(|pos| pos as u64))
    }

    fn line_of(
        &mut self,
        buffer: Resource<BufferHandle>,
        at: u64,
    ) -> HostResult<Result<u64, editor::Error>> {
        let buffer = self.buffer(&buffer)?;
        wit_result(
            pos(at)
                .and_then(|at| buffer.line_of(at))
                .map(|line| line as u64),
        )
    }

    fn next_grapheme(
        &mut self,
        buffer: Resource<BufferHandle>,
        at: u64,
    ) -> HostResult<Result<u64, editor::Error>> {
        let buffer = self.buffer(&buffer)?;
        wit_result(
            pos(at)
                .and_then(|at| buffer.next_grapheme(at))
                .map(|p| p as u64),
        )
    }

    fn prev_grapheme(
        &mut self,
        buffer: Resource<BufferHandle>,
        at: u64,
    ) -> HostResult<Result<u64, editor::Error>> {
        let buffer = self.buffer(&buffer)?;
        wit_result(
            pos(at)
                .and_then(|at| buffer.prev_grapheme(at))
                .map(|p| p as u64),
        )
    }

    fn path(&mut self, buffer: Resource<BufferHandle>) -> HostResult<Option<String>> {
        let buffer = self.buffer(&buffer)?;
        Ok(buffer
            .path()
            .map(|path| path.to_string_lossy().into_owned()))
    }

    fn find(
        &mut self,
        buffer: Resource<BufferHandle>,
        pattern: String,
        start: u64,
        backward: bool,
    ) -> HostResult<Result<Option<(u64, u64)>, editor::Error>> {
        let buffer = self.buffer(&buffer)?;
        wit_result(
            pos(start)
                .and_then(|start| buffer.find(&pattern, start, backward))
                .map(|found| found.map(|(s, e)| (s as u64, e as u64))),
        )
    }

    fn find_all(
        &mut self,
        buffer: Resource<BufferHandle>,
        pattern: String,
        start: u64,
        end: u64,
    ) -> HostResult<Result<Vec<(u64, u64)>, editor::Error>> {
        let buffer = self.buffer(&buffer)?;
        wit_result(
            pos(start)
                .and_then(|start| buffer.find_all(&pattern, start, pos(end)?))
                .map(|found| {
                    found
                        .into_iter()
                        .map(|(s, e)| (s as u64, e as u64))
                        .collect()
                }),
        )
    }

    fn drop(&mut self, _buffer: Resource<BufferHandle>) -> HostResult<()> {
        Ok(())
    }
}

impl editor::HostView for PluginData {
    fn buffer(&mut self, view: Resource<ViewHandle>) -> HostResult<Resource<BufferHandle>> {
        let state = self.view_state(&view)?;
        Ok(Resource::new_own(state.view.buffer as u32))
    }

    fn selection(&mut self, view: Resource<ViewHandle>) -> HostResult<wit::Selection> {
        let state = self.view_state(&view)?;
        Ok(to_wit_selection(&state.view.selection))
    }

    fn set_selection(
        &mut self,
        view: Resource<ViewHandle>,
        selection: wit::Selection,
    ) -> HostResult<Result<(), editor::Error>> {
        let state = self.view_state(&view)?;
        let buffer = &state.buffers[state.view.buffer];
        let result = from_wit_selection(selection)
            .and_then(|(ranges, primary)| Selection::new(ranges, primary, buffer.text()));
        if let Ok(selection) = &result {
            state.view.selection = selection.clone();
        }
        wit_result(result.map(|_| ()))
    }

    fn apply(
        &mut self,
        view: Resource<ViewHandle>,
        base_version: u64,
        edits: Vec<wit::Edit>,
        after: Option<wit::Selection>,
        undo: wit::UndoMode,
    ) -> HostResult<Result<(), editor::Error>> {
        let state = self.view_state(&view)?;
        let buffer = &mut state.buffers[state.view.buffer];
        let result = (|| {
            let edits = edits
                .into_iter()
                .map(|edit| Ok(Edit::new(pos(edit.start)?, pos(edit.end)?, edit.text)))
                .collect::<Result<Vec<_>, Error>>()?;
            let after = after.map(from_wit_selection).transpose()?;
            let mode = match undo {
                wit::UndoMode::NewStep => UndoMode::NewStep,
                wit::UndoMode::Merge => UndoMode::Merge,
            };
            buffer.apply(base_version, edits, &state.view.selection, after, mode)
        })();
        let result = result.map(|change| state.view.selection = change.selection);
        wit_result(result)
    }

    fn undo(&mut self, view: Resource<ViewHandle>) -> HostResult<bool> {
        let state = self.view_state(&view)?;
        let change = state.buffers[state.view.buffer].undo();
        Ok(change
            .map(|change| state.view.selection = change.selection)
            .is_some())
    }

    fn redo(&mut self, view: Resource<ViewHandle>) -> HostResult<bool> {
        let state = self.view_state(&view)?;
        let change = state.buffers[state.view.buffer].redo();
        Ok(change
            .map(|change| state.view.selection = change.selection)
            .is_some())
    }

    fn move_vertically(
        &mut self,
        view: Resource<ViewHandle>,
        at: u64,
        lines: i32,
        column: Option<u32>,
    ) -> HostResult<Result<(u64, u32), editor::Error>> {
        let state = self.view_state(&view)?;
        let tab_width = state.settings.tab_width;
        let text = state.buffers[state.view.buffer].text();
        wit_result(
            pos(at)
                .and_then(|at| {
                    grapheme::check_position(text, at)?;
                    Ok(layout::move_vertically(text, at, lines, column, tab_width))
                })
                .map(|(at, column)| (at as u64, column)),
        )
    }

    fn scroll(
        &mut self,
        view: Resource<ViewHandle>,
        amount: editor::ScrollAmount,
    ) -> HostResult<i32> {
        let state = self.view_state(&view)?;
        Ok(state.scroll(match amount {
            editor::ScrollAmount::Lines(n) => ScrollAmount::Lines(n),
            editor::ScrollAmount::HalfPage(n) => ScrollAmount::HalfPage(n),
            editor::ScrollAmount::Page(n) => ScrollAmount::Page(n),
        }))
    }

    fn set_cursor_shape(
        &mut self,
        view: Resource<ViewHandle>,
        shape: wit::CursorShape,
    ) -> HostResult<()> {
        let state = self.view_state(&view)?;
        state.view.cursor_shape = match shape {
            wit::CursorShape::Block => CursorShape::Block,
            wit::CursorShape::Bar => CursorShape::Bar,
            wit::CursorShape::Underline => CursorShape::Underline,
        };
        Ok(())
    }

    fn drop(&mut self, _view: Resource<ViewHandle>) -> HostResult<()> {
        Ok(())
    }
}

impl input::Host for PluginData {
    fn push_layer(&mut self) -> HostResult<()> {
        let plugin = self.plugin;
        self.state()?.layers.push(plugin);
        Ok(())
    }

    fn pop_layer(&mut self) -> HostResult<()> {
        let plugin = self.plugin;
        let layers = &mut self.state()?.layers;
        if let Some(i) = layers.iter().rposition(|&layer| layer == plugin) {
            layers.remove(i);
        }
        Ok(())
    }
}

impl commands::Host for PluginData {
    fn call(&mut self, name: String, args: String) -> HostResult<Result<String, String>> {
        Ok(self.state()?.run_command(&name, &args))
    }
}

impl settings::Host for PluginData {
    fn get(&mut self, key: String) -> HostResult<Option<String>> {
        Ok(self.state()?.settings.get_json(&key))
    }
}

impl wit_ui::Host for PluginData {
    fn set_status(
        &mut self,
        id: String,
        side: wit_ui::Side,
        priority: i32,
        content: Vec<wit::Span>,
    ) -> HostResult<()> {
        let owner = self.plugin;
        let status = &mut self.state()?.status;
        status.retain(|item| !(item.owner == owner && item.id == id));
        status.push(StatusItem {
            owner,
            id,
            side: match side {
                wit_ui::Side::Left => Side::Left,
                wit_ui::Side::Right => Side::Right,
            },
            priority,
            content: styled_line(content),
        });
        Ok(())
    }

    fn remove_status(&mut self, id: String) -> HostResult<()> {
        let owner = self.plugin;
        self.state()?
            .status
            .retain(|item| !(item.owner == owner && item.id == id));
        Ok(())
    }

    fn show_message(&mut self, text: String) -> HostResult<()> {
        self.state()?.message = Some(text);
        Ok(())
    }
}

impl wit_ui::HostPanel for PluginData {
    fn new(&mut self, lines: Vec<Vec<wit::Span>>) -> HostResult<Resource<PanelHandle>> {
        let owner = self.plugin;
        let state = self.state()?;
        state.last_panel_id += 1;
        let id = state.last_panel_id;
        state.panels.push(Panel {
            id,
            owner,
            lines: lines.into_iter().map(styled_line).collect(),
            cursor: None,
        });
        Ok(Resource::new_own(id))
    }

    fn update(
        &mut self,
        panel: Resource<PanelHandle>,
        lines: Vec<Vec<wit::Span>>,
    ) -> HostResult<()> {
        self.panel(&panel)?.lines = lines.into_iter().map(styled_line).collect();
        Ok(())
    }

    fn set_cursor(
        &mut self,
        panel: Resource<PanelHandle>,
        cursor: Option<(u32, u32)>,
    ) -> HostResult<()> {
        self.panel(&panel)?.cursor = cursor;
        Ok(())
    }

    fn drop(&mut self, panel: Resource<PanelHandle>) -> HostResult<()> {
        // Outside a call, the plugin is being stopped and its panels are
        // removed anyway.
        if let Some(state) = self.state.as_mut() {
            state.panels.retain(|p| p.id != panel.rep());
        }
        Ok(())
    }
}

impl PluginData {
    fn panel(&mut self, handle: &Resource<PanelHandle>) -> HostResult<&mut Panel> {
        self.state()?
            .panels
            .iter_mut()
            .find(|panel| panel.id == handle.rep())
            .ok_or_else(|| wasmtime::Error::msg("the panel is closed"))
    }
}

fn styled_line(spans: Vec<wit::Span>) -> StyledLine {
    spans
        .into_iter()
        .map(|span| Span {
            text: span.text,
            style: span.style,
        })
        .collect()
}

pub(crate) fn key_event(key: KeyEvent) -> wit::KeyEvent {
    let code = match key.code {
        KeyCode::Char(c) => wit::KeyCode::Char(c),
        KeyCode::Enter => wit::KeyCode::Enter,
        KeyCode::Escape => wit::KeyCode::Escape,
        KeyCode::Tab => wit::KeyCode::Tab,
        KeyCode::Backspace => wit::KeyCode::Backspace,
        KeyCode::Delete => wit::KeyCode::Delete,
        KeyCode::Up => wit::KeyCode::Up,
        KeyCode::Down => wit::KeyCode::Down,
        KeyCode::Left => wit::KeyCode::Left,
        KeyCode::Right => wit::KeyCode::Right,
        KeyCode::Home => wit::KeyCode::Home,
        KeyCode::End => wit::KeyCode::End,
        KeyCode::PageUp => wit::KeyCode::PageUp,
        KeyCode::PageDown => wit::KeyCode::PageDown,
        KeyCode::F(n) => wit::KeyCode::F(n),
    };
    let mut modifiers = wit::Modifiers::empty();
    for (on, flag) in [
        (key.modifiers.ctrl, wit::Modifiers::CTRL),
        (key.modifiers.alt, wit::Modifiers::ALT),
        (key.modifiers.shift, wit::Modifiers::SHIFT),
        (key.modifiers.super_, wit::Modifiers::SUPER),
    ] {
        if on {
            modifiers |= flag;
        }
    }
    wit::KeyEvent { code, modifiers }
}

fn pos(offset: u64) -> Result<usize, Error> {
    usize::try_from(offset).map_err(|_| Error::InvalidPosition(usize::MAX))
}

fn to_wit_selection(selection: &Selection) -> wit::Selection {
    wit::Selection {
        ranges: selection
            .ranges()
            .iter()
            .map(|range| wit::SelRange {
                anchor: range.anchor as u64,
                head: range.head as u64,
            })
            .collect(),
        primary: selection.primary_index() as u32,
    }
}

fn from_wit_selection(selection: wit::Selection) -> Result<(Vec<Range>, usize), Error> {
    let ranges = selection
        .ranges
        .into_iter()
        .map(|range| Ok(Range::new(pos(range.anchor)?, pos(range.head)?)))
        .collect::<Result<_, Error>>()?;
    Ok((ranges, selection.primary as usize))
}

/// Errors a plugin can act on become WIT errors. Anything else is a bug in
/// the host and traps.
fn wit_result<T>(result: Result<T, Error>) -> HostResult<Result<T, editor::Error>> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(err) => Ok(Err(match err {
            Error::StaleVersion { .. } => editor::Error::StaleVersion,
            Error::InvalidPosition(_) => editor::Error::InvalidPosition,
            Error::OverlappingEdits => editor::Error::OverlappingEdits,
            Error::InvalidSelection => editor::Error::InvalidSelection,
            Error::InvalidPattern(message) => editor::Error::InvalidPattern(message),
            other => return Err(wasmtime::Error::msg(other.to_string())),
        })),
    }
}
