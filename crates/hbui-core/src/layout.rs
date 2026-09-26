//! Where widgets go, without saying where on a screen.
//!
//! Four shapes and no others: a horizontal split, a vertical split, tabs, and
//! (in [`crate::state`]) one modal on top. No floating windows and no absolute
//! coordinates. That restriction is what makes "what is the user looking at,
//! and what will a key press go to?" a question with one answer — for a person
//! glancing at the screen and for an agent reading the view.

use serde::Serialize;
use serde_json::{json, Value};

use crate::widget::WidgetId;

/// Which way a split lays its children out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Side by side, left to right.
    Horizontal,
    /// Stacked, top to bottom.
    Vertical,
}

/// How much of a split one child gets.
///
/// Measured in rows or columns for the terminal; a GUI renderer is free to
/// read `Fixed` as "this many lines of text".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Fixed(u16),
    /// A share of whatever the fixed children left over.
    Fill(u16),
}

/// One tab of a [`Layout::Tabs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub id: String,
    pub title: String,
    pub content: Layout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layout {
    Split {
        direction: Direction,
        children: Vec<(Size, Layout)>,
    },
    /// A bordered, titled frame around one widget. The unit of "which part of
    /// the screen is focused".
    Pane {
        id: String,
        title: String,
        content: WidgetId,
    },
    /// Only the active tab is shown, and only its widgets can take focus.
    /// Switched with `select`, targeting the tabs' id with the tab's id.
    Tabs {
        id: String,
        tabs: Vec<Tab>,
        active: usize,
    },
    /// A widget with no frame, like a status line.
    Widget(WidgetId),
}

impl Layout {
    pub fn hsplit(children: Vec<(Size, Layout)>) -> Self {
        Layout::Split {
            direction: Direction::Horizontal,
            children,
        }
    }

    pub fn vsplit(children: Vec<(Size, Layout)>) -> Self {
        Layout::Split {
            direction: Direction::Vertical,
            children,
        }
    }

    pub fn pane(
        id: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<WidgetId>,
    ) -> Self {
        Layout::Pane {
            id: id.into(),
            title: title.into(),
            content: content.into(),
        }
    }

    /// The widgets on screen, in reading order — the order Tab walks.
    pub fn visible_widgets(&self) -> Vec<&WidgetId> {
        let mut out = Vec::new();
        self.walk_visible(&mut out);
        out
    }

    fn walk_visible<'a>(&'a self, out: &mut Vec<&'a WidgetId>) {
        match self {
            Layout::Split { children, .. } => {
                for (_, child) in children {
                    child.walk_visible(out);
                }
            }
            Layout::Pane { content, .. } | Layout::Widget(content) => out.push(content),
            Layout::Tabs { tabs, active, .. } => {
                if let Some(tab) = tabs.get(*active) {
                    tab.content.walk_visible(out);
                }
            }
        }
    }

    /// Find a tab set by id, to switch its active tab.
    pub fn tabs_mut(&mut self, id: &str) -> Option<(&mut Vec<Tab>, &mut usize)> {
        match self {
            Layout::Tabs {
                id: this,
                tabs,
                active,
            } => {
                if this == id {
                    Some((tabs, active))
                } else {
                    tabs.iter_mut().find_map(|t| t.content.tabs_mut(id))
                }
            }
            Layout::Split { children, .. } => children.iter_mut().find_map(|(_, c)| c.tabs_mut(id)),
            _ => None,
        }
    }

    /// Retitle a pane — to show the directory a file list is in, say.
    pub fn set_pane_title(&mut self, id: &str, new: impl Into<String>) -> bool {
        fn find<'a>(l: &'a mut Layout, id: &str) -> Option<&'a mut String> {
            match l {
                Layout::Pane {
                    id: this, title, ..
                } if this == id => Some(title),
                Layout::Pane { .. } | Layout::Widget(_) => None,
                Layout::Split { children, .. } => {
                    children.iter_mut().find_map(|(_, c)| find(c, id))
                }
                Layout::Tabs { tabs, .. } => tabs.iter_mut().find_map(|t| find(&mut t.content, id)),
            }
        }
        match find(self, id) {
            Some(title) => {
                *title = new.into();
                true
            }
            None => false,
        }
    }

    /// The layout as an agent reads it: the structure and the ids, no sizes
    /// in cells, because there are none to report.
    pub fn to_view(&self) -> Value {
        match self {
            Layout::Split {
                direction,
                children,
            } => json!({
                "split": direction,
                "children": children.iter().map(|(_, c)| c.to_view()).collect::<Vec<_>>(),
            }),
            Layout::Pane { id, title, content } => json!({
                "pane": id,
                "title": title,
                "content": content,
            }),
            Layout::Tabs { id, tabs, active } => json!({
                "tabs": id,
                "active": tabs.get(*active).map(|t| t.id.as_str()),
                "items": tabs.iter().map(|t| json!({
                    "id": t.id,
                    "title": t.title,
                    "content": t.content.to_view(),
                })).collect::<Vec<_>>(),
            }),
            Layout::Widget(id) => json!({ "widget": id }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_active_tab_is_visible() {
        let mut l = Layout::vsplit(vec![
            (
                Size::Fill(1),
                Layout::Tabs {
                    id: "tabs".into(),
                    tabs: vec![
                        Tab {
                            id: "one".into(),
                            title: "One".into(),
                            content: Layout::Widget("a".into()),
                        },
                        Tab {
                            id: "two".into(),
                            title: "Two".into(),
                            content: Layout::Widget("b".into()),
                        },
                    ],
                    active: 0,
                },
            ),
            (Size::Fixed(1), Layout::Widget("status".into())),
        ]);
        assert_eq!(
            l.visible_widgets(),
            [&WidgetId::from("a"), &"status".into()]
        );
        *l.tabs_mut("tabs").unwrap().1 = 1;
        assert_eq!(
            l.visible_widgets(),
            [&WidgetId::from("b"), &"status".into()]
        );
    }
}
