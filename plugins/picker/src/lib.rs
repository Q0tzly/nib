//! Fuzzy file picker: `picker.files` lists the files of the working
//! directory with git, or rg outside a repository, and opens the chosen
//! one. While open, it takes the keys with its own input layer.

mod fuzzy;

use std::cell::RefCell;

use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::events::Event;
use nib_plugin::nib::plugin::process::{self, Child};
use nib_plugin::nib::plugin::types::{KeyCode, KeyEvent, Modifiers, Span};
use nib_plugin::nib::plugin::ui::{self, Panel};
use nib_plugin::nib::plugin::{commands, input};

/// Candidates shown at once.
const ROWS: usize = 10;
const PROMPT: &str = "files> ";

/// Where the file list comes from, in the order tried.
const LISTERS: [&[&str]; 2] = [
    &[
        "git",
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
    ],
    &["rg", "--files"],
];

struct Picker {
    panel: Panel,
    query: String,
    files: Vec<String>,
    /// Indices into `files`, best first.
    matches: Vec<usize>,
    selected: usize,
    /// Still listing: the program and what it printed so far.
    listing: Option<(Child, Vec<u8>)>,
    /// The lister in use, as an index into `LISTERS`.
    lister: usize,
}

thread_local! {
    static PICKER: RefCell<Option<Picker>> = const { RefCell::new(None) };
}

struct Plugin;

impl Guest for Plugin {
    fn init(_config: String) -> Result<(), String> {
        commands::register("files", "Pick a file to open");
        Ok(())
    }

    fn handle_key(ev: KeyEvent) -> KeyResult {
        PICKER.with_borrow_mut(|picker| {
            let Some(open) = picker else {
                return KeyResult::Pass;
            };
            match key(open, ev) {
                Action::Stay => open.show(),
                Action::Close => close(picker),
                Action::Open(path) => {
                    close(picker);
                    let args = format!("{{\"path\":{}}}", json_string(&path));
                    if let Err(err) = commands::call("buffer.open", &args) {
                        ui::show_message(&err);
                    }
                }
            }
            // It owns the keys while open.
            KeyResult::Handled
        })
    }

    fn run_command(name: String, _args: String) -> Result<String, String> {
        if name != "files" {
            return Err(format!("no command {name}"));
        }
        PICKER.with_borrow_mut(|picker| {
            if picker.is_some() {
                return Ok("null".into());
            }
            let child = list(0)?;
            input::push_layer();
            let open = picker.insert(Picker {
                panel: Panel::new(&[]),
                query: String::new(),
                files: Vec::new(),
                matches: Vec::new(),
                selected: 0,
                listing: Some((child, Vec::new())),
                lister: 0,
            });
            open.show();
            Ok("null".into())
        })
    }

    fn on_event(ev: Event) {
        PICKER.with_borrow_mut(|picker| {
            let Some(open) = picker else {
                return;
            };
            match ev {
                Event::ProcessOutput(output) => {
                    if let Some((child, data)) = &mut open.listing
                        && child.id() == output.process
                    {
                        data.extend(output.data);
                    }
                }
                Event::ProcessExit(exit) => {
                    let Some((child, data)) = open.listing.take_if(|(c, _)| c.id() == exit.process)
                    else {
                        return;
                    };
                    drop(child);
                    if exit.code == Some(0) {
                        open.files = String::from_utf8_lossy(&data)
                            .lines()
                            .map(String::from)
                            .collect();
                        open.filter();
                        open.show();
                        return;
                    }
                    // Not a repository, or no git: try the next lister.
                    let next = open.lister + 1;
                    match LISTERS.get(next).map(|_| list(next)) {
                        Some(Ok(child)) => {
                            open.lister = next;
                            open.listing = Some((child, Vec::new()));
                        }
                        _ => {
                            close(picker);
                            ui::show_message("listing files needs git or rg");
                        }
                    }
                }
                _ => {}
            }
        })
    }
}

enum Action {
    Stay,
    Close,
    Open(String),
}

fn key(picker: &mut Picker, ev: KeyEvent) -> Action {
    let ctrl = ev.modifiers == Modifiers::CTRL;
    match ev.code {
        KeyCode::Escape => return Action::Close,
        KeyCode::Enter => {
            return match picker.matches.get(picker.selected) {
                Some(&i) => Action::Open(picker.files[i].clone()),
                None => Action::Stay,
            };
        }
        KeyCode::Up => picker.select(-1),
        KeyCode::Down => picker.select(1),
        KeyCode::Char('p') if ctrl => picker.select(-1),
        KeyCode::Char('n') if ctrl => picker.select(1),
        KeyCode::Backspace => {
            picker.query.pop();
            picker.filter();
        }
        KeyCode::Char(c) if ev.modifiers.is_empty() => {
            picker.query.push(c);
            picker.filter();
        }
        _ => {}
    }
    Action::Stay
}

impl Picker {
    fn filter(&mut self) {
        let mut scored: Vec<(i64, usize)> = self
            .files
            .iter()
            .enumerate()
            .filter_map(|(i, path)| Some((fuzzy::score(&self.query, path)?, i)))
            .collect();
        // Best first; the list order breaks ties, so an empty query keeps it.
        scored.sort_by_key(|&(score, i)| (std::cmp::Reverse(score), i));
        self.matches = scored.into_iter().map(|(_, i)| i).collect();
        self.selected = 0;
    }

    fn select(&mut self, step: isize) {
        let count = self.matches.len().min(ROWS);
        if count > 0 {
            self.selected = (self.selected as isize + step).rem_euclid(count as isize) as usize;
        }
    }

    fn show(&self) {
        let status = if self.listing.is_some() {
            "  listing…".to_string()
        } else {
            format!("  {}/{}", self.matches.len(), self.files.len())
        };
        let mut lines = vec![vec![
            span(PROMPT, ""),
            span(&self.query, ""),
            span(&status, "comment"),
        ]];
        for (row, &i) in self.matches.iter().take(ROWS).enumerate() {
            let style = if row == self.selected {
                "ui.menu.selected"
            } else {
                ""
            };
            lines.push(vec![span(&format!(" {}", self.files[i]), style)]);
        }
        // A fixed height, so the text above does not jump while typing.
        lines.resize(ROWS + 1, Vec::new());
        self.panel.update(&lines);
        let cursor = (PROMPT.len() + self.query.len()) as u32;
        self.panel.set_cursor(Some((0, cursor)));
    }
}

/// Starts lister `index` in the working directory.
fn list(index: usize) -> Result<Child, String> {
    let (program, args) = LISTERS[index].split_first().expect("not empty");
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    process::spawn(program, &args, None)
}

/// Closes the picker: dropping it closes its panel and kills a listing
/// still running.
fn close(picker: &mut Option<Picker>) {
    *picker = None;
    input::pop_layer();
}

fn span(text: &str, style: &str) -> Span {
    Span {
        text: text.into(),
        style: style.into(),
    }
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

nib_plugin::export!(Plugin);
