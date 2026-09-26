//! Selections written into text, as Helix's tests write them: `#[` and
//! `]#` around the primary range, `#(` and `)#` around the others, and `|`
//! where the head is. `#[h|]#ello` is a cursor on the `h`.

use crate::selection::{Range, Selection};

/// Text with its selection marks taken out.
#[derive(Debug, PartialEq, Eq)]
pub struct Marked {
    pub text: String,
    /// In the order written; empty when the text had no marks.
    pub ranges: Vec<Range>,
    pub primary: usize,
}

pub fn parse(marked: &str) -> Result<Marked, String> {
    let mut text = String::with_capacity(marked.len());
    let mut ranges = Vec::new();
    let mut primary = None;
    // The range being read: whether it is primary, where it starts, and
    // where its head is if a `|` came.
    let mut open: Option<(bool, usize, Option<usize>)> = None;
    let mut rest = marked;
    while let Some(c) = rest.chars().next() {
        let two = rest.get(..2);
        match (two, &mut open) {
            (Some(start @ ("#[" | "#(")), None) => {
                open = Some((start == "#[", text.len(), None));
                rest = &rest[2..];
                continue;
            }
            (Some(end @ ("]#" | ")#")), Some((is_primary, start, head))) => {
                if (end == "]#") != *is_primary {
                    return Err(format!("{end} closes the wrong kind of range"));
                }
                let end_pos = text.len();
                let range = match *head {
                    Some(h) if h == *start && h != end_pos => Range::new(end_pos, h),
                    Some(h) if h == end_pos => Range::new(*start, end_pos),
                    None => Range::new(*start, end_pos),
                    Some(_) => return Err("`|` must be at the start or the end of a range".into()),
                };
                if *is_primary {
                    if primary.is_some() {
                        return Err("more than one primary range `#[`".into());
                    }
                    primary = Some(ranges.len());
                }
                ranges.push(range);
                open = None;
                rest = &rest[2..];
                continue;
            }
            (_, Some((_, _, head))) if c == '|' => {
                if head.is_some() {
                    return Err("more than one `|` in a range".into());
                }
                *head = Some(text.len());
            }
            _ => text.push(c),
        }
        rest = &rest[c.len_utf8()..];
    }
    if open.is_some() {
        return Err("a range is not closed".into());
    }
    let primary = match (primary, ranges.is_empty()) {
        (Some(primary), _) => primary,
        (None, true) => 0,
        (None, false) => return Err("no primary range `#[`".into()),
    };
    Ok(Marked {
        text,
        ranges,
        primary,
    })
}

/// `text` with the marks of `selection` put in.
pub fn show(text: &str, selection: &Selection) -> String {
    let mut out = String::with_capacity(text.len() + 6 * selection.ranges().len());
    let mut at = 0;
    for (i, range) in selection.ranges().iter().enumerate() {
        let (open, close) = if i == selection.primary_index() {
            ("#[", "]#")
        } else {
            ("#(", ")#")
        };
        let (from, to) = (range.from(), range.to());
        out.push_str(&text[at..from]);
        out.push_str(open);
        if range.head < range.anchor {
            out.push('|');
            out.push_str(&text[from..to]);
        } else {
            out.push_str(&text[from..to]);
            out.push('|');
        }
        out.push_str(close);
        at = to;
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use ropey::Rope;

    use super::*;

    fn round_trip(marked: &str) -> Marked {
        let parsed = parse(marked).unwrap();
        let text = Rope::from_str(&parsed.text);
        let selection = Selection::new(parsed.ranges.clone(), parsed.primary, &text).unwrap();
        assert_eq!(show(&parsed.text, &selection), marked);
        parsed
    }

    #[test]
    fn reads_and_writes_helix_marks() {
        let cursor = round_trip("#[h|]#ello");
        assert_eq!(cursor.text, "hello");
        assert_eq!(cursor.ranges, [Range::new(0, 1)]);

        let backward = round_trip("#[|hello]# world");
        assert_eq!(backward.ranges, [Range::new(5, 0)]);

        let two = round_trip("#(a|)#b #[c|]#d");
        assert_eq!(two.ranges, [Range::new(0, 1), Range::new(3, 4)]);
        assert_eq!(two.primary, 1);

        let empty = round_trip("ab#[|]#c");
        assert_eq!(empty.ranges, [Range::point(2)]);
    }

    #[test]
    fn plain_text_has_no_ranges() {
        let plain = parse("a | b").unwrap();
        assert_eq!(plain.text, "a | b");
        assert!(plain.ranges.is_empty());
    }

    #[test]
    fn broken_marks_are_errors() {
        for marked in [
            "#[ab",
            "#(a|)#",
            "#[a|b|]#",
            "#[a|b]#",
            "#[a]# #[b]#",
            "#[a)#",
        ] {
            assert!(parse(marked).is_err(), "{marked}");
        }
    }
}
