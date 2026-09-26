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
}
