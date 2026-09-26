//! A person's keys, turned into what they mean.
//!
//! Most keys become a semantic [`Action`] — the same one an agent would send —
//! so a keystroke and a tool call reach the state by one road. Down in a list
//! is `select` of the next item's id, not "move the cursor". The exceptions
//! are the ones with no semantic meaning of their own: Tab walking the focus
//! order, and the character-by-character editing of a text field, which an
//! agent does in one `set_text` instead.
//!
//! Priority is modal, then the focused widget, then the screen: a key the
//! focused widget uses is never also a command.

use crate::action::Action;
use crate::input::Key;
use crate::state::UiState;
use crate::widget::Widget;

/// How far PageUp and PageDown move. The core does not know how tall the list
/// is drawn, and does not need to: a page is a unit of intent, not of pixels.
pub const PAGE: isize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Human {
    Action(Action),
    /// Walk the focus order. Tab is a human navigation device; an agent says
    /// `focus` with the id it wants.
    FocusBy(isize),
    /// Edit the focused input in place.
    Edit(Key),
    Ignored,
}

pub fn translate(ui: &UiState, key: Key) -> Human {
    if key == Key::Esc && ui.modal().is_some() {
        return Human::Action(Action::CloseModal);
    }
    match key {
        Key::Tab => return Human::FocusBy(1),
        Key::BackTab => return Human::FocusBy(-1),
        _ => {}
    }
    if let Some(h) = for_focused(ui, key) {
        return h;
    }
    if ui.modal().is_none() {
        if let Some(c) = ui.commands.iter().find(|c| c.key == Some(key)) {
            return Human::Action(Action::Invoke {
                command: c.id.clone(),
            });
        }
    }
    Human::Ignored
}

fn for_focused(ui: &UiState, key: Key) -> Option<Human> {
    let target = ui.focus()?.to_string();
    let widget = ui.widget(&target)?;
    let activate = || {
        Human::Action(Action::Activate {
            target: target.clone(),
            item: None,
        })
    };
    let delta = match key {
        Key::Up => Some(-1),
        Key::Down => Some(1),
        Key::PageUp => Some(-PAGE),
        Key::PageDown => Some(PAGE),
        Key::Home => Some(isize::MIN / 2),
        Key::End => Some(isize::MAX / 2),
        _ => None,
    };
    match widget {
        Widget::List(l) => {
            if key == Key::Enter {
                return Some(activate());
            }
            let ids: Vec<&str> = l.items.iter().map(|i| i.id.as_str()).collect();
            let item = neighbor(&ids, l.selected.as_deref(), delta?)?;
            Some(Human::Action(Action::Select { target, item }))
        }
        Widget::Tree(t) => {
            let selected = t.selected.as_deref();
            match key {
                Key::Enter => return Some(activate()),
                Key::Right | Key::Left => {
                    let node = t.node(selected?)?;
                    if node.children.is_empty() {
                        return None;
                    }
                    let item = node.id.clone();
                    return Some(Human::Action(if key == Key::Right {
                        Action::Expand { target, item }
                    } else {
                        Action::Collapse { target, item }
                    }));
                }
                _ => {}
            }
            let visible = t.visible();
            let ids: Vec<&str> = visible.iter().map(|(_, n)| n.id.as_str()).collect();
            let item = neighbor(&ids, selected, delta?)?;
            Some(Human::Action(Action::Select { target, item }))
        }
        Widget::Input(_) => match key {
            Key::Enter => Some(activate()),
            Key::Char(_)
            | Key::Backspace
            | Key::Delete
            | Key::Left
            | Key::Right
            | Key::Home
            | Key::End => Some(Human::Edit(key)),
            _ => None,
        },
        Widget::Button(_) => matches!(key, Key::Enter | Key::Char(' ')).then(activate),
        Widget::Text(_) => None,
    }
}

/// The id `delta` rows from `selected`, clamped to the ends. `None` when there
/// is nowhere to go, so a key at the edge is not a no-op action.
fn neighbor(ids: &[&str], selected: Option<&str>, delta: isize) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let at = selected.and_then(|s| ids.iter().position(|i| *i == s));
    let to = match at {
        Some(at) => (at as isize)
            .saturating_add(delta)
            .clamp(0, ids.len() as isize - 1) as usize,
        None => 0,
    };
    (Some(to) != at).then(|| ids[to].to_string())
}
