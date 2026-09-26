//! What changed between two views, as a list of path operations.
//!
//! The paths are JSON-Pointer shaped, with one difference that makes them
//! worth reading: **an array whose elements all carry a unique `id` is
//! addressed by id, not by index.** So a selection change is
//! `/widgets/left.files/selected`, and a renamed file is
//! `/widgets/left.files/items/notes.txt/label` — a path that says what
//! changed rather than where it happened to sit.
//!
//! An array whose ids were added, removed or reordered is replaced whole.
//! Positional add/remove ops against an id-keyed path would be ambiguous, and
//! a directory listing that changed is honestly described as "replaced".

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Change {
    pub op: Op,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Add,
    Remove,
    Replace,
}

pub fn diff(old: &Value, new: &Value) -> Vec<Change> {
    let mut out = Vec::new();
    walk(old, new, &mut String::new(), &mut out);
    out
}

fn walk(old: &Value, new: &Value, path: &mut String, out: &mut Vec<Change>) {
    if old == new {
        return;
    }
    match (old, new) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, va) in a {
                let len = push(path, k);
                match b.get(k) {
                    Some(vb) => walk(va, vb, path, out),
                    None => out.push(change(Op::Remove, path, None)),
                }
                path.truncate(len);
            }
            for (k, vb) in b {
                if !a.contains_key(k) {
                    let len = push(path, k);
                    out.push(change(Op::Add, path, Some(vb)));
                    path.truncate(len);
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => match (ids(a), ids(b)) {
            (Some(ia), Some(ib)) if ia == ib => {
                for ((id, va), vb) in ia.iter().zip(a).zip(b) {
                    let len = push(path, id);
                    walk(va, vb, path, out);
                    path.truncate(len);
                }
            }
            (None, None) if a.len() == b.len() => {
                for (i, (va, vb)) in a.iter().zip(b).enumerate() {
                    let len = push(path, &i.to_string());
                    walk(va, vb, path, out);
                    path.truncate(len);
                }
            }
            _ => out.push(change(Op::Replace, path, Some(new))),
        },
        _ => out.push(change(Op::Replace, path, Some(new))),
    }
}

/// The element ids, if every element is an object with a distinct string id.
fn ids(items: &[Value]) -> Option<Vec<&str>> {
    let ids: Vec<&str> = items
        .iter()
        .map(|v| v.get("id").and_then(Value::as_str))
        .collect::<Option<_>>()?;
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    (sorted.len() == ids.len()).then_some(ids)
}

/// Append one escaped segment, returning the length to truncate back to.
fn push(path: &mut String, segment: &str) -> usize {
    let len = path.len();
    path.push('/');
    path.push_str(&segment.replace('~', "~0").replace('/', "~1"));
    len
}

fn change(op: Op, path: &str, value: Option<&Value>) -> Change {
    Change {
        op,
        path: if path.is_empty() {
            "/".into()
        } else {
            path.to_string()
        },
        value: value.cloned(),
    }
}

/// The changes since some old view, built from the paths each revision in
/// between touched and the view as it is now — for when the old view itself
/// is no longer kept.
///
/// Exact as to state: every path whose value may differ from the old view is
/// reported with its value now, or removed if it no longer exists. What it
/// cannot tell is whether a path is new, so it only ever says `replace` or
/// `remove`. Paths under another touched path are folded into it.
pub fn merge<'a>(touched: impl IntoIterator<Item = &'a str>, now: &Value) -> Vec<Change> {
    let mut paths: Vec<&str> = touched.into_iter().collect();
    paths.sort_unstable();
    paths.dedup();
    let covered = |p: &str, by: &str| by == "/" || p == by || p.starts_with(&format!("{by}/"));
    let kept: Vec<&str> = paths
        .iter()
        .copied()
        .filter(|p| !paths.iter().any(|q| q != p && covered(p, q)))
        .collect();
    kept.into_iter()
        .map(|p| match resolve(now, p) {
            Some(v) => change(Op::Replace, p, Some(v)),
            None => change(Op::Remove, p, None),
        })
        .collect()
}

/// Follow one of this module's paths into a view: object keys by name, and
/// arrays by element id where the elements have ids, by index otherwise.
pub fn resolve<'v>(view: &'v Value, path: &str) -> Option<&'v Value> {
    if path == "/" || path.is_empty() {
        return Some(view);
    }
    let mut at = view;
    for raw in path.strip_prefix('/')?.split('/') {
        let seg = raw.replace("~1", "/").replace("~0", "~");
        at = match at {
            Value::Object(o) => o.get(&seg)?,
            Value::Array(items) => match ids(items) {
                Some(ids) => &items[ids.iter().position(|id| *id == seg)?],
                None => items.get(seg.parse::<usize>().ok()?)?,
            },
            _ => return None,
        };
    }
    Some(at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identical_views_have_no_changes() {
        let v = json!({"a": [1, 2], "b": {"c": null}});
        assert!(diff(&v, &v).is_empty());
    }

    #[test]
    fn id_keyed_arrays_are_addressed_by_id() {
        let a = json!({"items": [{"id": "x", "label": "X"}, {"id": "y", "label": "Y"}]});
        let b = json!({"items": [{"id": "x", "label": "X"}, {"id": "y", "label": "Why"}]});
        assert_eq!(
            diff(&a, &b),
            [Change {
                op: Op::Replace,
                path: "/items/y/label".into(),
                value: Some(json!("Why"))
            }]
        );
    }

    #[test]
    fn a_changed_id_set_replaces_the_array() {
        let a = json!({"items": [{"id": "x"}]});
        let b = json!({"items": [{"id": "x"}, {"id": "z"}]});
        let d = diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "/items");
        assert_eq!(d[0].op, Op::Replace);
    }

    #[test]
    fn keys_are_added_removed_and_escaped() {
        let a = json!({"w": {"a/b": 1, "gone": 2}});
        let b = json!({"w": {"a/b": 3, "new~": 4}});
        let paths: Vec<(Op, String)> = diff(&a, &b).into_iter().map(|c| (c.op, c.path)).collect();
        assert_eq!(
            paths,
            [
                (Op::Replace, "/w/a~1b".to_string()),
                (Op::Remove, "/w/gone".to_string()),
                (Op::Add, "/w/new~0".to_string()),
            ]
        );
    }

    #[test]
    fn a_merged_diff_reports_current_values_for_every_touched_path() {
        let now = json!({"w": {"list": {"selected": "c", "items": [{"id": "a", "label": "A2"}]}}});
        let touched = [
            "/w/list/selected",
            "/w/list/selected",
            "/w/list/items/a/label",
            "/w/gone",
            "/w/list/items/a", // covers the label path above
        ];
        let merged = diff_paths(merge(touched, &now));
        assert_eq!(
            merged,
            [
                (Op::Remove, "/w/gone".to_string(), None),
                (
                    Op::Replace,
                    "/w/list/items/a".to_string(),
                    Some(json!({"id": "a", "label": "A2"}))
                ),
                (
                    Op::Replace,
                    "/w/list/selected".to_string(),
                    Some(json!("c"))
                ),
            ]
        );
    }

    #[test]
    fn a_merged_diff_equals_the_real_one_in_effect() {
        // Three revisions: the middle change is undone by nothing later, so a
        // diff from the newest kept view alone would miss it.
        let v0 = json!({"a": 1, "b": {"x": 1}, "items": [{"id": "p", "n": 1}]});
        let v1 = json!({"a": 2, "b": {"x": 1}, "items": [{"id": "p", "n": 1}]});
        let v2 = json!({"a": 2, "b": {"x": 5}, "items": [{"id": "p", "n": 9}]});
        let touched: Vec<String> = diff(&v0, &v1)
            .into_iter()
            .chain(diff(&v1, &v2))
            .map(|c| c.path)
            .collect();
        let merged = merge(touched.iter().map(String::as_str), &v2);
        let mut exact = diff(&v0, &v2);
        exact.sort_by(|x, y| x.path.cmp(&y.path));
        assert_eq!(merged, exact);
    }

    fn diff_paths(changes: Vec<Change>) -> Vec<(Op, String, Option<Value>)> {
        changes
            .into_iter()
            .map(|c| (c.op, c.path, c.value))
            .collect()
    }
}
