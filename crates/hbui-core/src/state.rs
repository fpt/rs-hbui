//! `UiState`: the single source of truth.
//!
//! Every renderer reads this and nothing else, and so does the agent's view.
//! Nothing a terminal knows — where its cursor is, what it last drew — is
//! allowed to be the answer to a question about the UI. If it matters, it is
//! here; if it is not here, it is a rendering detail.

use std::collections::BTreeMap;

use crate::action::ActionError;
use crate::input::Key;
use crate::layout::Layout;
use crate::widget::{Input, List, Tree, Widget, WidgetId};

/// A screen-level command, like "Copy" on F5.
///
/// Listed in the view so an agent can `invoke` it by id, and drawn in the key
/// bar so a person can press it. Both read the same list, so the key bar
/// cannot advertise a command the agent does not have, or the other way round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: String,
    pub label: String,
    pub key: Option<Key>,
}

impl Command {
    pub fn new(id: impl Into<String>, label: impl Into<String>, key: Option<Key>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            key,
        }
    }
}

/// The one dialog that may sit on top of the screen.
///
/// While it is open, it is the whole input context: focus stays inside it and
/// actions aimed at anything behind it are refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Modal {
    pub id: String,
    pub title: String,
    /// Its widgets, top to bottom. They live in the ordinary widget map for as
    /// long as the modal is open, and are removed when it closes.
    pub children: Vec<WidgetId>,
    return_focus: Option<WidgetId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiState {
    revision: u64,
    pub layout: Layout,
    widgets: BTreeMap<WidgetId, Widget>,
    focus: Option<WidgetId>,
    modal: Option<Modal>,
    pub commands: Vec<Command>,
}

impl UiState {
    pub fn new(layout: Layout) -> Self {
        Self {
            revision: 0,
            layout,
            widgets: BTreeMap::new(),
            focus: None,
            modal: None,
            commands: Vec::new(),
        }
    }

    /// How many times the view has changed. Only the session moves it, and
    /// only when something an observer could see actually changed.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }

    pub fn insert(&mut self, id: impl Into<WidgetId>, widget: Widget) {
        self.widgets.insert(id.into(), widget);
    }

    pub fn with(mut self, id: impl Into<WidgetId>, widget: Widget) -> Self {
        self.insert(id, widget);
        self
    }

    pub fn widgets(&self) -> &BTreeMap<WidgetId, Widget> {
        &self.widgets
    }

    pub fn widget(&self, id: &str) -> Option<&Widget> {
        self.widgets.get(&WidgetId::from(id))
    }

    pub fn widget_mut(&mut self, id: &str) -> Option<&mut Widget> {
        self.widgets.get_mut(&WidgetId::from(id))
    }

    pub fn list_mut(&mut self, id: &str) -> Option<&mut List> {
        match self.widget_mut(id)? {
            Widget::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn tree_mut(&mut self, id: &str) -> Option<&mut Tree> {
        match self.widget_mut(id)? {
            Widget::Tree(t) => Some(t),
            _ => None,
        }
    }

    pub fn input(&self, id: &str) -> Option<&Input> {
        match self.widget(id)? {
            Widget::Input(i) => Some(i),
            _ => None,
        }
    }

    /// Replace a text widget's contents — the usual way to report status.
    pub fn set_text(&mut self, id: &str, value: impl Into<String>) {
        self.insert(id, Widget::text(value));
    }

    pub fn focus(&self) -> Option<&WidgetId> {
        self.focus.as_ref()
    }

    pub fn modal(&self) -> Option<&Modal> {
        self.modal.as_ref()
    }

    /// The widgets focus may move between right now: the modal's if one is
    /// open, otherwise the visible layout's. Never anything else.
    pub fn focus_order(&self) -> Vec<WidgetId> {
        let candidates: Vec<&WidgetId> = match &self.modal {
            Some(m) => m.children.iter().collect(),
            None => self.layout.visible_widgets(),
        };
        candidates
            .into_iter()
            .filter(|id| self.widgets.get(*id).is_some_and(Widget::focusable))
            .cloned()
            .collect()
    }

    /// Whether `id` is in the current input context — reachable without
    /// closing a modal or switching a tab.
    pub fn is_reachable(&self, id: &str) -> bool {
        match &self.modal {
            Some(m) => m.children.iter().any(|c| c == id),
            None => self.layout.visible_widgets().iter().any(|c| *c == id),
        }
    }

    pub fn set_focus(&mut self, id: &str) -> Result<(), ActionError> {
        let widget = self
            .widget(id)
            .ok_or_else(|| ActionError::unknown_target(id))?;
        if !widget.focusable() {
            return Err(ActionError::NotSupported {
                target: id.to_string(),
                action: "focus".into(),
                role: widget.role().into(),
            });
        }
        self.check_reachable(id)?;
        self.focus = Some(id.into());
        Ok(())
    }

    /// Move focus `delta` places around the focus order, wrapping.
    pub fn focus_by(&mut self, delta: isize) {
        let order = self.focus_order();
        if order.is_empty() {
            self.focus = None;
            return;
        }
        let at = self
            .focus
            .as_ref()
            .and_then(|f| order.iter().position(|o| o == f))
            .unwrap_or(0) as isize;
        let to = (at + delta).rem_euclid(order.len() as isize) as usize;
        self.focus = Some(order[to].clone());
    }

    /// Put focus somewhere legal if it is not already: after a modal closed,
    /// a tab switched, or a focused widget was removed.
    pub(crate) fn ensure_focus(&mut self) {
        let order = self.focus_order();
        let ok = self.focus.as_ref().is_some_and(|f| order.contains(f));
        if !ok {
            self.focus = order.into_iter().next();
        }
    }

    pub(crate) fn check_reachable(&self, id: &str) -> Result<(), ActionError> {
        if self.widget(id).is_none() {
            return Err(ActionError::unknown_target(id));
        }
        if self.is_reachable(id) {
            return Ok(());
        }
        match &self.modal {
            Some(m) => Err(ActionError::BlockedByModal {
                modal: m.id.clone(),
            }),
            None => Err(ActionError::NotVisible { target: id.into() }),
        }
    }

    /// Open a modal with these widgets, focusing the first one that can take
    /// focus. Replaces any modal already open.
    pub fn open_modal(
        &mut self,
        id: impl Into<String>,
        title: impl Into<String>,
        widgets: Vec<(WidgetId, Widget)>,
    ) {
        let return_focus = match self.modal.take() {
            Some(old) => {
                for c in &old.children {
                    self.widgets.remove(c);
                }
                old.return_focus
            }
            None => self.focus.clone(),
        };
        let children = widgets.iter().map(|(id, _)| id.clone()).collect();
        for (wid, w) in widgets {
            self.widgets.insert(wid, w);
        }
        self.modal = Some(Modal {
            id: id.into(),
            title: title.into(),
            children,
            return_focus,
        });
        self.focus = None;
        self.ensure_focus();
    }

    /// Close the modal, drop its widgets and give focus back to where it was.
    pub fn close_modal(&mut self) -> Option<Modal> {
        let modal = self.modal.take()?;
        for c in &modal.children {
            self.widgets.remove(c);
        }
        self.focus = modal.return_focus.clone();
        self.ensure_focus();
        Some(modal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Size;
    use crate::widget::Item;

    fn ui() -> UiState {
        UiState::new(Layout::hsplit(vec![
            (Size::Fill(1), Layout::pane("left", "Left", "left.files")),
            (Size::Fill(1), Layout::pane("right", "Right", "right.files")),
        ]))
        .with(
            "left.files",
            Widget::List(List::new("L", vec![Item::new("a", "a")])),
        )
        .with(
            "right.files",
            Widget::List(List::new("R", vec![Item::new("b", "b")])),
        )
    }

    #[test]
    fn a_modal_captures_focus_and_gives_it_back() {
        let mut ui = ui();
        ui.set_focus("right.files").unwrap();
        ui.open_modal(
            "confirm",
            "Sure?",
            vec![
                ("confirm.msg".into(), Widget::text("Sure?")),
                ("confirm.ok".into(), Widget::button("OK")),
            ],
        );
        assert_eq!(ui.focus().unwrap(), "confirm.ok");
        assert_eq!(
            ui.set_focus("left.files"),
            Err(ActionError::BlockedByModal {
                modal: "confirm".into()
            })
        );
        ui.close_modal();
        assert_eq!(ui.focus().unwrap(), "right.files");
        assert!(ui.widget("confirm.ok").is_none());
    }

    #[test]
    fn focus_wraps_around() {
        let mut ui = ui();
        ui.ensure_focus();
        assert_eq!(ui.focus().unwrap(), "left.files");
        ui.focus_by(1);
        ui.focus_by(1);
        assert_eq!(ui.focus().unwrap(), "left.files");
        ui.focus_by(-1);
        assert_eq!(ui.focus().unwrap(), "right.files");
    }
}
