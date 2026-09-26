//! Human input, already normalized.
//!
//! A backend (crossterm today) turns whatever its terminal sent into these, and
//! nothing past that point knows which backend it was. This is the only input
//! vocabulary `hbui-core` accepts from a person; an agent does not use it at
//! all, it sends [`crate::Action`]s.

use std::fmt;

use serde::{Deserialize, Serialize};

/// One key, with the modifiers that change its meaning folded in.
///
/// Deliberately small. Shift is folded into the character (`Char('A')`) and
/// into [`Key::BackTab`]; Ctrl only matters for letters, where it is a chord
/// and not a character. Anything else a terminal can report is dropped by the
/// backend rather than represented here and ignored later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

impl fmt::Display for Key {
    /// The label a person reads in a key hint: `F5`, `^Q`, `Enter`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Char(' ') => write!(f, "Space"),
            Key::Char(c) => write!(f, "{c}"),
            Key::Ctrl(c) => write!(f, "^{}", c.to_ascii_uppercase()),
            Key::F(n) => write!(f, "F{n}"),
            other => write!(f, "{other:?}"),
        }
    }
}

/// Everything a backend can hand the core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    Key(Key),
    /// Text committed in one piece — an IME commit, typically. Treated as typed
    /// characters, never as keys, so a committed "q" is not a shortcut.
    Text(String),
    /// Bracketed paste. Kept apart from [`InputEvent::Text`] so an editable
    /// widget can decide what to do with the newlines.
    Paste(String),
    Resize {
        cols: u16,
        rows: u16,
    },
}
