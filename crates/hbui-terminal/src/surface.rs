//! A grid of cells: what the renderer draws into, what the diff writer
//! compares, and what a visual snapshot test reads.
//!
//! Nothing here touches a terminal. A `Surface` is plain data, so "is this
//! drawn correctly?" is a question a test can ask without one.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Color {
    #[default]
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    Grey,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub reverse: bool,
    pub underline: bool,
}

impl Style {
    pub const PLAIN: Style = Style {
        fg: Color::Reset,
        bg: Color::Reset,
        bold: false,
        reverse: false,
        underline: false,
    };

    pub fn fg(self, fg: Color) -> Self {
        Self { fg, ..self }
    }

    pub fn bold(self) -> Self {
        Self { bold: true, ..self }
    }

    pub fn reverse(self) -> Self {
        Self {
            reverse: true,
            ..self
        }
    }

    pub fn underline(self) -> Self {
        Self {
            underline: true,
            ..self
        }
    }
}

/// One terminal cell.
///
/// A wide grapheme (日, most emoji) occupies two cells: the first holds it, the
/// second holds the empty string as a continuation marker. That keeps a cell
/// index equal to a terminal column, which is the whole point of a cell grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub grapheme: String,
    pub style: Style,
}

impl Cell {
    pub fn is_continuation(&self) -> bool {
        self.grapheme.is_empty()
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            grapheme: " ".into(),
            style: Style::PLAIN,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub fn new(x: u16, y: u16, w: u16, h: u16) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> u16 {
        self.x + self.w
    }

    pub fn bottom(&self) -> u16 {
        self.y + self.h
    }

    /// The rect one cell in from each edge, for what goes inside a border.
    pub fn inner(&self) -> Rect {
        Rect::new(
            self.x + 1,
            self.y + 1,
            self.w.saturating_sub(2),
            self.h.saturating_sub(2),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Surface {
    pub width: u16,
    pub height: u16,
    cells: Vec<Cell>,
}

impl Surface {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            cells: vec![Cell::default(); width as usize * height as usize],
        }
    }

    pub fn area(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    pub fn cell(&self, x: u16, y: u16) -> &Cell {
        &self.cells[self.index(x, y)]
    }

    pub fn row(&self, y: u16) -> &[Cell] {
        let start = y as usize * self.width as usize;
        &self.cells[start..start + self.width as usize]
    }

    fn index(&self, x: u16, y: u16) -> usize {
        y as usize * self.width as usize + x as usize
    }

    /// Write one grapheme of width 1 or 2 at `(x, y)`, repairing any wide
    /// grapheme it cuts in half so the grid never holds an orphaned half.
    fn set(&mut self, x: u16, y: u16, grapheme: &str, style: Style, wide: bool) {
        if x >= self.width || y >= self.height {
            return;
        }
        let last = x + u16::from(wide);
        // Overwriting a continuation orphans the wide grapheme to its left.
        if self.cell(x, y).is_continuation() && x > 0 {
            let i = self.index(x - 1, y);
            self.cells[i].grapheme = " ".into();
        }
        // Overwriting the lead of a wide grapheme orphans its continuation.
        if last + 1 < self.width && self.cell(last + 1, y).is_continuation() {
            let i = self.index(last + 1, y);
            self.cells[i].grapheme = " ".into();
        }
        let i = self.index(x, y);
        self.cells[i] = Cell {
            grapheme: grapheme.into(),
            style,
        };
        if wide {
            let i = self.index(x + 1, y);
            self.cells[i] = Cell {
                grapheme: String::new(),
                style,
            };
        }
    }

    /// Write `s` from `(x, y)`, stopping before column `max_x`. Returns the
    /// column after the last cell written.
    ///
    /// A wide grapheme that would straddle `max_x` is not drawn at all, rather
    /// than drawn half. Zero-width graphemes on their own (stray combining
    /// marks, controls) are skipped: they have no cell to go in.
    pub fn put_str(&mut self, x: u16, y: u16, s: &str, style: Style, max_x: u16) -> u16 {
        let max_x = max_x.min(self.width);
        let mut x = x;
        for g in s.graphemes(true) {
            let w = g.width();
            if w == 0 || g.chars().any(char::is_control) {
                continue;
            }
            let wide = w >= 2;
            if x + 1 + u16::from(wide) > max_x {
                break;
            }
            self.set(x, y, g, style, wide);
            x += 1 + u16::from(wide);
        }
        x
    }

    pub fn fill(&mut self, r: Rect, ch: &str, style: Style) {
        for y in r.y..r.bottom().min(self.height) {
            for x in r.x..r.right().min(self.width) {
                self.set(x, y, ch, style, false);
            }
        }
    }

    /// Restyle cells without changing what they hold — for highlighting a row.
    pub fn restyle(&mut self, r: Rect, style: Style) {
        for y in r.y..r.bottom().min(self.height) {
            for x in r.x..r.right().min(self.width) {
                let i = self.index(x, y);
                self.cells[i].style = style;
            }
        }
    }

    /// The visual snapshot: one line per row, trailing blanks trimmed.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for y in 0..self.height {
            let line: String = self.row(y).iter().map(|c| c.grapheme.as_str()).collect();
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_graphemes_take_two_cells() {
        let mut s = Surface::new(6, 1);
        let end = s.put_str(0, 0, "日本x", Style::PLAIN, 6);
        assert_eq!(end, 5);
        assert!(s.cell(1, 0).is_continuation());
        assert_eq!(s.to_text(), "日本x\n");
    }

    #[test]
    fn a_wide_grapheme_is_never_split_at_the_edge() {
        let mut s = Surface::new(4, 1);
        s.put_str(0, 0, "ab日", Style::PLAIN, 3);
        assert_eq!(s.to_text(), "ab\n");
    }

    #[test]
    fn overwriting_half_a_wide_grapheme_blanks_the_other_half() {
        let mut s = Surface::new(4, 1);
        s.put_str(0, 0, "日本", Style::PLAIN, 4);
        s.put_str(1, 0, "x", Style::PLAIN, 4);
        assert_eq!(s.to_text(), " x本\n");
        s.put_str(2, 0, "y", Style::PLAIN, 4);
        assert_eq!(s.to_text(), " xy\n");
    }
}
