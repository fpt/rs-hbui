//! The semantic view: what an agent sees instead of a screenshot.
//!
//! It is also the semantic snapshot tests compare, and the thing whose changes
//! define a revision. One function builds it, so those three can never
//! disagree about what "the UI" is.

use serde_json::{json, Map, Value};

use crate::state::UiState;
use crate::widget::{TreeNode, Widget};

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
    json!({
        "focus": ui.focus(),
        "modal": ui.modal().map(|m| json!({
            "id": m.id,
            "title": m.title,
            "children": m.children,
        })),
        "layout": ui.layout.to_view(),
        "commands": ui.commands.iter().map(|c| {
            let mut v = json!({ "id": c.id, "label": c.label });
            if let Some(k) = c.key {
                v["key"] = json!(k.to_string());
            }
            v
        }).collect::<Vec<_>>(),
        "widgets": widgets,
    })
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
        Widget::Button(b) => json!({ "label": b.label }),
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
