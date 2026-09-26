//! The human half of hbui: a terminal renderer for a `UiState`.
//!
//! ```text
//! UiState → render → Surface → diff against the last Surface → terminal
//! ```
//!
//! [`render`] and [`Surface`] never touch a terminal, so what a person would
//! see can be checked in a test as text ([`Surface::to_text`]) — the visual
//! snapshot that sits beside the core's semantic one. [`run`] is the part
//! that owns a real terminal.

pub mod render;
pub mod surface;
pub mod terminal;

pub use render::{render, Frame, SAFETY_MARGIN};
pub use surface::{Cell, Color, Rect, Style, Surface};
pub use terminal::{normalize, run, Screen, TerminalGuard};

#[cfg(test)]
mod tests;
