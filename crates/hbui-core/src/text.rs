//! Editable text, held in graphemes.
//!
//! Four different things get called "a position in a string", and this module
//! exists so they stay apart:
//!
//! - a UTF-8 **byte** index — what `String` slices by, and never stored here;
//! - a Unicode **scalar** (`char`) — not what a person thinks of as a letter;
//! - a **grapheme** cluster — what a person thinks of as a letter, and what the
//!   cursor counts;
//! - a terminal **cell** — what the renderer counts, and `width` measures.
//!
//! The cursor is a grapheme index, so "move right" can never land inside `é`
//! written as `e` + U+0301, and never between the halves of a byte sequence.
//! Screen coordinates are not stored at all; the renderer derives them.

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// A cursor position, in graphemes from the start of the text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TextCursor {
    pub grapheme: usize,
}

/// The state of a one-line text field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextBuffer {
    text: String,
    cursor: TextCursor,
}

impl TextBuffer {
    /// A buffer holding `text`, with the cursor at its end — where a person
    /// expects to start typing into a prefilled field.
    pub fn new(text: impl Into<String>) -> Self {
        let text = sanitize(&text.into());
        let cursor = TextCursor {
            grapheme: text.graphemes(true).count(),
        };
        Self { text, cursor }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> TextCursor {
        self.cursor
    }

    pub fn len_graphemes(&self) -> usize {
        self.text.graphemes(true).count()
    }

    /// Replace the whole text, cursor to the end. What `set_text` does.
    pub fn set(&mut self, text: &str) {
        *self = Self::new(text);
    }

    /// Insert at the cursor and move past what was inserted.
    ///
    /// Newlines and other control characters are dropped: this is a one-line
    /// field, and a pasted paragraph should arrive as one line, not as a line
    /// the renderer then has to decide how to draw.
    pub fn insert(&mut self, s: &str) {
        let s = sanitize(s);
        if s.is_empty() {
            return;
        }
        let at = self.byte_at(self.cursor.grapheme);
        self.text.insert_str(at, &s);
        // Count graphemes in the text up to the end of the insertion rather
        // than in `s` alone: an inserted combining mark merges with the
        // grapheme before it, and the cursor must not count it twice.
        self.cursor.grapheme = self.text[..at + s.len()].graphemes(true).count();
    }

    /// Delete the grapheme before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor.grapheme == 0 {
            return;
        }
        let start = self.byte_at(self.cursor.grapheme - 1);
        let end = self.byte_at(self.cursor.grapheme);
        self.text.replace_range(start..end, "");
        self.cursor.grapheme -= 1;
    }

    /// Delete the grapheme under the cursor.
    pub fn delete(&mut self) {
        if self.cursor.grapheme >= self.len_graphemes() {
            return;
        }
        let start = self.byte_at(self.cursor.grapheme);
        let end = self.byte_at(self.cursor.grapheme + 1);
        self.text.replace_range(start..end, "");
    }

    pub fn left(&mut self) {
        self.cursor.grapheme = self.cursor.grapheme.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor.grapheme = (self.cursor.grapheme + 1).min(self.len_graphemes());
    }

    pub fn home(&mut self) {
        self.cursor.grapheme = 0;
    }

    pub fn end(&mut self) {
        self.cursor.grapheme = self.len_graphemes();
    }

    /// Terminal cells between the start of the text and the cursor — where the
    /// renderer puts the terminal cursor, before scrolling.
    pub fn cursor_cells(&self) -> usize {
        width(&self.text[..self.byte_at(self.cursor.grapheme)])
    }

    /// The byte offset of grapheme `n`, or the end of the text past the last.
    fn byte_at(&self, n: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .nth(n)
            .map_or(self.text.len(), |(i, _)| i)
    }
}

/// Terminal cells `s` occupies. Control characters count as zero; the text
/// here never holds any, because [`TextBuffer`] strips them on the way in.
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn sanitize(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_counts_graphemes_not_bytes_or_chars() {
        // "é" as e + combining acute: two chars, three bytes, one grapheme.
        let mut t = TextBuffer::new("ae\u{301}b");
        assert_eq!(t.len_graphemes(), 3);
        t.left(); // before "b"
        t.backspace(); // removes the whole "é", not just the accent
        assert_eq!(t.text(), "ab");
        assert_eq!(t.cursor().grapheme, 1);
    }

    #[test]
    fn wide_characters_are_two_cells_but_one_grapheme() {
        let mut t = TextBuffer::new("日本語");
        assert_eq!(t.cursor().grapheme, 3);
        assert_eq!(t.cursor_cells(), 6);
        t.left();
        assert_eq!(t.cursor_cells(), 4);
    }

    #[test]
    fn insertion_lands_at_the_cursor_and_moves_past_it() {
        let mut t = TextBuffer::new("ac");
        t.left();
        t.insert("b");
        assert_eq!(t.text(), "abc");
        assert_eq!(t.cursor().grapheme, 2);
        t.home();
        t.insert("ファイル_");
        assert_eq!(t.text(), "ファイル_abc");
        assert_eq!(t.cursor().grapheme, 5);
    }

    #[test]
    fn a_combining_mark_merges_with_the_grapheme_before_it() {
        let mut t = TextBuffer::new("e");
        t.insert("\u{301}");
        assert_eq!(t.len_graphemes(), 1);
        assert_eq!(t.cursor().grapheme, 1);
    }

    #[test]
    fn a_pasted_newline_does_not_make_a_second_line() {
        let mut t = TextBuffer::new("");
        t.insert("one\ntwo\r\n");
        assert_eq!(t.text(), "onetwo");
    }

    #[test]
    fn editing_at_the_edges_is_a_no_op_not_a_panic() {
        let mut t = TextBuffer::new("");
        t.backspace();
        t.delete();
        t.left();
        t.right();
        assert_eq!(t, TextBuffer::new(""));
        let mut t = TextBuffer::new("x");
        t.delete(); // cursor is at the end
        assert_eq!(t.text(), "x");
    }
}
