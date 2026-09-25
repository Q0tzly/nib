//! Completion items: reading them from a response, narrowing them to what
//! is typed, and the text a chosen one inserts.

use serde_json::Value;

pub struct Item {
    pub label: String,
    pub detail: String,
    /// What typing is matched against.
    filter: String,
    sort: String,
    /// The text to insert, with snippet marks removed.
    pub text: String,
    /// The range the server wants replaced, as LSP positions.
    pub range: Option<((u32, u32), (u32, u32))>,
}

/// The items of a completion response: a list, or a list in `items`.
pub fn items(result: &Value) -> Vec<Item> {
    let list = result
        .as_array()
        .or_else(|| result["items"].as_array())
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut items: Vec<Item> = list.iter().filter_map(item).collect();
    items.sort_by(|a, b| (&a.sort, &a.label).cmp(&(&b.sort, &b.label)));
    items
}

fn item(value: &Value) -> Option<Item> {
    let label = value["label"].as_str()?.to_string();
    let string = |key: &str| value[key].as_str().map(String::from);
    let edit = &value["textEdit"];
    // A plain edit has a range; an insert-or-replace one has two.
    let range = edit
        .get("range")
        .or_else(|| edit.get("replace"))
        .and_then(position_range);
    let text = string("insertText")
        .or_else(|| edit["newText"].as_str().map(String::from))
        .unwrap_or_else(|| label.clone());
    let text = if range.is_some() {
        edit["newText"].as_str().map_or(text, String::from)
    } else {
        text
    };
    let snippet = value["insertTextFormat"].as_u64() == Some(2);
    Some(Item {
        filter: string("filterText").unwrap_or_else(|| label.clone()),
        sort: string("sortText").unwrap_or_else(|| label.clone()),
        detail: string("detail").unwrap_or_default(),
        text: if snippet { strip_snippet(&text) } else { text },
        range,
        label,
    })
}

fn position_range(range: &Value) -> Option<((u32, u32), (u32, u32))> {
    let at = |end: &str| {
        let position = &range[end];
        Some((
            position["line"].as_u64()? as u32,
            position["character"].as_u64()? as u32,
        ))
    };
    Some((at("start")?, at("end")?))
}

/// The indices of the items that start with `prefix`, ignoring case.
pub fn matching(items: &[Item], prefix: &str) -> Vec<usize> {
    let prefix = prefix.to_lowercase();
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.filter.to_lowercase().starts_with(&prefix))
        .map(|(i, _)| i)
        .collect()
}

/// A snippet's text without its tab stops: `$1` and `$0` go, and
/// `${1:name}` leaves `name`.
pub fn strip_snippet(snippet: &str) -> String {
    let mut out = String::new();
    let mut chars = snippet.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            '$' if chars.peek() == Some(&'{') => {
                chars.next();
                // Skip the number, and keep what follows a colon.
                let mut depth = 1;
                let mut keep = false;
                for c in chars.by_ref() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        ':' if !keep => {
                            keep = true;
                            continue;
                        }
                        _ => {}
                    }
                    if keep {
                        out.push(c);
                    }
                }
            }
            '$' if chars.peek().is_some_and(char::is_ascii_digit) => {
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    chars.next();
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// Chars that make up the word being completed.
pub fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn snippets_lose_their_marks() {
        assert_eq!(strip_snippet("f($1)$0"), "f()");
        assert_eq!(strip_snippet("f(${1:x}, ${2:y})"), "f(x, y)");
        assert_eq!(strip_snippet("a \\$1 b"), "a $1 b");
    }

    #[test]
    fn items_are_sorted_and_matched_by_prefix() {
        let result = json!({"items": [
            {"label": "beta"},
            {"label": "Alpha", "sortText": "2"},
            {"label": "alphabet", "sortText": "1", "insertText": "alphabet($1)", "insertTextFormat": 2},
        ]});
        let items = items(&result);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["alphabet", "Alpha", "beta"]);
        assert_eq!(items[0].text, "alphabet()");
        assert_eq!(matching(&items, "al"), [0, 1]);
        assert_eq!(matching(&items, ""), [0, 1, 2]);
    }
}
