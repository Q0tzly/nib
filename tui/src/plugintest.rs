//! `nib plugin test`: runs a plugin's tests, written in TOML, in an editor
//! without a terminal (docs/plugin-dev.md).

use std::fs;
use std::path::{Path, PathBuf};

use nib_core::{Editor, Grid, PluginSource, Selection, Symbol, marks, parse_keys, read_manifest};
use serde::Deserialize;

use crate::builtin;
use crate::settings;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestFile {
    /// The standard plugins to load; all but `lsp` when not given.
    with: Option<Vec<String>>,
    #[serde(default)]
    test: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    #[serde(default = "default_file")]
    file: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    keys: String,
    command: Option<String>,
    args: Option<String>,
    #[serde(default)]
    expect: Expect,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Expect {
    text: Option<String>,
    selections: Option<String>,
    message: Option<String>,
    screen: Option<Vec<String>>,
    result: Option<String>,
    error: Option<String>,
}

fn default_file() -> String {
    "test.txt".into()
}

/// The standard plugins tests load unless they say: all but `lsp`, which
/// starts language servers.
const LEFT_OUT: &[&str] = &["lsp"];

const SIZE: (u16, u16) = (80, 24);

/// Runs the tests in `files`, or in `dir/tests/*.toml`, against the plugin
/// in `dir`. Prints a line per test and fails if any did.
pub fn run(dir: &Path, files: &[PathBuf]) -> Result<(), String> {
    let manifest = read_manifest(dir).map_err(|err| err.to_string())?;
    if manifest.has_code && !dir.join("plugin.wasm").is_file() {
        return Err(format!(
            "{} has no plugin.wasm; build it first with `nib plugin build`",
            dir.display()
        ));
    }
    let files = match files {
        [] => test_files(dir)?,
        files => files.to_vec(),
    };
    let scratch = Scratch::new()?;
    let (mut passed, mut failed) = (0, 0);
    for file in &files {
        let shown = file.strip_prefix(dir).unwrap_or(file).display().to_string();
        let tests = match read(file) {
            Ok(tests) => tests,
            Err(err) => {
                println!("{shown}: {err}");
                failed += 1;
                continue;
            }
        };
        for case in &tests.test {
            let outcome = run_case(dir, &manifest.name, tests.with.as_deref(), case, &scratch.0);
            match outcome {
                Ok(()) => {
                    println!("test {shown}: {} ... ok", case.name);
                    passed += 1;
                }
                Err(problems) => {
                    println!("test {shown}: {} ... FAILED", case.name);
                    for problem in problems {
                        println!("    {}", problem.replace('\n', "\n    "));
                    }
                    failed += 1;
                }
            }
        }
    }
    println!("{passed} passed, {failed} failed");
    match failed {
        0 => Ok(()),
        _ => Err(format!("{failed} of {} tests failed", passed + failed)),
    }
}

fn test_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let tests = dir.join("tests");
    let mut files: Vec<PathBuf> = fs::read_dir(&tests)
        .map_err(|err| format!("{}: {err}", tests.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(format!("no tests in {}", tests.display()));
    }
    Ok(files)
}

fn read(file: &Path) -> Result<TestFile, String> {
    let text = fs::read_to_string(file).map_err(|err| err.to_string())?;
    let tests: TestFile = toml::from_str(&text).map_err(|err| err.to_string())?;
    if let Some(with) = &tests.with
        && let Some(unknown) = with
            .iter()
            .find(|name| !builtin::PLUGINS.iter().any(|(n, _, _)| n == name))
    {
        return Err(format!(
            "`with` names {unknown:?}, which is not a standard plugin"
        ));
    }
    Ok(tests)
}

/// Runs one test in a new editor, and says what did not come out as
/// expected.
fn run_case(
    dir: &Path,
    name: &str,
    with: Option<&[String]>,
    case: &Case,
    scratch: &Path,
) -> Result<(), Vec<String>> {
    let fail = |problem: String| vec![problem];
    let marked = marks::parse(&case.text).map_err(|err| fail(format!("text: {err}")))?;
    let keys = parse_keys(&case.keys).map_err(|err| fail(format!("keys: {err}")))?;
    let file_name = Path::new(&case.file)
        .file_name()
        .ok_or_else(|| fail(format!("file: {:?} is not a file name", case.file)))?;
    let path = scratch.join(file_name);
    fs::write(&path, &marked.text).map_err(|err| fail(format!("{}: {err}", path.display())))?;

    let mut editor = Editor::default();
    editor.set_plugin_cache_dir(settings::cache_dir());
    editor.resize(SIZE.0, SIZE.1);
    // Opened before the plugins load, as when nib starts with a file.
    editor
        .open(&path)
        .map_err(|err| fail(format!("{}: {err}", path.display())))?;
    let standard = builtin::PLUGINS.iter().filter(|(n, _, _)| {
        *n != name
            && match with {
                Some(with) => with.iter().any(|w| w == n),
                None => !LEFT_OUT.contains(n),
            }
    });
    let mut sources: Vec<_> = standard
        .map(|&(_, manifest, files)| PluginSource::Bytes { manifest, files })
        .collect();
    sources.push(PluginSource::Dir(dir));
    let failures: Vec<String> = editor
        .load_plugins(&sources)
        .into_iter()
        .filter_map(Result::err)
        .map(|err| format!("loading: {err}"))
        .collect();
    if !failures.is_empty() {
        return Err(failures);
    }
    if !marked.ranges.is_empty() {
        let selection = Selection::new(marked.ranges, marked.primary, editor.buffer().text())
            .map_err(|err| fail(format!("text: the selection does not fit: {err}")))?;
        editor.view_mut().selection = selection;
    }
    settle(&mut editor);
    for key in keys {
        editor.handle_key(key);
        settle(&mut editor);
    }
    let result = case.command.as_ref().map(|command| {
        let result = editor.call_command(command, case.args.as_deref().unwrap_or("{}"));
        settle(&mut editor);
        result
    });
    check(&editor, &case.expect, result)
}

/// Does what the terminal loop does after each frame.
fn settle(editor: &mut Editor) {
    while editor.catch_up() {}
}

fn check(
    editor: &Editor,
    expect: &Expect,
    result: Option<Result<String, String>>,
) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();
    let mut differ = |what: &str, expected: &str, actual: &str| {
        problems.push(format!(
            "{what}:\n  expected: {expected:?}\n  actual:   {actual:?}"
        ));
    };
    let text = editor.buffer().text().to_string();
    if let Some(expected) = &expect.text
        && *expected != text
    {
        differ("text", expected, &text);
    }
    if let Some(expected) = &expect.selections {
        let actual = marks::show(&text, &editor.view().selection);
        if *expected != actual {
            differ("selections", expected, &actual);
        }
    }
    if let Some(expected) = &expect.message {
        let actual = editor.message().unwrap_or_default();
        if expected != actual {
            differ("message", expected, actual);
        }
    }
    if let Some(expected) = &expect.screen {
        let rows = screen(editor);
        for line in expected {
            if !rows.iter().any(|row| row.contains(line.as_str())) {
                differ("screen", line, &rows.join("\n"));
            }
        }
    }
    match (result, &expect.result, &expect.error) {
        (Some(Ok(actual)), Some(expected), _) => {
            if !same_json(expected, &actual) {
                differ("result", expected, &actual);
            }
        }
        (Some(Err(actual)), _, Some(expected)) => {
            if !actual.contains(expected.as_str()) {
                differ("error", expected, &actual);
            }
        }
        (Some(Ok(actual)), None, Some(expected)) => {
            differ(
                "error",
                expected,
                &format!("no error; the result was {actual}"),
            );
        }
        (Some(Err(actual)), _, None) => problems.push(format!("command failed: {actual}")),
        (None, Some(_), _) | (None, _, Some(_)) => {
            problems.push("`result` and `error` need a `command` to call".into());
        }
        _ => {}
    }
    match problems.is_empty() {
        true => Ok(()),
        false => Err(problems),
    }
}

/// Compares as JSON if both are, so `{"a": 1}` matches `{"a":1}`.
fn same_json(expected: &str, actual: &str) -> bool {
    match (
        serde_json::from_str::<serde_json::Value>(expected),
        serde_json::from_str::<serde_json::Value>(actual),
    ) {
        (Ok(expected), Ok(actual)) => expected == actual,
        _ => expected == actual,
    }
}

fn screen(editor: &Editor) -> Vec<String> {
    let mut grid = Grid::default();
    editor.render(&mut grid);
    (0..grid.height())
        .map(|y| {
            grid.row(y)
                .iter()
                .filter_map(|cell| match &cell.symbol {
                    Symbol::Char(c) => Some(c.to_string()),
                    Symbol::Str(s) => Some(s.to_string()),
                    Symbol::Continuation => None,
                })
                .collect()
        })
        .collect()
}

/// A directory for the tests' files, removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!("nib-plugin-test-{}", std::process::id()));
        fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
