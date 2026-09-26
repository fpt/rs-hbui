//! hbui — a half-breed UI, for a person and an agent at the same time.
//!
//! > Pixels are a rendering detail. The UI is a structured interactive
//! > document.
//!
//! This crate is that document. A [`UiState`] holds every widget, the layout,
//! the focus and the modal; a [`Session`] wraps it with the application's
//! [`Controller`] and a revision history. A person's keys come in through
//! [`Session::input`], an agent's semantic [`Action`]s through
//! [`Session::dispatch`], and both change the same state.
//!
//! Renderers read the state and nothing else: `hbui-terminal` draws it for the
//! person, `hbui-mcp` serves [`Session::view`] to the agent. Neither is a
//! dependency of this crate, and nothing of theirs — no terminal type, no
//! screen coordinate — appears in it.

pub mod action;
pub mod diff;
pub mod input;
pub mod keymap;
pub mod layout;
pub mod session;
pub mod state;
pub mod text;
pub mod view;
pub mod widget;

pub use action::{Action, ActionError, ActionRequest, Event};
pub use diff::{Change, Op};
pub use input::{InputEvent, Key};
pub use layout::{Direction, Layout, Size, Tab};
pub use session::{Controller, NoController, Outcome, Session, Shared};
pub use state::{Command, Modal, UiState};
pub use text::{TextBuffer, TextCursor};
pub use widget::{Button, Input, Item, List, Text, Tree, TreeNode, Widget, WidgetId};

#[cfg(test)]
mod tests;
