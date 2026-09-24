//! Core of the nib editor: buffers, selections, rendering, and the plugin host.

pub mod buffer;
pub mod change;
mod error;
pub mod grapheme;
mod history;
pub mod selection;

pub use buffer::{Buffer, Change, LineEnding};
pub use change::{Assoc, ChangeSet, Edit};
pub use error::Error;
pub use history::UndoMode;
pub use selection::{Range, Selection};
