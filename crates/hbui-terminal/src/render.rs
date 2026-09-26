//! `UiState` → `Surface`. A pure function of the state and the terminal size.
//!
//! Everything positional is decided here, every frame: pane rectangles, which
//! list rows are scrolled into view, how far an input is scrolled, where the
//! terminal cursor goes. None of it is written back into the state. That is
//! what makes resizing free and what keeps the agent's view free of
//! coordinates.

use hbui_core::text::width;
use hbui_core::{Direction, Layout, Size, UiState, Widget, WidgetId};
use unicode_segmentation::UnicodeSegmentation;

use crate::surface::{Color, Rect, Style, Surface};

/// Cells kept free to the right of an editable field's text.
///
/// Some terminals — Terminal.app with an IME composing, most visibly — turn
/// unstable when the cursor reaches the last column. Rather than chase every
/// such case, the cursor is simply never put anywhere near it.
pub const SAFETY_MARGIN: u16 = 2;

/// A rendered frame: the cells, and where the terminal cursor belongs.
///
/// `cursor` is `Some` only while a text input has focus. Otherwise the cursor
/// is hidden — a blinking block in a file list only confuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub surface: Surface,
    pub cursor: Option<(u16, u16)>,
}

pub fn render(ui: &UiState, width: u16, height: u16) -> Frame {
    let mut r = Renderer {
        ui,
        surface: Surface::new(width, height),
        cursor: None,
    };
    let mut area = r.surface.area();
    let bar = ui.commands.iter().any(|c| c.key.is_some()) && area.h > 1;
    if bar {
        area.h -= 1;
        r.key_bar(Rect::new(0, area.bottom(), width, 1));
    }
    r.layout(&ui.layout, area);
    if ui.modal().is_some() {
        // Anything the screen behind put a cursor on is no longer where input goes.
        r.cursor = None;
        r.modal(area);
    }
    // Belt and braces: whatever placed it, the cursor never sits in the last column.
    let cursor = r.cursor.map(|(x, y)| (x.min(width.saturating_sub(2)), y));
    Frame {
        surface: r.surface,
        cursor,
    }
}

struct Renderer<'a> {
    ui: &'a UiState,
    surface: Surface,
    cursor: Option<(u16, u16)>,
}

impl Renderer<'_> {
    fn focused(&self, id: &WidgetId) -> bool {
        self.ui.focus() == Some(id)
    }

    fn layout(&mut self, layout: &Layout, area: Rect) {
        if area.is_empty() {
            return;
        }
        match layout {
            Layout::Split {
                direction,
                children,
            } => {
                let total = match direction {
                    Direction::Horizontal => area.w,
                    Direction::Vertical => area.h,
                };
                let sizes: Vec<Size> = children.iter().map(|(s, _)| *s).collect();
                let sizes = split(total, &sizes);
                let mut at = 0;
                for ((_, child), len) in children.iter().zip(sizes) {
                    let r = match direction {
                        Direction::Horizontal => Rect::new(area.x + at, area.y, len, area.h),
                        Direction::Vertical => Rect::new(area.x, area.y + at, area.w, len),
                    };
                    self.layout(child, r);
                    at += len;
                }
            }
            Layout::Pane { title, content, .. } => {
                let active = self.focused(content) && self.ui.modal().is_none();
                let style = if active {
                    Style::PLAIN.fg(Color::Cyan).bold()
                } else {
                    Style::PLAIN
                };
                self.frame(area, title, style);
                self.widget(content, area.inner());
            }
            Layout::Tabs { tabs, active, .. } => {
                let mut x = area.x;
                for (i, tab) in tabs.iter().enumerate() {
                    let label = format!(" {} ", tab.title);
                    let style = if i == *active {
                        Style::PLAIN.reverse()
                    } else {
                        Style::PLAIN
                    };
                    x = self.surface.put_str(x, area.y, &label, style, area.right());
                }
                if let Some(tab) = tabs.get(*active) {
                    let body = Rect::new(area.x, area.y + 1, area.w, area.h.saturating_sub(1));
                    self.layout(&tab.content, body);
                }
            }
            Layout::Widget(id) => self.widget(id, area),
        }
    }

    /// A box with a centered title: `┌──── Left ────┐`.
    fn frame(&mut self, r: Rect, title: &str, style: Style) {
        if r.w < 2 || r.h < 2 {
            return;
        }
        let s = &mut self.surface;
        let (right, bottom) = (r.right() - 1, r.bottom() - 1);
        for x in r.x + 1..right {
            s.put_str(x, r.y, "─", style, x + 1);
            s.put_str(x, bottom, "─", style, x + 1);
        }
        for y in r.y + 1..bottom {
            s.put_str(r.x, y, "│", style, r.x + 1);
            s.put_str(right, y, "│", style, right + 1);
        }
        s.put_str(r.x, r.y, "┌", style, r.x + 1);
        s.put_str(right, r.y, "┐", style, right + 1);
        s.put_str(r.x, bottom, "└", style, r.x + 1);
        s.put_str(right, bottom, "┘", style, right + 1);
        if !title.is_empty() && r.w > 4 {
            let label = format!(" {} ", tail(title, (r.w - 4) as usize));
            let w = (width(&label) as u16).min(r.w - 2);
            let x = r.x + (r.w - w) / 2;
            s.put_str(x, r.y, &label, style, right);
        }
    }

    fn widget(&mut self, id: &WidgetId, r: Rect) {
        if r.is_empty() {
            return;
        }
        let Some(widget) = self.ui.widget(id.as_str()) else {
            return;
        };
        let focused = self.focused(id);
        match widget {
            Widget::Text(t) => {
                for (i, line) in t.value.lines().take(r.h as usize).enumerate() {
                    self.surface
                        .put_str(r.x, r.y + i as u16, line, Style::PLAIN, r.right());
                }
            }
            Widget::List(l) => {
                let rows: Vec<(usize, &str)> =
                    l.items.iter().map(|i| (0, i.label.as_str())).collect();
                self.rows(r, &rows, l.selected_index(), focused, |_| "");
            }
            Widget::Tree(t) => {
                let visible = t.visible();
                let rows: Vec<(usize, &str)> = visible
                    .iter()
                    .map(|(d, n)| (*d, n.label.as_str()))
                    .collect();
                let selected = t
                    .selected
                    .as_deref()
                    .and_then(|s| visible.iter().position(|(_, n)| n.id == s));
                self.rows(r, &rows, selected, focused, |i| {
                    let n = visible[i].1;
                    match (n.children.is_empty(), n.expanded) {
                        (true, _) => "  ",
                        (false, true) => "▾ ",
                        (false, false) => "▸ ",
                    }
                });
            }
            Widget::Input(input) => self.input(input, r, focused),
            Widget::Button(b) => {
                let style = if focused {
                    Style::PLAIN.reverse()
                } else {
                    Style::PLAIN
                };
                self.surface
                    .put_str(r.x, r.y, &format!("[ {} ]", b.label), style, r.right());
            }
        }
    }

    /// List-like rows with a `> ` marker on the selection, scrolled so the
    /// selection is always on screen.
    fn rows(
        &mut self,
        r: Rect,
        rows: &[(usize, &str)],
        selected: Option<usize>,
        focused: bool,
        glyph: impl Fn(usize) -> &'static str,
    ) {
        let h = r.h as usize;
        let offset = selected.map_or(0, |s| (s + 1).saturating_sub(h));
        for (row, i) in (offset..rows.len()).take(h).enumerate() {
            let (depth, label) = rows[i];
            let y = r.y + row as u16;
            let is_sel = selected == Some(i);
            let text = format!(
                "{}{}{}{}",
                if is_sel { "> " } else { "  " },
                "  ".repeat(depth),
                glyph(i),
                label
            );
            let style = match (is_sel, focused) {
                (true, true) => Style::PLAIN.reverse(),
                (true, false) => Style::PLAIN.bold(),
                _ => Style::PLAIN,
            };
            if is_sel && focused {
                self.surface.fill(Rect::new(r.x, y, r.w, 1), " ", style);
            }
            self.surface.put_str(r.x, y, &text, style, r.right());
        }
    }

    /// A one-line field, scrolled horizontally so the cursor stays at least
    /// [`SAFETY_MARGIN`] cells from the right edge.
    fn input(&mut self, input: &hbui_core::Input, r: Rect, focused: bool) {
        let field = Rect::new(r.x, r.y, r.w, 1);
        let style = Style::PLAIN.underline();
        self.surface.fill(field, " ", style);

        let usable = field.w.saturating_sub(SAFETY_MARGIN).max(1) as usize;
        let cursor = input.buffer.cursor_cells();
        // Skip whole graphemes until the cursor fits; never half of a wide one.
        let text = input.buffer.text();
        let mut skipped = 0;
        let mut start = 0;
        for (i, g) in text.grapheme_indices(true) {
            if cursor - skipped < usable {
                break;
            }
            skipped += width(g);
            start = i + g.len();
        }
        self.surface
            .put_str(field.x, field.y, &text[start..], style, field.right());
        if focused {
            self.cursor = Some((field.x + (cursor - skipped) as u16, field.y));
        }
    }

    fn modal(&mut self, area: Rect) {
        let Some(modal) = self.ui.modal() else { return };
        // Consecutive buttons share a row, like a dialog's button bar.
        let mut rows: Vec<Vec<&WidgetId>> = Vec::new();
        for id in &modal.children {
            let is_button = matches!(self.ui.widget(id.as_str()), Some(Widget::Button(_)));
            let joins = is_button
                && rows.last().is_some_and(|row| {
                    matches!(self.ui.widget(row[0].as_str()), Some(Widget::Button(_)))
                });
            if joins {
                rows.last_mut().unwrap().push(id);
            } else {
                rows.push(vec![id]);
            }
        }
        let height_of = |row: &[&WidgetId]| -> u16 {
            match self.ui.widget(row[0].as_str()) {
                Some(Widget::Text(t)) => t.value.lines().count().max(1) as u16,
                Some(Widget::List(_) | Widget::Tree(_)) => 6,
                _ => 1,
            }
        };
        let inner_h: u16 =
            rows.iter().map(|r| height_of(r)).sum::<u16>() + rows.len().saturating_sub(1) as u16;
        let w = area.w.saturating_sub(4).min(60);
        let h = (inner_h + 4).min(area.h);
        if w < 8 || h < 3 {
            return;
        }
        let outer = Rect::new(area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h);
        self.surface.fill(outer, " ", Style::PLAIN);
        self.frame(outer, &modal.title, Style::PLAIN.fg(Color::Cyan).bold());

        // One cell of padding inside the border on every side.
        let body = Rect::new(
            outer.x + 2,
            outer.y + 2,
            outer.w.saturating_sub(4),
            outer.h.saturating_sub(4),
        );
        let mut y = body.y;
        for row in &rows {
            let rh = height_of(row).min(body.bottom().saturating_sub(y));
            let mut x = body.x;
            for id in row {
                let label = match self.ui.widget(id.as_str()) {
                    Some(Widget::Button(b)) => width(&b.label) as u16 + 4,
                    _ => body.right() - x,
                };
                self.widget(id, Rect::new(x, y, label.min(body.right() - x), rh));
                x = (x + label + 2).min(body.right());
            }
            y += rh + 1;
            if y >= body.bottom() {
                break;
            }
        }
    }

    /// The command bar: `F2 Rename  F5 Copy`, drawn from the same command list
    /// the agent reads.
    fn key_bar(&mut self, r: Rect) {
        let mut x = r.x;
        for c in &self.ui.commands {
            let Some(key) = c.key else { continue };
            x = self
                .surface
                .put_str(x, r.y, &key.to_string(), Style::PLAIN.bold(), r.right());
            x = self.surface.put_str(
                x,
                r.y,
                &format!(" {} ", c.label),
                Style::PLAIN.reverse(),
                r.right(),
            );
            x = self.surface.put_str(x, r.y, " ", Style::PLAIN, r.right());
        }
    }
}

/// The end of `s` that fits in `max` cells, with `…` in front if anything was
/// cut. The end, because for a title that is a path, the end is the part that
/// says where you are.
fn tail(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let mut kept = Vec::new();
    let mut used = 1; // the ellipsis
    for g in s.graphemes(true).rev() {
        used += width(g);
        if used > max {
            break;
        }
        kept.push(g);
    }
    kept.reverse();
    format!("…{}", kept.concat())
}

/// Divide `total` cells between children: fixed ones first, then the rest by
/// weight, with the rounding remainder going to the last fill.
fn split(total: u16, sizes: &[Size]) -> Vec<u16> {
    let fixed = sizes.iter().fold(0u16, |a, s| match s {
        Size::Fixed(n) => a.saturating_add(*n),
        Size::Fill(_) => a,
    });
    let weights: u32 = sizes
        .iter()
        .map(|s| if let Size::Fill(w) = s { *w as u32 } else { 0 })
        .sum();
    let free = total.saturating_sub(fixed) as u32;
    let mut out: Vec<u16> = sizes
        .iter()
        .map(|s| match s {
            Size::Fixed(n) => *n,
            Size::Fill(w) if weights > 0 => (free * *w as u32 / weights) as u16,
            Size::Fill(_) => 0,
        })
        .collect();
    let used = out.iter().fold(0u16, |a, b| a.saturating_add(*b));
    if let Some(last_fill) = sizes
        .iter()
        .rposition(|s| matches!(s, Size::Fill(w) if *w > 0))
    {
        out[last_fill] += total.saturating_sub(used);
    }
    // Clip fixed sizes that did not fit, front to back.
    let mut left = total;
    for len in &mut out {
        *len = (*len).min(left);
        left -= *len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_share_what_the_fixed_sizes_leave() {
        let s = [Size::Fill(1), Size::Fixed(3), Size::Fill(1)];
        assert_eq!(split(21, &s), vec![9, 3, 9]);
        let s = [Size::Fill(1), Size::Fill(1)];
        assert_eq!(split(5, &s), vec![2, 3]);
    }

    #[test]
    fn long_titles_keep_their_end() {
        assert_eq!(tail("/usr/local/share", 20), "/usr/local/share");
        assert_eq!(tail("/usr/local/share", 8), "…l/share");
        assert_eq!(tail("/日本語/フォルダ", 9), "…フォルダ");
    }

    #[test]
    fn fixed_sizes_that_do_not_fit_are_clipped() {
        let s = [Size::Fixed(4), Size::Fixed(4)];
        assert_eq!(split(6, &s), vec![4, 2]);
    }
}
