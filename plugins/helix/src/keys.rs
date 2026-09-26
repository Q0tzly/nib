//! Key remapping from `[settings.keys.*]` in plugins/helix.toml, written as
//! in Helix's config. Helix command names stand for the keys that do the
//! same here, so a remapped key replays them.

use nib_plugin::nib::plugin::types::{KeyCode, KeyEvent, Modifiers};
use serde_json::Value;

/// Helix command names, and the keys that run them in this keymap.
const COMMANDS: &[(&str, &str)] = &[
    ("move_char_left", "h"),
    ("move_char_right", "l"),
    ("move_visual_line_down", "j"),
    ("move_visual_line_up", "k"),
    ("move_line_down", "j"),
    ("move_line_up", "k"),
    ("move_next_word_start", "w"),
    ("move_prev_word_start", "b"),
    ("move_next_word_end", "e"),
    ("move_next_long_word_start", "W"),
    ("move_prev_long_word_start", "B"),
    ("move_next_long_word_end", "E"),
    ("find_next_char", "f"),
    ("find_till_char", "t"),
    ("find_prev_char", "F"),
    ("till_prev_char", "T"),
    ("replace", "r"),
    ("goto_file_start", "g g"),
    ("goto_last_line", "g e"),
    ("goto_line_start", "g h"),
    ("goto_line_end", "g l"),
    ("goto_first_nonwhitespace", "g s"),
    ("goto_definition", "g d"),
    ("goto_next_buffer", "g n"),
    ("goto_previous_buffer", "g p"),
    ("page_down", "C-f"),
    ("page_up", "C-b"),
    ("half_page_down", "C-d"),
    ("half_page_up", "C-u"),
    ("extend_line_below", "x"),
    ("select_all", "%"),
    ("collapse_selection", ";"),
    ("keep_primary_selection", ","),
    ("flip_selections", "A-;"),
    ("copy_selection_on_next_line", "C"),
    ("select_regex", "s"),
    ("select_mode", "v"),
    ("insert_mode", "i"),
    ("append_mode", "a"),
    ("insert_at_line_start", "I"),
    ("insert_at_line_end", "A"),
    ("open_below", "o"),
    ("open_above", "O"),
    ("yank", "y"),
    ("delete_selection", "d"),
    ("change_selection", "c"),
    ("paste_after", "p"),
    ("paste_before", "P"),
    ("indent", ">"),
    ("unindent", "<"),
    ("join_selections", "J"),
    ("undo", "u"),
    ("redo", "U"),
    ("command_mode", ":"),
    ("search", "/"),
    ("rsearch", "?"),
    ("search_next", "n"),
    ("search_prev", "N"),
    ("search_selection", "*"),
    ("match_brackets", "m m"),
    ("select_textobject_inner", "m i"),
    ("select_textobject_around", "m a"),
    ("expand_selection", "A-o"),
    ("shrink_selection", "A-i"),
    ("select_next_sibling", "A-n"),
    ("select_prev_sibling", "A-p"),
    ("goto_next_function", "] f"),
    ("goto_prev_function", "[ f"),
    ("goto_next_class", "] t"),
    ("goto_prev_class", "[ t"),
    ("goto_next_parameter", "] a"),
    ("goto_prev_parameter", "[ a"),
    ("goto_next_comment", "] c"),
    ("goto_prev_comment", "[ c"),
    ("goto_next_test", "] T"),
    ("goto_prev_test", "[ T"),
    ("file_picker", "space f"),
    ("hover", "space k"),
    ("vsplit", "C-w v"),
    ("hsplit", "C-w s"),
    ("rotate_view", "C-w w"),
    ("jump_view_left", "C-w h"),
    ("jump_view_down", "C-w j"),
    ("jump_view_up", "C-w k"),
    ("jump_view_right", "C-w l"),
    ("wclose", "C-w q"),
    ("wonly", "C-w o"),
    ("yank_to_clipboard", "space y"),
    ("paste_clipboard_after", "space p"),
    ("paste_clipboard_before", "space P"),
    ("normal_mode", "esc"),
    ("completion", "C-x"),
    ("insert_newline", "ret"),
    ("delete_char_backward", "backspace"),
    ("delete_char_forward", "del"),
    ("insert_tab", "tab"),
    ("no_op", ""),
];

#[derive(Clone)]
pub enum Binding {
    /// A command of the core or another plugin, by its dotted name.
    Command(String),
    /// The keys that run a Helix command here.
    Keys(Vec<KeyEvent>),
    /// A table of the keys that may follow.
    Prefix(Keymap),
}

pub type Keymap = Vec<(KeyEvent, Binding)>;

#[derive(Default)]
pub struct Keymaps {
    pub normal: Keymap,
    pub insert: Keymap,
    pub select: Keymap,
}

/// The keymaps in `settings`, and what was wrong in them. Wrong entries are
/// left out, so the rest still works.
pub fn keymaps(settings: &Value) -> (Keymaps, Vec<String>) {
    let mut errors = Vec::new();
    let mut table = |mode: &str| {
        let mut keymap = Keymap::new();
        if let Some(entries) = settings["keys"][mode].as_object() {
            parse_table(entries, &format!("keys.{mode}"), &mut keymap, &mut errors);
        }
        keymap
    };
    let keymaps = Keymaps {
        normal: table("normal"),
        insert: table("insert"),
        select: table("select"),
    };
    (keymaps, errors)
}

fn parse_table(
    entries: &serde_json::Map<String, Value>,
    at: &str,
    keymap: &mut Keymap,
    errors: &mut Vec<String>,
) {
    for (key, value) in entries {
        let place = format!("{at}.{key}");
        let key = match parse_key(key) {
            Ok(key) => key,
            Err(err) => {
                errors.push(format!("{place}: {err}"));
                continue;
            }
        };
        let binding = match value {
            Value::String(name) if name.contains('.') => Binding::Command(name.clone()),
            Value::String(name) => match command_keys(name) {
                Some(keys) => Binding::Keys(keys),
                None => {
                    errors.push(format!("{place}: unknown command {name:?}"));
                    continue;
                }
            },
            Value::Object(entries) => {
                let mut prefix = Keymap::new();
                parse_table(entries, &place, &mut prefix, errors);
                Binding::Prefix(prefix)
            }
            _ => {
                errors.push(format!("{place}: expected a command name or a table"));
                continue;
            }
        };
        keymap.push((key, binding));
    }
}

/// The keys that run Helix command `name` here.
fn command_keys(name: &str) -> Option<Vec<KeyEvent>> {
    let (_, keys) = COMMANDS.iter().find(|(command, _)| *command == name)?;
    Some(
        keys.split_whitespace()
            .map(|key| parse_key(key).expect("the table's keys parse"))
            .collect(),
    )
}

pub fn lookup<'a>(keymap: &'a Keymap, key: &KeyEvent) -> Option<&'a Binding> {
    keymap.iter().find(|(k, _)| k == key).map(|(_, b)| b)
}

/// A key in Helix's notation: a char, or a name such as `ret`, with `C-`,
/// `A-`, and `S-` in front for modifiers.
pub fn parse_key(text: &str) -> Result<KeyEvent, String> {
    let mut modifiers = Modifiers::empty();
    let mut rest = text;
    loop {
        let (modifier, tail) = match rest.split_once('-') {
            Some((m @ ("C" | "A" | "S"), tail)) if !tail.is_empty() => (m, tail),
            _ => break,
        };
        modifiers |= match modifier {
            "C" => Modifiers::CTRL,
            "A" => Modifiers::ALT,
            _ => Modifiers::SHIFT,
        };
        rest = tail;
    }
    let mut chars = rest.chars();
    let code = match (chars.next(), chars.next()) {
        (Some(c), None) => KeyCode::Char(c),
        _ => match rest {
            "ret" | "enter" => KeyCode::Enter,
            "esc" => KeyCode::Escape,
            "space" => KeyCode::Char(' '),
            "tab" => KeyCode::Tab,
            "backspace" => KeyCode::Backspace,
            "del" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "minus" => KeyCode::Char('-'),
            f if f.starts_with('F') => KeyCode::F(
                f[1..]
                    .parse()
                    .map_err(|_| format!("unknown key {text:?}"))?,
            ),
            _ => return Err(format!("unknown key {text:?}")),
        },
    };
    // A char already says whether it is shifted, as the core sends it.
    if let KeyCode::Char(c) = code
        && modifiers.contains(Modifiers::SHIFT)
    {
        modifiers.remove(Modifiers::SHIFT);
        let upper: Vec<char> = c.to_uppercase().collect();
        if let [upper] = upper[..] {
            return Ok(KeyEvent {
                code: KeyCode::Char(upper),
                modifiers,
            });
        }
    }
    Ok(KeyEvent { code, modifiers })
}

/// A key as Helix writes it, for hints.
pub fn label(key: &KeyEvent) -> String {
    let mut label = String::new();
    for (flag, prefix) in [
        (Modifiers::CTRL, "C-"),
        (Modifiers::ALT, "A-"),
        (Modifiers::SHIFT, "S-"),
    ] {
        if key.modifiers.contains(flag) {
            label.push_str(prefix);
        }
    }
    match key.code {
        KeyCode::Char(' ') => label.push_str("space"),
        KeyCode::Char(c) => label.push(c),
        KeyCode::Enter => label.push_str("ret"),
        KeyCode::Escape => label.push_str("esc"),
        KeyCode::Tab => label.push_str("tab"),
        KeyCode::Backspace => label.push_str("backspace"),
        KeyCode::Delete => label.push_str("del"),
        KeyCode::Up => label.push_str("up"),
        KeyCode::Down => label.push_str("down"),
        KeyCode::Left => label.push_str("left"),
        KeyCode::Right => label.push_str("right"),
        KeyCode::Home => label.push_str("home"),
        KeyCode::End => label.push_str("end"),
        KeyCode::PageUp => label.push_str("pageup"),
        KeyCode::PageDown => label.push_str("pagedown"),
        KeyCode::F(n) => label.push_str(&format!("F{n}")),
    }
    label
}

/// What a binding does, for hints.
pub fn describe(binding: &Binding) -> String {
    match binding {
        Binding::Command(name) => name.clone(),
        Binding::Keys(keys) => {
            let keys: Vec<String> = keys.iter().map(label).collect();
            format!("as {}", keys.join(" "))
        }
        Binding::Prefix(_) => "…".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_parse_as_helix_writes_them() {
        let key = |code, modifiers| KeyEvent { code, modifiers };
        assert_eq!(
            parse_key("a"),
            Ok(key(KeyCode::Char('a'), Modifiers::empty()))
        );
        assert_eq!(
            parse_key("C-s"),
            Ok(key(KeyCode::Char('s'), Modifiers::CTRL))
        );
        assert_eq!(
            parse_key("S-a"),
            Ok(key(KeyCode::Char('A'), Modifiers::empty()))
        );
        assert_eq!(parse_key("S-tab"), Ok(key(KeyCode::Tab, Modifiers::SHIFT)));
        assert_eq!(parse_key("A-ret"), Ok(key(KeyCode::Enter, Modifiers::ALT)));
        assert_eq!(
            parse_key("-"),
            Ok(key(KeyCode::Char('-'), Modifiers::empty()))
        );
        assert_eq!(parse_key("F5"), Ok(key(KeyCode::F(5), Modifiers::empty())));
        assert!(parse_key("hello").is_err());
        assert_eq!(label(&parse_key("C-space").unwrap()), "C-space");
    }

    #[test]
    fn every_command_has_keys_that_parse() {
        for (name, _) in COMMANDS {
            command_keys(name).unwrap();
        }
    }

    #[test]
    fn wrong_entries_are_left_out() {
        let settings = json!({"keys": {"normal": {
            "C-s": "buffer.save",
            "q": "no_such_command",
            "nope": "undo",
            "g": {"a": "goto_file_start"},
        }}});
        let (keymaps, errors) = keymaps(&settings);
        assert_eq!(keymaps.normal.len(), 2);
        assert_eq!(
            errors,
            [
                "keys.normal.nope: unknown key \"nope\"",
                "keys.normal.q: unknown command \"no_such_command\"",
            ]
        );
    }
}
