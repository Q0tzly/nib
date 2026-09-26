//! Core of the nib editor: buffers, selections, rendering, and the plugin host.

mod background;
pub mod buffer;
pub mod change;
pub mod config;
pub mod editor;
mod error;
mod events;
mod files;
pub mod grapheme;
pub mod grid;
mod history;
pub mod input;
mod layout;
mod plugin;
mod process;
mod render;
pub mod search;
pub mod selection;
mod syntax;
pub mod ui;
pub mod view;

pub use background::Waker;
pub use buffer::{Buffer, Change, LineEnding};
pub use change::{Assoc, ChangeSet, Edit};
pub use config::{Config, Indent, Load, PluginConfig, Settings, Timeout};
pub use editor::{Editor, Menu, ScrollAmount};
pub use error::Error;
pub use events::TextChange;
pub use grid::{Cell, Color, Cursor, CursorShape, Grid, Style, Symbol};
pub use history::UndoMode;
pub use input::{KeyCode, KeyEvent, Modifiers};
pub use plugin::{API_VERSION, Interrupter, PluginInfo, PluginOptions, plugin_name};
pub use process::Stream;
pub use selection::{Range, Selection};
pub use ui::{Side, Span, StyledLine, Theme};
pub use view::View;
