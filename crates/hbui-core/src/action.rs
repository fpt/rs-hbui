//! Semantic actions: how an agent operates the UI.
//!
//! An agent never presses keys and never clicks coordinates. It says what it
//! means — "select `documents` in `left.files`" — against a widget id it read
//! from the view. A person's keystrokes are translated into the same actions
//! wherever one exists (see [`crate::keymap`]), so both end up on one code
//! path, changing one state.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::state::UiState;
use crate::widget::Widget;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Move focus to a widget.
    Focus {
        target: String,
    },
    /// Select an item of a list or tree, or a tab of a tab set.
    Select {
        target: String,
        item: String,
    },
    /// Do the widget's primary thing: open the selected item, press the
    /// button, submit the input. With `item`, select it first.
    Activate {
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item: Option<String>,
    },
    /// Set a checkbox on or off. Unlike `activate`, which toggles, this
    /// says what the agent wants, so sending it twice does no harm.
    SetChecked {
        target: String,
        checked: bool,
    },
    /// Replace an input's whole text. The agent's way to type.
    SetText {
        target: String,
        value: String,
    },
    Expand {
        target: String,
        item: String,
    },
    Collapse {
        target: String,
        item: String,
    },
    /// Run a screen-level command by id, as its key would.
    Invoke {
        command: String,
    },
    /// Dismiss the open modal without doing what it asked.
    CloseModal,
}

/// An action as it arrives from outside, with the revision the sender was
/// looking at when it decided on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRequest {
    #[serde(flatten)]
    pub action: Action,
    /// When present and not the current revision, the action is refused as
    /// [`ActionError::StaleRevision`] — something (usually the person at the
    /// keyboard) changed the UI since the sender last looked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
}

impl From<Action> for ActionRequest {
    fn from(action: Action) -> Self {
        Self {
            action,
            expected_revision: None,
        }
    }
}

/// What the core hands the application when an action needs a decision only
/// the application can make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A list or tree item was opened, a button pressed, or an input
    /// submitted. `item` is the list or tree item, if there is one.
    Activated {
        target: String,
        item: Option<String>,
    },
    Invoked {
        command: String,
    },
    /// The modal was dismissed (Esc, or `close_modal`). Already closed by the
    /// time the application hears about it.
    ModalClosed {
        modal: String,
    },
}

/// Why an action was refused. Serialized for the agent with a stable `error`
/// code it can branch on, and a message it can read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionError {
    StaleRevision {
        current_revision: u64,
    },
    UnknownTarget {
        target: String,
    },
    UnknownItem {
        target: String,
        item: String,
    },
    UnknownCommand {
        command: String,
    },
    NotSupported {
        target: String,
        action: String,
        role: String,
    },
    /// The target exists but is not on screen — behind an inactive tab.
    NotVisible {
        target: String,
    },
    BlockedByModal {
        modal: String,
    },
    NoModal,
    CommandDisabled {
        command: String,
    },
    /// The application looked at the request and said no.
    Rejected {
        message: String,
    },
}

impl ActionError {
    pub fn unknown_target(target: &str) -> Self {
        ActionError::UnknownTarget {
            target: target.to_string(),
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            ActionError::StaleRevision { .. } => "stale_revision",
            ActionError::UnknownTarget { .. } => "unknown_target",
            ActionError::UnknownItem { .. } => "unknown_item",
            ActionError::UnknownCommand { .. } => "unknown_command",
            ActionError::NotSupported { .. } => "not_supported",
            ActionError::NotVisible { .. } => "not_visible",
            ActionError::BlockedByModal { .. } => "blocked_by_modal",
            ActionError::NoModal => "no_modal",
            ActionError::CommandDisabled { .. } => "command_disabled",
            ActionError::Rejected { .. } => "rejected",
        }
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({ "ok": false, "error": self.code(), "message": self.to_string() });
        match self {
            ActionError::StaleRevision { current_revision } => {
                v["current_revision"] = json!(current_revision);
            }
            ActionError::BlockedByModal { modal } => v["modal"] = json!(modal),
            _ => {}
        }
        v
    }
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionError::StaleRevision { current_revision } => write!(
                f,
                "the view changed since you read it (now revision {current_revision}); \
                 read it again before acting"
            ),
            ActionError::UnknownTarget { target } => write!(f, "no widget with id {target:?}"),
            ActionError::UnknownItem { target, item } => {
                write!(f, "{target:?} has no item {item:?}")
            }
            ActionError::UnknownCommand { command } => write!(f, "no command {command:?}"),
            ActionError::NotSupported {
                target,
                action,
                role,
            } => {
                write!(
                    f,
                    "{target:?} is a {role}, which does not accept {action:?}"
                )
            }
            ActionError::NotVisible { target } => {
                write!(f, "{target:?} is not on screen; switch to its tab first")
            }
            ActionError::BlockedByModal { modal } => {
                write!(
                    f,
                    "modal {modal:?} is open; act inside it or close it first"
                )
            }
            ActionError::NoModal => write!(f, "no modal is open"),
            ActionError::CommandDisabled { command } => {
                write!(f, "command {command:?} is disabled right now")
            }
            ActionError::Rejected { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for ActionError {}

/// Apply one action to the state. Returns the event the application must
/// handle, if the action needs one.
///
/// Widget-level effects — selection, text, focus, expansion — happen here, so
/// every application gets them the same way. Anything that means something
/// only to the application comes back as an [`Event`].
pub(crate) fn apply(ui: &mut UiState, action: &Action) -> Result<Option<Event>, ActionError> {
    match action {
        Action::Focus { target } => ui.set_focus(target).map(|()| None),

        Action::Select { target, item } => {
            if let Some((tabs, active)) = ui.layout.tabs_mut(target) {
                let at = tabs.iter().position(|t| t.id == *item).ok_or_else(|| {
                    ActionError::UnknownItem {
                        target: target.clone(),
                        item: item.clone(),
                    }
                })?;
                *active = at;
                return Ok(None);
            }
            ui.check_reachable(target)?;
            let found = match widget_mut(ui, target)? {
                Widget::List(l) => l.select(item),
                Widget::Tree(t) => t.select(item),
                Widget::Radio(r) => r.select(item),
                other => return Err(unsupported(target, "select", other)),
            };
            if !found {
                return Err(ActionError::UnknownItem {
                    target: target.clone(),
                    item: item.clone(),
                });
            }
            ui.set_focus(target)?;
            Ok(None)
        }

        Action::Activate { target, item } => {
            if let Some(item) = item {
                apply(
                    ui,
                    &Action::Select {
                        target: target.clone(),
                        item: item.clone(),
                    },
                )?;
            }
            ui.check_reachable(target)?;
            let item = match widget_mut(ui, target)? {
                Widget::List(l) => l.selected.clone(),
                Widget::Tree(t) => t.selected.clone(),
                Widget::Input(_) | Widget::Button(_) => None,
                // Activating a checkbox toggles it, as Space does. Its value
                // is read when its dialog is confirmed, so there is no event.
                Widget::Checkbox(c) => {
                    c.checked = !c.checked;
                    ui.set_focus(target)?;
                    return Ok(None);
                }
                other => return Err(unsupported(target, "activate", other)),
            };
            ui.set_focus(target)?;
            Ok(Some(Event::Activated {
                target: target.clone(),
                item,
            }))
        }

        Action::SetChecked { target, checked } => {
            ui.check_reachable(target)?;
            match widget_mut(ui, target)? {
                Widget::Checkbox(c) => c.checked = *checked,
                other => return Err(unsupported(target, "set_checked", other)),
            }
            ui.set_focus(target)?;
            Ok(None)
        }

        Action::SetText { target, value } => {
            ui.check_reachable(target)?;
            match widget_mut(ui, target)? {
                Widget::Input(i) => i.buffer.set(value),
                other => return Err(unsupported(target, "set_text", other)),
            }
            ui.set_focus(target)?;
            Ok(None)
        }

        Action::Expand { target, item } | Action::Collapse { target, item } => {
            let expand = matches!(action, Action::Expand { .. });
            ui.check_reachable(target)?;
            let name = if expand { "expand" } else { "collapse" };
            let found = match widget_mut(ui, target)? {
                Widget::Tree(t) => t.set_expanded(item, expand),
                other => return Err(unsupported(target, name, other)),
            };
            if !found {
                return Err(ActionError::UnknownItem {
                    target: target.clone(),
                    item: item.clone(),
                });
            }
            Ok(None)
        }

        Action::Invoke { command } => {
            let Some(c) = ui.command(command) else {
                return Err(ActionError::UnknownCommand {
                    command: command.clone(),
                });
            };
            if !c.enabled {
                return Err(ActionError::CommandDisabled {
                    command: command.clone(),
                });
            }
            if let Some(m) = ui.modal() {
                return Err(ActionError::BlockedByModal {
                    modal: m.id.clone(),
                });
            }
            // Choosing a command, from its menu or anywhere else, puts any
            // pulled-down menu away.
            ui.close_menu();
            Ok(Some(Event::Invoked {
                command: command.clone(),
            }))
        }

        Action::CloseModal => {
            let modal = ui.close_modal().ok_or(ActionError::NoModal)?;
            Ok(Some(Event::ModalClosed { modal: modal.id }))
        }
    }
}

fn widget_mut<'a>(ui: &'a mut UiState, target: &str) -> Result<&'a mut Widget, ActionError> {
    ui.widget_mut(target)
        .ok_or_else(|| ActionError::unknown_target(target))
}

fn unsupported(target: &str, action: &str, widget: &Widget) -> ActionError {
    ActionError::NotSupported {
        target: target.to_string(),
        action: action.to_string(),
        role: widget.role().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_parse_in_the_shape_the_design_uses() {
        let r: ActionRequest = serde_json::from_value(json!({
            "type": "select",
            "target": "left.files",
            "item": "documents",
            "expected_revision": 42
        }))
        .unwrap();
        assert_eq!(r.expected_revision, Some(42));
        assert_eq!(
            r.action,
            Action::Select {
                target: "left.files".into(),
                item: "documents".into()
            }
        );
        let r: ActionRequest = serde_json::from_value(json!({"type": "close_modal"})).unwrap();
        assert_eq!(r.action, Action::CloseModal);
    }

    #[test]
    fn a_stale_revision_error_says_where_the_view_is_now() {
        let v = ActionError::StaleRevision {
            current_revision: 43,
        }
        .to_json();
        assert_eq!(v["error"], "stale_revision");
        assert_eq!(v["current_revision"], 43);
    }
}
