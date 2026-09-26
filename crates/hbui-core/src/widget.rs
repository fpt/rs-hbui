//! The widget set: text, list, tree, input, button — and nothing else yet.
//!
//! Every widget is plain data. None of them knows where it is on a screen, how
//! tall it is, or which row is scrolled into view; those are answers the
//! renderer computes every frame from what is here. What *is* here is exactly
//! what an agent needs to operate the widget without seeing it: stable item
//! ids, the selection, the text and the cursor.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::text::TextBuffer;

/// A widget's stable name, like `left.files` or `rename.name`.
///
/// Stable is the point. An agent addresses widgets by id and never by
/// position, so an id must survive re-layout, resizing and re-rendering, and
/// mean the same thing from one revision to the next.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WidgetId(pub String);

impl WidgetId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WidgetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for WidgetId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for WidgetId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl PartialEq<str> for WidgetId {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for WidgetId {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

/// One row of a [`List`]: an id to address it by and a label to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub label: String,
}

impl Item {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// A flat, single-selection list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct List {
    pub label: String,
    pub items: Vec<Item>,
    /// The selected item's **id**, never its index: an index would silently
    /// point at a different item after the list is refreshed.
    pub selected: Option<String>,
}

impl List {
    pub fn new(label: impl Into<String>, items: Vec<Item>) -> Self {
        let selected = items.first().map(|i| i.id.clone());
        Self {
            label: label.into(),
            items,
            selected,
        }
    }

    pub fn selected_index(&self) -> Option<usize> {
        let id = self.selected.as_deref()?;
        self.items.iter().position(|i| i.id == id)
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.selected_index().map(|i| &self.items[i])
    }

    /// Select by id. `false` when there is no such item, and nothing changes.
    pub fn select(&mut self, id: &str) -> bool {
        if self.items.iter().any(|i| i.id == id) {
            self.selected = Some(id.to_string());
            true
        } else {
            false
        }
    }

    /// Move the selection by `delta` rows, clamped at both ends.
    pub fn move_by(&mut self, delta: isize) {
        if self.items.is_empty() {
            self.selected = None;
            return;
        }
        let at = self.selected_index().unwrap_or(0) as isize;
        let to = (at + delta).clamp(0, self.items.len() as isize - 1) as usize;
        self.selected = Some(self.items[to].id.clone());
    }

    /// Replace the items, keeping the selection if its id survived and
    /// falling back to the first item if it did not.
    pub fn set_items(&mut self, items: Vec<Item>) {
        self.items = items;
        let keep = self
            .selected
            .as_deref()
            .is_some_and(|id| self.items.iter().any(|i| i.id == id));
        if !keep {
            self.selected = self.items.first().map(|i| i.id.clone());
        }
    }
}

/// One node of a [`Tree`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub id: String,
    pub label: String,
    pub children: Vec<TreeNode>,
    pub expanded: bool,
}

impl TreeNode {
    pub fn leaf(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            children: Vec::new(),
            expanded: false,
        }
    }

    pub fn branch(
        id: impl Into<String>,
        label: impl Into<String>,
        children: Vec<TreeNode>,
    ) -> Self {
        Self {
            children,
            ..Self::leaf(id, label)
        }
    }
}

/// A single-selection tree. Ids must be unique across the whole tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree {
    pub label: String,
    pub roots: Vec<TreeNode>,
    pub selected: Option<String>,
}

impl Tree {
    pub fn new(label: impl Into<String>, roots: Vec<TreeNode>) -> Self {
        let selected = roots.first().map(|n| n.id.clone());
        Self {
            label: label.into(),
            roots,
            selected,
        }
    }

    /// The nodes a person can see, in order, with their depth.
    pub fn visible(&self) -> Vec<(usize, &TreeNode)> {
        fn walk<'a>(nodes: &'a [TreeNode], depth: usize, out: &mut Vec<(usize, &'a TreeNode)>) {
            for n in nodes {
                out.push((depth, n));
                if n.expanded {
                    walk(&n.children, depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.roots, 0, &mut out);
        out
    }

    /// Select any node by id, expanding its ancestors so it is visible.
    ///
    /// An agent may name a node it has never seen on screen; revealing it is
    /// what a person would have had to do by hand to select it.
    pub fn select(&mut self, id: &str) -> bool {
        fn reveal(nodes: &mut [TreeNode], id: &str) -> bool {
            for n in nodes {
                if n.id == id {
                    return true;
                }
                if reveal(&mut n.children, id) {
                    n.expanded = true;
                    return true;
                }
            }
            false
        }
        if reveal(&mut self.roots, id) {
            self.selected = Some(id.to_string());
            true
        } else {
            false
        }
    }

    /// Expand or collapse a node. Collapsing a node that hides the selection
    /// moves the selection onto it, so the selection never becomes invisible.
    pub fn set_expanded(&mut self, id: &str, expanded: bool) -> bool {
        let Some(node) = find_mut(&mut self.roots, id) else {
            return false;
        };
        node.expanded = expanded;
        if !expanded {
            let hides_selection = self
                .selected
                .as_deref()
                .is_some_and(|sel| sel != id && find(&node.children, sel).is_some());
            if hides_selection {
                self.selected = Some(id.to_string());
            }
        }
        true
    }

    pub fn node(&self, id: &str) -> Option<&TreeNode> {
        find(&self.roots, id)
    }

    /// Move the selection by `delta` visible rows, clamped at both ends.
    pub fn move_by(&mut self, delta: isize) {
        let ids: Vec<String> = self.visible().iter().map(|(_, n)| n.id.clone()).collect();
        if ids.is_empty() {
            self.selected = None;
            return;
        }
        let at = self
            .selected
            .as_deref()
            .and_then(|s| ids.iter().position(|i| i == s))
            .unwrap_or(0) as isize;
        let to = (at + delta).clamp(0, ids.len() as isize - 1) as usize;
        self.selected = Some(ids[to].clone());
    }
}

fn find<'a>(nodes: &'a [TreeNode], id: &str) -> Option<&'a TreeNode> {
    nodes.iter().find_map(|n| {
        if n.id == id {
            Some(n)
        } else {
            find(&n.children, id)
        }
    })
}

fn find_mut<'a>(nodes: &'a mut [TreeNode], id: &str) -> Option<&'a mut TreeNode> {
    for n in nodes {
        if n.id == id {
            return Some(n);
        }
        if let Some(found) = find_mut(&mut n.children, id) {
            return Some(found);
        }
    }
    None
}

/// A one-line text field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    pub label: String,
    pub buffer: TextBuffer,
}

impl Input {
    pub fn new(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            buffer: TextBuffer::new(text),
        }
    }
}

/// Something to press. What pressing it does is the application's business;
/// the core only reports that it was pressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    /// May mark a hotkey with `&`, as in `&OK`.
    pub label: String,
    /// The dialog's default: Enter anywhere in the modal presses it, as in
    /// Midnight Commander. Drawn `[< OK >]`.
    pub default: bool,
}

/// An on/off option. Drawn `[x] label`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checkbox {
    /// May mark a hotkey with `&`, as in `Show &hidden files`.
    pub label: String,
    pub checked: bool,
}

/// One choice out of several, all visible. Drawn `(*) item`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RadioGroup {
    pub label: String,
    pub items: Vec<Item>,
    /// The chosen item's id.
    pub selected: Option<String>,
}

impl RadioGroup {
    pub fn new(label: impl Into<String>, items: Vec<Item>) -> Self {
        let selected = items.first().map(|i| i.id.clone());
        Self {
            label: label.into(),
            items,
            selected,
        }
    }

    pub fn select(&mut self, id: &str) -> bool {
        if self.items.iter().any(|i| i.id == id) {
            self.selected = Some(id.to_string());
            true
        } else {
            false
        }
    }
}

/// Split a label into what is shown and its hotkey: `&Copy` is shown `Copy`
/// with hotkey `c`, and `&&` is a literal `&`. The hotkey is lowercase.
pub fn hotkey(label: &str) -> (String, Option<char>) {
    let mut shown = String::with_capacity(label.len());
    let mut key = None;
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            match chars.next() {
                Some('&') => shown.push('&'),
                Some(k) => {
                    if key.is_none() {
                        key = Some(k.to_ascii_lowercase());
                    }
                    shown.push(k);
                }
                None => {}
            }
        } else {
            shown.push(c);
        }
    }
    (shown, key)
}

/// Where the hotkey sits in the shown label, in chars — for the renderer to
/// mark it.
pub fn hotkey_index(label: &str) -> Option<usize> {
    let mut shown = 0;
    let mut chars = label.chars();
    while let Some(c) = chars.next() {
        if c == '&' {
            match chars.next() {
                Some('&') => shown += 1,
                Some(_) => return Some(shown),
                None => return None,
            }
        } else {
            shown += 1;
        }
    }
    None
}

/// Read-only text, possibly several lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Text {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Widget {
    Text(Text),
    List(List),
    Tree(Tree),
    Input(Input),
    Button(Button),
    Checkbox(Checkbox),
    Radio(RadioGroup),
}

impl Widget {
    pub fn text(value: impl Into<String>) -> Self {
        Widget::Text(Text {
            value: value.into(),
        })
    }

    pub fn button(label: impl Into<String>) -> Self {
        Widget::Button(Button {
            label: label.into(),
            default: false,
        })
    }

    /// The button Enter presses from anywhere in its modal.
    pub fn default_button(label: impl Into<String>) -> Self {
        Widget::Button(Button {
            label: label.into(),
            default: true,
        })
    }

    pub fn checkbox(label: impl Into<String>, checked: bool) -> Self {
        Widget::Checkbox(Checkbox {
            label: label.into(),
            checked,
        })
    }

    /// The `&` hotkey of a button or checkbox, if it has one.
    pub fn hotkey(&self) -> Option<char> {
        match self {
            Widget::Button(Button { label, .. }) | Widget::Checkbox(Checkbox { label, .. }) => {
                hotkey(label).1
            }
            _ => None,
        }
    }

    /// The `role` an agent sees.
    pub fn role(&self) -> &'static str {
        match self {
            Widget::Text(_) => "text",
            Widget::List(_) => "list",
            Widget::Tree(_) => "tree",
            Widget::Input(_) => "input",
            Widget::Button(_) => "button",
            Widget::Checkbox(_) => "checkbox",
            Widget::Radio(_) => "radio",
        }
    }

    /// Whether focus can land here. Text is only ever read.
    pub fn focusable(&self) -> bool {
        !matches!(self, Widget::Text(_))
    }

    /// The semantic actions this widget accepts, as advertised in the view.
    pub fn actions(&self) -> &'static [&'static str] {
        match self {
            Widget::Text(_) => &[],
            Widget::List(_) => &["focus", "select", "activate"],
            Widget::Tree(_) => &["focus", "select", "activate", "expand", "collapse"],
            Widget::Input(_) => &["focus", "set_text", "activate"],
            Widget::Button(_) => &["focus", "activate"],
            Widget::Checkbox(_) => &["focus", "set_checked", "activate"],
            Widget::Radio(_) => &["focus", "select"],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Tree {
        Tree::new(
            "Files",
            vec![
                TreeNode::branch(
                    "root",
                    "/root",
                    vec![
                        TreeNode::branch("docs", "Documents", vec![TreeNode::leaf("a", "a.txt")]),
                        TreeNode::leaf("dl", "Downloads"),
                    ],
                ),
                TreeNode::leaf("tmp", "/tmp"),
            ],
        )
    }

    #[test]
    fn selecting_a_hidden_node_reveals_it() {
        let mut t = tree();
        assert_eq!(t.visible().len(), 2);
        assert!(t.select("a"));
        let ids: Vec<&str> = t.visible().iter().map(|(_, n)| n.id.as_str()).collect();
        assert_eq!(ids, ["root", "docs", "a", "dl", "tmp"]);
    }

    #[test]
    fn collapsing_over_the_selection_moves_it_to_the_collapsed_node() {
        let mut t = tree();
        t.select("a");
        t.set_expanded("root", false);
        assert_eq!(t.selected.as_deref(), Some("root"));
    }

    #[test]
    fn list_selection_survives_a_refresh_by_id() {
        let mut l = List::new("x", vec![Item::new("a", "A"), Item::new("b", "B")]);
        l.select("b");
        l.set_items(vec![Item::new("z", "Z"), Item::new("b", "B")]);
        assert_eq!(l.selected.as_deref(), Some("b"));
        l.set_items(vec![Item::new("q", "Q")]);
        assert_eq!(l.selected.as_deref(), Some("q"));
    }

    #[test]
    fn moving_clamps_at_both_ends() {
        let mut l = List::new("x", vec![Item::new("a", "A"), Item::new("b", "B")]);
        l.move_by(-5);
        assert_eq!(l.selected.as_deref(), Some("a"));
        l.move_by(5);
        assert_eq!(l.selected.as_deref(), Some("b"));
    }

    #[test]
    fn hotkeys_are_marked_with_an_ampersand() {
        assert_eq!(hotkey("&Copy"), ("Copy".into(), Some('c')));
        assert_eq!(hotkey("Show &Hidden"), ("Show Hidden".into(), Some('h')));
        assert_eq!(hotkey("Save && quit"), ("Save & quit".into(), None));
        assert_eq!(hotkey("plain"), ("plain".into(), None));
        assert_eq!(hotkey_index("Show &Hidden"), Some(5));
        assert_eq!(hotkey_index("A && &B"), Some(4));
    }
}
