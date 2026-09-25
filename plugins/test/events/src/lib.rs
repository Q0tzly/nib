//! Test plugin for commands, events, and timers. It writes down the events
//! it gets, and its commands do what a test asks. Arguments are plain text,
//! not JSON, to keep the tests short.

use std::cell::RefCell;

use nib_plugin::exports::nib::plugin::guest::{Guest, KeyResult};
use nib_plugin::nib::plugin::events::{self, Event};
use nib_plugin::nib::plugin::types::{Edit, KeyEvent, UndoMode};
use nib_plugin::nib::plugin::{commands, editor, timers};

thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

const COMMANDS: [&str; 8] = [
    "echo", "call", "log", "emit", "timer", "cancel", "edit", "buffers",
];

struct Events;

impl Guest for Events {
    fn init(_config: String) -> Result<(), String> {
        for name in COMMANDS {
            commands::register(name, &format!("test: {name}"));
        }
        Ok(())
    }

    fn handle_key(_ev: KeyEvent) -> KeyResult {
        KeyResult::Pass
    }

    fn run_command(name: String, args: String) -> Result<String, String> {
        match name.as_str() {
            "echo" => Ok(args),
            // "<command> <args>": the result, marked ok or err.
            "call" => {
                let (command, args) = args.split_once(' ').unwrap_or((&args, ""));
                Ok(match commands::call(command, args) {
                    Ok(result) => format!("ok:{result}"),
                    Err(err) => format!("err:{err}"),
                })
            }
            "log" => Ok(LOG.with_borrow_mut(|log| std::mem::take(log).join("\n"))),
            "emit" => {
                events::emit(&args, "");
                Ok(String::new())
            }
            "timer" => {
                let ms = args.parse().map_err(|_| "timer needs milliseconds")?;
                Ok(timers::set(ms).to_string())
            }
            "cancel" => {
                timers::cancel(args.parse().map_err(|_| "cancel needs an id")?);
                Ok(String::new())
            }
            // Inserts the text at the start of the shown buffer.
            "edit" => {
                let view = editor::active_view();
                let edit = Edit {
                    start: 0,
                    end: 0,
                    text: args,
                };
                let version = view.buffer().version();
                view.apply(version, &[edit], None, UndoMode::NewStep)
                    .map_err(|err| format!("{err:?}"))?;
                Ok(String::new())
            }
            "buffers" => Ok(editor::buffers().len().to_string()),
            _ => Err(format!("no command {name}")),
        }
    }

    fn on_event(ev: Event) {
        let entry = match ev {
            Event::BufferOpened(buffer) => format!("opened {}", name(buffer.path())),
            Event::BufferSaved(buffer) => format!("saved {}", name(buffer.path())),
            Event::BufferChanged(change) => {
                let changes: Vec<String> = change
                    .changes
                    .iter()
                    .map(|c| {
                        format!(
                            "{}-{}@{}:{}-{}:{}={}",
                            c.start,
                            c.end,
                            c.start_line,
                            c.start_column,
                            c.end_line,
                            c.end_column,
                            c.text
                        )
                    })
                    .collect();
                format!("changed v{} {}", change.version, changes.join(";"))
            }
            Event::Custom(custom) => {
                // Answers itself forever, to check the host stops it.
                if custom.name == "test-events.loop" {
                    events::emit("loop", "");
                }
                format!("custom {} {}", custom.name, custom.data)
            }
            Event::Timer(id) => format!("timer {id}"),
        };
        LOG.with_borrow_mut(|log| log.push(entry));
    }
}

/// The file name of a buffer's path.
fn name(path: Option<String>) -> String {
    path.map_or("[scratch]".into(), |path| {
        path.rsplit(['/', '\\']).next().unwrap_or("").to_string()
    })
}

nib_plugin::export!(Events);
