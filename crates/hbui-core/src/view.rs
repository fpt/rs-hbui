//! The semantic view: what an agent sees instead of a screenshot.
//!
//! It is also the semantic snapshot tests compare, and the thing whose changes
//! define a revision. One function builds it, so those three can never
//! disagree about what "the UI" is.

use serde_json::{json, Map, Value};

use crate::state::{MenuItem, UiState};
use crate::widget::{hotkey, TreeNode, Widget};

/// The view of `ui`, without its revision.
///
/// The revision is left out on purpose: it is a count of changes to *this*,
/// so it cannot be part of what gets compared to decide whether anything
/// changed. [`crate::Session::view`] adds it back.
pub fn view_body(ui: &UiState) -> Value {
    let widgets: Map<String, Value> = ui
        .widgets()
        .iter()
        .map(|(id, w)| (id.to_string(), widget_view(w)))
        .collect();
    let mut v = json!({
        "focus": ui.focus(),
        "modal": ui.modal().map(|m| json!({
            "id": m.id,
            "title": m.title,
            "children": m.children,
        })),
        "layout": ui.layout.to_view(),
        "commands": ui.commands.iter().map(|c| {
            let mut v = labelled(&c.label);
            v["id"] = json!(c.id);
            if let Some(k) = c.key {
                v["key"] = json!(k.to_string());
            }
            if !c.enabled {
                v["enabled"] = json!(false);
            }
            v
        }).collect::<Vec<_>>(),
        "widgets": widgets,
    });
    // Only UIs with a menu bar say anything about menus.
    if !ui.menus.is_empty() {
        v["menus"] = json!(ui
            .menus
            .iter()
            .map(|m| {
                let mut v = labelled(&m.label);
                v["id"] = json!(m.id);
                v["items"] = json!(m
                    .items
                    .iter()
                    .map(|i| match i {
                        MenuItem::Command(id) => id.as_str(),
                        MenuItem::Separator => "-",
                    })
                    .collect::<Vec<_>>());
                v
            })
            .collect::<Vec<_>>());
        v["menu"] = json!(ui.open_menu().map(|o| json!({
            "menu": o.menu,
            "highlighted": o.highlighted,
        })));
    }
    v
}

/// A label as an agent reads it: without the `&` markup, and the hotkey
/// beside it when there is one.
fn labelled(label: &str) -> Value {
    let (shown, key) = hotkey(label);
    let mut v = json!({ "label": shown });
    if let Some(k) = key {
        v["hotkey"] = json!(k.to_string());
    }
    v
}

fn widget_view(w: &Widget) -> Value {
    let mut v = match w {
        Widget::Text(t) => json!({ "value": t.value }),
        Widget::List(l) => json!({
            "label": l.label,
            "selected": l.selected,
            "items": l.items.iter().map(|i| json!({ "id": i.id, "label": i.label })).collect::<Vec<_>>(),
        }),
        Widget::Tree(t) => json!({
            "label": t.label,
            "selected": t.selected,
            "items": t.roots.iter().map(tree_node_view).collect::<Vec<_>>(),
        }),
        Widget::Input(i) => json!({
            "label": i.label,
            "value": i.buffer.text(),
            "cursor": i.buffer.cursor().grapheme,
        }),
        Widget::Button(b) => {
            let mut v = labelled(&b.label);
            if b.default {
                v["default"] = json!(true);
            }
            v
        }
        Widget::Checkbox(c) => {
            let mut v = labelled(&c.label);
            v["checked"] = json!(c.checked);
            v
        }
        Widget::Radio(r) => json!({
            "label": r.label,
            "selected": r.selected,
            "items": r.items.iter().map(|i| json!({ "id": i.id, "label": i.label })).collect::<Vec<_>>(),
        }),
    };
    v["role"] = json!(w.role());
    if !w.actions().is_empty() {
        v["actions"] = json!(w.actions());
    }
    v
}

/// A collapsed node reports how many children it hides rather than listing
/// them: the view describes what is on screen, and an agent that wants them
/// can `expand` — or just `select` one by id, which reveals it.
fn tree_node_view(n: &TreeNode) -> Value {
    let mut v = json!({ "id": n.id, "label": n.label });
    if !n.children.is_empty() {
        v["expanded"] = json!(n.expanded);
        if n.expanded {
            v["children"] = json!(n.children.iter().map(tree_node_view).collect::<Vec<_>>());
        } else {
            v["child_count"] = json!(n.children.len());
        }
    }
    v
}
