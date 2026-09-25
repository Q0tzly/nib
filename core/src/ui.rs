//! UI parts that plugins put on screen: status line items and panels. The
//! core lays them out; plugins never see screen coordinates.

use crate::grid::{Color, Style};
use crate::plugin::PluginId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    /// A theme entry, e.g. "ui.mode.normal".
    pub style: String,
}

pub type StyledLine = Vec<Span>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Debug)]
pub(crate) struct StatusItem {
    pub owner: PluginId,
    pub id: String,
    pub side: Side,
    pub priority: i32,
    pub content: StyledLine,
}

#[derive(Clone, Debug)]
pub(crate) struct Panel {
    pub id: u32,
    pub owner: PluginId,
    pub lines: Vec<StyledLine>,
    /// A line index and a byte offset into that line's text.
    pub cursor: Option<(u32, u32)>,
}

/// The built-in theme. Unknown names give `None`, and the caller keeps the
/// style of the surrounding area.
pub fn theme_style(name: &str) -> Option<Style> {
    let mode = |bg| Style {
        fg: Color::Indexed(0),
        bg: Color::Indexed(bg),
        bold: true,
        ..Style::default()
    };
    match name {
        "ui.mode.normal" => Some(mode(4)),
        "ui.mode.insert" => Some(mode(2)),
        "ui.mode.select" => Some(mode(5)),
        "ui.selection" => Some(Style {
            bg: Color::Indexed(8),
            ..Style::default()
        }),
        "ui.menu.selected" => Some(Style {
            reverse: true,
            ..Style::default()
        }),
        "ui.error" => Some(Style {
            fg: Color::Indexed(1),
            bold: true,
            ..Style::default()
        }),
        _ => None,
    }
}
