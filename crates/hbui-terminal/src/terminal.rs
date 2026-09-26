//! The only module that talks to a real terminal, and the only one that
//! names crossterm.
//!
//! It does three things and tries to be clever about none of them: put the
//! terminal into a state it can be restored from, write the cells that
//! changed, and hand keystrokes to the core. Terminals get raw drawing and raw
//! input; the intelligence stays in the serializable state.

use std::io::{self, Stdout, Write};
use std::sync::Once;
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::style::{Attribute, Print, SetAttribute, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute, queue};
use hbui_core::{InputEvent, Key, Shared};

use crate::render::{render, Frame};
use crate::surface::{Color, Style, Surface};

/// Raw mode, the alternate screen and bracketed paste, for as long as it
/// lives — and all of it undone when it drops, or when anything panics.
///
/// Written first and trusted most. A UI library that leaves someone's shell
/// in raw mode after a bug has broken their terminal, not just itself.
pub struct TerminalGuard {
    _private: (),
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        install_panic_hook();
        terminal::enable_raw_mode()?;
        let guard = Self { _private: () };
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            cursor::Hide
        )?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}

/// Undo everything [`TerminalGuard::enter`] did. Safe to call twice: each step
/// is idempotent, and the panic hook and the guard may both run.
fn restore() {
    let _ = execute!(
        io::stdout(),
        SetAttribute(Attribute::Reset),
        cursor::Show,
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
}

/// Restore the terminal *before* the panic message is printed, so the message
/// lands on the normal screen, readable, instead of on the alternate screen
/// that is about to vanish.
fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
    });
}

/// Writes frames, sending only the cells that changed since the last one.
///
/// Redrawing the whole screen on every change is what makes an IME's
/// composition window flicker and jump; writing a few cells, and not moving
/// the cursor at all when nothing changed, is what lets it sit still.
pub struct Screen {
    out: Stdout,
    previous: Option<Surface>,
    cursor: Option<(u16, u16)>,
}

impl Default for Screen {
    fn default() -> Self {
        Self::new()
    }
}

impl Screen {
    pub fn new() -> Self {
        Self {
            out: io::stdout(),
            previous: None,
            cursor: None,
        }
    }

    /// Forget what is on screen, so the next frame is drawn in full. After a
    /// resize the terminal's contents are anyone's guess.
    pub fn invalidate(&mut self) {
        self.previous = None;
    }

    pub fn draw(&mut self, frame: &Frame) -> io::Result<()> {
        let next = &frame.surface;
        let full = self
            .previous
            .as_ref()
            .is_none_or(|p| p.width != next.width || p.height != next.height);
        if full {
            queue!(
                self.out,
                SetAttribute(Attribute::Reset),
                terminal::Clear(terminal::ClearType::All)
            )?;
        }
        let mut wrote = false;
        for y in 0..next.height {
            let span = match (&self.previous, full) {
                (Some(prev), false) => changed_span(prev.row(y), next.row(y)),
                _ => Some((0, next.width - 1)),
            };
            let Some((x0, x1)) = span else { continue };
            if !wrote {
                queue!(self.out, cursor::Hide)?;
                wrote = true;
            }
            queue!(self.out, cursor::MoveTo(x0, y))?;
            let mut style = None;
            for x in x0..=x1 {
                let cell = next.cell(x, y);
                if cell.is_continuation() {
                    continue;
                }
                if style != Some(cell.style) {
                    apply_style(&mut self.out, cell.style)?;
                    style = Some(cell.style);
                }
                queue!(self.out, Print(&cell.grapheme))?;
            }
        }
        if wrote || frame.cursor != self.cursor {
            match frame.cursor {
                Some((x, y)) => queue!(self.out, cursor::MoveTo(x, y), cursor::Show)?,
                None => queue!(self.out, cursor::Hide)?,
            }
        }
        self.out.flush()?;
        self.previous = Some(next.clone());
        self.cursor = frame.cursor;
        Ok(())
    }
}

/// The first and last column that differ, widened so neither end falls in
/// the middle of a wide grapheme.
fn changed_span(
    prev: &[crate::surface::Cell],
    next: &[crate::surface::Cell],
) -> Option<(u16, u16)> {
    let mut x0 = prev.iter().zip(next).position(|(a, b)| a != b)?;
    let mut x1 = prev.iter().zip(next).rposition(|(a, b)| a != b)?;
    while x0 > 0 && next[x0].is_continuation() {
        x0 -= 1;
    }
    while x1 + 1 < next.len() && next[x1 + 1].is_continuation() {
        x1 += 1;
    }
    Some((x0 as u16, x1 as u16))
}

fn apply_style(out: &mut Stdout, s: Style) -> io::Result<()> {
    queue!(
        out,
        SetAttribute(Attribute::Reset),
        SetForegroundColor(color(s.fg)),
        SetBackgroundColor(color(s.bg))
    )?;
    if s.bold {
        queue!(out, SetAttribute(Attribute::Bold))?;
    }
    if s.reverse {
        queue!(out, SetAttribute(Attribute::Reverse))?;
    }
    if s.underline {
        queue!(out, SetAttribute(Attribute::Underlined))?;
    }
    Ok(())
}

fn color(c: Color) -> crossterm::style::Color {
    use crossterm::style::Color as C;
    match c {
        Color::Reset => C::Reset,
        Color::Black => C::Black,
        Color::Red => C::DarkRed,
        Color::Green => C::DarkGreen,
        Color::Yellow => C::DarkYellow,
        Color::Blue => C::DarkBlue,
        Color::Magenta => C::DarkMagenta,
        Color::Cyan => C::DarkCyan,
        Color::White => C::White,
        Color::Grey => C::Grey,
    }
}

/// crossterm's event, in the core's vocabulary. `None` for anything the core
/// has no word for: releases, mouse, focus changes, Alt chords.
pub fn normalize(event: event::Event) -> Option<InputEvent> {
    match event {
        event::Event::Key(k) => {
            if k.kind == KeyEventKind::Release {
                return None;
            }
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            let key = match k.code {
                KeyCode::Char(c) if ctrl => Key::Ctrl(c.to_ascii_lowercase()),
                KeyCode::Char(_) if k.modifiers.contains(KeyModifiers::ALT) => return None,
                KeyCode::Char(c) => Key::Char(c),
                KeyCode::Tab if k.modifiers.contains(KeyModifiers::SHIFT) => Key::BackTab,
                KeyCode::Tab => Key::Tab,
                KeyCode::BackTab => Key::BackTab,
                KeyCode::Enter => Key::Enter,
                KeyCode::Esc => Key::Esc,
                KeyCode::Backspace => Key::Backspace,
                KeyCode::Delete => Key::Delete,
                KeyCode::Up => Key::Up,
                KeyCode::Down => Key::Down,
                KeyCode::Left => Key::Left,
                KeyCode::Right => Key::Right,
                KeyCode::Home => Key::Home,
                KeyCode::End => Key::End,
                KeyCode::PageUp => Key::PageUp,
                KeyCode::PageDown => Key::PageDown,
                KeyCode::F(n) => Key::F(n),
                _ => return None,
            };
            Some(InputEvent::Key(key))
        }
        event::Event::Paste(s) => Some(InputEvent::Paste(s)),
        event::Event::Resize(cols, rows) => Some(InputEvent::Resize { cols, rows }),
        _ => None,
    }
}

/// How often the loop looks for changes it did not cause — an agent's action
/// arriving on another thread. Short enough to feel live, long enough to idle.
const POLL: Duration = Duration::from_millis(50);

/// Run the person's side of a shared session until Ctrl-C or Ctrl-Q.
///
/// Redraws whenever the revision moves, whoever moved it, which is how an
/// agent's change shows up on the person's screen without the two sides
/// knowing about each other.
pub fn run(shared: &Shared) -> io::Result<()> {
    let _guard = TerminalGuard::enter()?;
    let mut screen = Screen::new();
    let (mut cols, mut rows) = terminal::size()?;
    let mut drawn: Option<u64> = None;
    loop {
        if event::poll(POLL)? {
            // Drain everything already queued before drawing once: a paste or
            // a held key should cost one frame, not one per event.
            loop {
                if let Some(input) = normalize(event::read()?) {
                    match input {
                        InputEvent::Key(Key::Ctrl('c' | 'q')) => return Ok(()),
                        InputEvent::Resize { cols: c, rows: r } => {
                            (cols, rows) = (c, r);
                            screen.invalidate();
                            drawn = None;
                        }
                        other => {
                            // A refused keystroke changes nothing, and the
                            // application reports anything worth reporting
                            // in its own status line.
                            let _ = shared.lock().input(&other);
                        }
                    }
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        let session = shared.lock();
        if drawn != Some(session.revision()) {
            let frame = render(session.ui(), cols, rows);
            drawn = Some(session.revision());
            drop(session);
            screen.draw(&frame)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::Cell;

    fn row(s: &str) -> Vec<Cell> {
        let mut surface = Surface::new(8, 1);
        surface.put_str(0, 0, s, Style::PLAIN, 8);
        surface.row(0).to_vec()
    }

    #[test]
    fn an_unchanged_row_writes_nothing() {
        assert_eq!(changed_span(&row("abc"), &row("abc")), None);
    }

    #[test]
    fn a_change_never_starts_or_ends_inside_a_wide_grapheme() {
        // "a日b" -> "a本b": the first differing cell is the lead, but the
        // continuation differs too only in content it does not have.
        assert_eq!(changed_span(&row("a日b"), &row("a本b")), Some((1, 2)));
        assert_eq!(changed_span(&row("ab"), &row("a日")), Some((1, 2)));
    }
}
