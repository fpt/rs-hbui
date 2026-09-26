//! A person's keys, turned into what they mean.
//!
//! Most keys become a semantic [`Action`] — the same one an agent would send —
//! so a keystroke and a tool call reach the state by one road. Down in a list
//! is `select` of the next item's id, not "move the cursor". The exceptions
//! are the ones with no semantic meaning of their own: Tab walking the focus
//! order, and the character-by-character editing of a text field, which an
//! agent does in one `set_text` instead.
//!
//! Priority is an open menu, then the modal, then the focused widget, then
//! the screen: a key the focused widget uses is never also a command.
//!
//! The dialog and menu conventions follow Midnight Commander: F9 pulls the
//! menu bar down; in a dialog, Enter presses the default button from
//! anywhere, arrows walk between widgets the focused one has no use for, and
//! a button's or checkbox's `&` letter presses it — unless the focus is in a
//! text field, where letters are text.

use crate::action::Action;
use crate::input::Key;
use crate::state::{MenuItem, UiState};
use crate::widget::{hotkey, Widget};

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
    /// Navigate the menu bar. Like Tab, this is how a person finds a
    /// command; an agent just `invoke`s it.
    Menu(MenuNav),
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuNav {
    PullDown(String),
    Close,
    /// To the menu this many places along the bar.
    Switch(isize),
    /// The highlight this many items down.
    Move(isize),
}

pub fn translate(ui: &UiState, key: Key) -> Human {
    if ui.open_menu().is_some() {
        return in_menu(ui, key);
    }
    if key == Key::F(9) && ui.modal().is_none() {
        return match ui.menus.first() {
            Some(m) => Human::Menu(MenuNav::PullDown(m.id.clone())),
            None => Human::Ignored,
        };
    }
    if key == Key::Esc && ui.modal().is_some() {
        return Human::Action(Action::CloseModal);
    }
    match key {
        Key::Tab => return Human::FocusBy(1),
        Key::BackTab => return Human::FocusBy(-1),
        _ => {}
    }
    if ui.modal().is_some() {
        if let Some(h) = in_dialog(ui, key) {
            return h;
        }
    }
    if let Some(h) = for_focused(ui, key) {
        return h;
    }
    if ui.modal().is_some() {
        // Arrows the focused widget had no use for walk the dialog.
        return match key {
            Key::Up | Key::Left => Human::FocusBy(-1),
            Key::Down | Key::Right => Human::FocusBy(1),
            _ => Human::Ignored,
        };
    }
    match ui.commands.iter().find(|c| c.key == Some(key)) {
        Some(c) if c.enabled => Human::Action(Action::Invoke {
            command: c.id.clone(),
        }),
        _ => Human::Ignored,
    }
}

/// Keys while a menu is pulled down. The menu takes every key: nothing
/// behind it should react to a key meant for it.
fn in_menu(ui: &UiState, key: Key) -> Human {
    let Some(open) = ui.open_menu() else {
        return Human::Ignored;
    };
    match key {
        Key::Esc | Key::F(9) | Key::F(10) => Human::Menu(MenuNav::Close),
        Key::Left => Human::Menu(MenuNav::Switch(-1)),
        Key::Right => Human::Menu(MenuNav::Switch(1)),
        Key::Up => Human::Menu(MenuNav::Move(-1)),
        Key::Down => Human::Menu(MenuNav::Move(1)),
        Key::Home => Human::Menu(MenuNav::Move(isize::MIN / 2)),
        Key::End => Human::Menu(MenuNav::Move(isize::MAX / 2)),
        Key::Enter => match &open.highlighted {
            Some(command) => Human::Action(Action::Invoke {
                command: command.clone(),
            }),
            None => Human::Ignored,
        },
        Key::Char(c) => {
            let c = c.to_ascii_lowercase();
            let menu = ui.menus.iter().find(|m| m.id == open.menu);
            // An item of this menu first, then another menu of the bar.
            let item = menu
                .into_iter()
                .flat_map(|m| &m.items)
                .find_map(|i| match i {
                    MenuItem::Command(id) => ui
                        .command(id)
                        .filter(|cmd| cmd.enabled && hotkey(&cmd.label).1 == Some(c))
                        .map(|cmd| cmd.id.clone()),
                    MenuItem::Separator => None,
                });
            if let Some(command) = item {
                return Human::Action(Action::Invoke { command });
            }
            match ui.menus.iter().find(|m| hotkey(&m.label).1 == Some(c)) {
                Some(m) => Human::Menu(MenuNav::PullDown(m.id.clone())),
                None => Human::Ignored,
            }
        }
        _ => Human::Ignored,
    }
}

/// Dialog conventions that go before the focused widget's own keys.
fn in_dialog(ui: &UiState, key: Key) -> Option<Human> {
    let modal = ui.modal()?;
    let focused = ui.focus().and_then(|f| ui.widget(f.as_str()));
    let activate = |id: &str| {
        Human::Action(Action::Activate {
            target: id.to_string(),
            item: None,
        })
    };
    match key {
        // Enter on a button presses that button; anywhere else, the default.
        Key::Enter if !matches!(focused, Some(Widget::Button(_))) => {
            let default = modal.children.iter().find(
                |id| matches!(ui.widget(id.as_str()), Some(Widget::Button(b)) if b.default),
            )?;
            Some(activate(default.as_str()))
        }
        Key::Char(c) if !matches!(focused, Some(Widget::Input(_))) => {
            let c = c.to_ascii_lowercase();
            let hit = modal
                .children
                .iter()
                .find(|id| ui.widget(id.as_str()).and_then(Widget::hotkey) == Some(c))?;
            Some(activate(hit.as_str()))
        }
        _ => None,
    }
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
        Widget::Checkbox(_) => matches!(key, Key::Enter | Key::Char(' ')).then(activate),
        Widget::Radio(r) => {
            let ids: Vec<&str> = r.items.iter().map(|i| i.id.as_str()).collect();
            let item = neighbor(&ids, r.selected.as_deref(), delta?)?;
            Some(Human::Action(Action::Select { target, item }))
        }
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
