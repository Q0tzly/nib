mod draw;
mod terminal;

use std::path::PathBuf;
use std::process::ExitCode;

use nib_core::Editor;

fn main() -> ExitCode {
    let mut editor = Editor::default();
    for path in std::env::args_os().skip(1).map(PathBuf::from) {
        if let Err(err) = editor.open(&path) {
            eprintln!("nib: {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
    }
    if let Err(err) = terminal::run(&mut editor) {
        eprintln!("nib: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
