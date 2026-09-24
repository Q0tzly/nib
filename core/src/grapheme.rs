//! Byte-offset validation and grapheme cluster boundaries on a rope.

use ropey::Rope;
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

use crate::Error;

pub fn is_char_boundary(text: &Rope, pos: usize) -> bool {
    pos <= text.len_bytes() && text.char_to_byte(text.byte_to_char(pos)) == pos
}

pub fn check_position(text: &Rope, pos: usize) -> Result<(), Error> {
    if is_char_boundary(text, pos) {
        Ok(())
    } else {
        Err(Error::InvalidPosition(pos))
    }
}

/// Returns the first grapheme boundary after `pos`, or the end of the text.
pub fn next_boundary(text: &Rope, pos: usize) -> usize {
    let len = text.len_bytes();
    if pos >= len {
        return len;
    }
    let (mut chunk, mut chunk_start, _, _) = text.chunk_at_byte(pos);
    let mut cursor = GraphemeCursor::new(pos, len, true);
    loop {
        match cursor.next_boundary(chunk, chunk_start) {
            Ok(next) => return next.unwrap_or(len),
            Err(GraphemeIncomplete::NextChunk) => {
                chunk_start += chunk.len();
                chunk = text.chunk_at_byte(chunk_start).0;
            }
            Err(GraphemeIncomplete::PreContext(end)) => provide_context(text, &mut cursor, end),
            Err(err) => unreachable!("unexpected grapheme cursor state: {err:?}"),
        }
    }
}

/// Returns the last grapheme boundary before `pos`, or 0.
pub fn prev_boundary(text: &Rope, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let len = text.len_bytes();
    let (mut chunk, mut chunk_start, _, _) = text.chunk_at_byte(pos);
    let mut cursor = GraphemeCursor::new(pos, len, true);
    loop {
        match cursor.prev_boundary(chunk, chunk_start) {
            Ok(prev) => return prev.unwrap_or(0),
            Err(GraphemeIncomplete::PrevChunk) => {
                let (prev_chunk, prev_start, _, _) = text.chunk_at_byte(chunk_start - 1);
                chunk = prev_chunk;
                chunk_start = prev_start;
            }
            Err(GraphemeIncomplete::PreContext(end)) => provide_context(text, &mut cursor, end),
            Err(err) => unreachable!("unexpected grapheme cursor state: {err:?}"),
        }
    }
}

pub fn is_boundary(text: &Rope, pos: usize) -> bool {
    let len = text.len_bytes();
    if pos == 0 || pos == len {
        return true;
    }
    let (chunk, chunk_start, _, _) = text.chunk_at_byte(pos);
    let mut cursor = GraphemeCursor::new(pos, len, true);
    loop {
        match cursor.is_boundary(chunk, chunk_start) {
            Ok(boundary) => return boundary,
            Err(GraphemeIncomplete::PreContext(end)) => provide_context(text, &mut cursor, end),
            Err(err) => unreachable!("unexpected grapheme cursor state: {err:?}"),
        }
    }
}

/// Rounds `pos` down to a grapheme boundary.
pub fn floor(text: &Rope, pos: usize) -> usize {
    if is_boundary(text, pos) {
        pos
    } else {
        prev_boundary(text, pos)
    }
}

/// Rounds `pos` up to a grapheme boundary.
pub fn ceil(text: &Rope, pos: usize) -> usize {
    if is_boundary(text, pos) {
        pos
    } else {
        next_boundary(text, pos)
    }
}

fn provide_context(text: &Rope, cursor: &mut GraphemeCursor, end: usize) {
    let (chunk, chunk_start, _, _) = text.chunk_at_byte(end - 1);
    cursor.provide_context(chunk, chunk_start);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_boundaries() {
        let text = Rope::from_str("aあb");
        assert!(is_char_boundary(&text, 0));
        assert!(is_char_boundary(&text, 1));
        assert!(!is_char_boundary(&text, 2));
        assert!(!is_char_boundary(&text, 3));
        assert!(is_char_boundary(&text, 4));
        assert!(is_char_boundary(&text, 5));
        assert!(!is_char_boundary(&text, 6));
    }

    #[test]
    fn combining_marks_and_emoji_form_single_graphemes() {
        // "e" + combining acute, then a family emoji joined with ZWJs
        let s = "e\u{301}👨\u{200d}👩\u{200d}👧x";
        let text = Rope::from_str(s);
        let emoji_start = "e\u{301}".len();
        let x = s.len() - 1;
        assert_eq!(next_boundary(&text, 0), emoji_start);
        assert_eq!(next_boundary(&text, emoji_start), x);
        assert_eq!(next_boundary(&text, x), s.len());
        assert_eq!(prev_boundary(&text, x), emoji_start);
        assert_eq!(prev_boundary(&text, emoji_start), 0);
        assert!(!is_boundary(&text, 1));
        assert_eq!(floor(&text, 1), 0);
        assert_eq!(ceil(&text, 1), emoji_start);
    }

    #[test]
    fn grapheme_spanning_many_chunks() {
        let s = format!("a{}b", "\u{301}".repeat(5000));
        let text = Rope::from_str(&s);
        assert!(text.chunks().count() > 1);
        let b = s.len() - 1;
        assert_eq!(next_boundary(&text, 0), b);
        assert_eq!(prev_boundary(&text, b), 0);
        assert_eq!(floor(&text, b - 2), 0);
        assert_eq!(ceil(&text, 3), b);
    }

    #[test]
    fn edges() {
        let text = Rope::from_str("ab");
        assert_eq!(next_boundary(&text, 2), 2);
        assert_eq!(prev_boundary(&text, 0), 0);
        let empty = Rope::new();
        assert_eq!(next_boundary(&empty, 0), 0);
        assert!(is_boundary(&empty, 0));
    }
}
