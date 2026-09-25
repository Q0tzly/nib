//! Core of the nib editor: buffers, selections, rendering, and the plugin host.

pub mod buffer;
pub mod change;
pub mod config;
pub mod editor;
mod error;
pub mod grapheme;
pub mod grid;
mod history;
pub mod input;
mod layout;
mod plugin;
mod render;
pub mod search;
pub mod selection;
pub mod ui;
pub mod view;

pub use buffer::{Buffer, Change, LineEnding};
pub use change::{Assoc, ChangeSet, Edit};
pub use config::{Config, Indent, Settings};
pub use editor::{Editor, Menu};
pub use error::Error;
pub use grid::{Cell, Color, Cursor, CursorShape, Grid, Style, Symbol};
pub use history::UndoMode;
pub use input::{KeyCode, KeyEvent, Modifiers};
pub use plugin::{API_VERSION, PluginInfo, PluginOptions};
pub use selection::{Range, Selection};
pub use ui::{Side, Span, StyledLine};
pub use view::View;
