//! Session-level behaviour: the promises the design makes to an agent.

use serde_json::json;

use crate::*;

/// Two lists and a status line; Enter on an item writes it to the status,
/// and the "rename" command opens a modal.
struct Demo;

impl Controller for Demo {
    fn handle(&mut self, ui: &mut UiState, event: &Event) -> Result<(), String> {
        match event {
            Event::Activated { target, item }
                if target == "rename.name" || target == "rename.ok" =>
            {
                let name = ui.input("rename.name").map(|i| i.buffer.text().to_string());
                ui.close_modal();
                ui.set_text("status", format!("renamed to {}", name.unwrap_or_default()));
                let _ = item;
            }
            Event::Activated { target, item } => {
                ui.set_text(
                    "status",
                    format!("opened {} in {target}", item.as_deref().unwrap_or("-")),
                );
            }
            Event::Invoked { command } if command == "rename" => ui.open_modal(
                "rename",
                "Rename",
                vec![
                    (
                        "rename.name".into(),
                        Widget::Input(Input::new("New name", "old.txt")),
                    ),
                    ("rename.ok".into(), Widget::button("OK")),
                ],
            ),
            Event::Invoked { command } if command == "fail" => return Err("that went wrong".into()),
            _ => {}
        }
        Ok(())
    }
}

fn session() -> Session {
    let ui = UiState::new(Layout::vsplit(vec![
        (
            Size::Fill(1),
            Layout::hsplit(vec![
                (Size::Fill(1), Layout::pane("left", "Left", "left.files")),
                (Size::Fill(1), Layout::pane("right", "Right", "right.files")),
            ]),
        ),
        (Size::Fixed(1), Layout::Widget("status".into())),
    ]))
    .with(
        "left.files",
        Widget::List(List::new(
            "Files",
            vec![
                Item::new("root", "/root"),
                Item::new("documents", "/root/Documents"),
                Item::new("download", "/root/Download"),
            ],
        )),
    )
    .with(
        "right.files",
        Widget::List(List::new("Files", vec![Item::new("temp", "/temp")])),
    )
    .with("status", Widget::text(""));
    let mut ui = ui;
    ui.commands = vec![
        Command::new("rename", "Rename", Some(Key::F(2))),
        Command::new("fail", "Fail", None),
    ];
    Session::new(ui, Demo)
}

fn select(target: &str, item: &str) -> Action {
    Action::Select {
        target: target.into(),
        item: item.into(),
    }
}

/// The semantic snapshot: exactly what an agent reads on its first look.
#[test]
fn semantic_snapshot_of_the_initial_view() {
    let v = session().view();
    assert_eq!(v["revision"], 0);
    assert_eq!(v["focus"], "left.files");
    assert_eq!(v["modal"], json!(null));
    assert_eq!(
        v["widgets"]["left.files"],
        json!({
            "role": "list",
            "label": "Files",
            "selected": "root",
            "items": [
                {"id": "root", "label": "/root"},
                {"id": "documents", "label": "/root/Documents"},
                {"id": "download", "label": "/root/Download"},
            ],
            "actions": ["focus", "select", "activate"],
        })
    );
    assert_eq!(
        v["layout"]["children"][0]["children"][1],
        json!({"pane": "right", "title": "Right", "content": "right.files"})
    );
    assert_eq!(
        v["commands"][0],
        json!({"id": "rename", "label": "Rename", "key": "F2"})
    );
}

#[test]
fn an_action_reports_exactly_what_it_changed() {
    let mut s = session();
    let out = s
        .dispatch(select("left.files", "documents").into())
        .unwrap();
    assert_eq!(out.revision, 1);
    assert_eq!(
        serde_json::to_value(&out.changes).unwrap(),
        json!([{"op": "replace", "path": "/widgets/left.files/selected", "value": "documents"}])
    );
}

#[test]
fn an_action_that_changes_nothing_does_not_move_the_revision() {
    let mut s = session();
    let out = s.dispatch(select("left.files", "root").into()).unwrap();
    assert_eq!(out.revision, 0);
    assert!(out.changes.is_empty());
}

#[test]
fn a_stale_revision_is_refused_without_effect() {
    let mut s = session();
    // The agent looks at revision 0 ...
    let seen = s.revision();
    // ... the person presses Down in the meantime ...
    s.input(&InputEvent::Key(Key::Down)).unwrap();
    // ... and the agent's action, decided on the old view, is refused.
    let err = s
        .dispatch(ActionRequest {
            action: select("left.files", "download"),
            expected_revision: Some(seen),
        })
        .unwrap_err();
    assert_eq!(
        err,
        ActionError::StaleRevision {
            current_revision: 1
        }
    );
    assert_eq!(s.view()["widgets"]["left.files"]["selected"], "documents");
}

#[test]
fn people_and_agents_take_turns_on_one_state() {
    let mut s = session();
    s.input(&InputEvent::Key(Key::Tab)).unwrap();
    assert_eq!(s.view()["focus"], "right.files");

    s.dispatch(
        Action::Focus {
            target: "left.files".into(),
        }
        .into(),
    )
    .unwrap();
    s.input(&InputEvent::Key(Key::End)).unwrap();
    assert_eq!(s.view()["widgets"]["left.files"]["selected"], "download");

    let out = s
        .dispatch(ActionRequest {
            action: Action::Activate {
                target: "left.files".into(),
                item: None,
            },
            expected_revision: Some(s.revision()),
        })
        .unwrap();
    assert_eq!(
        s.view()["widgets"]["status"]["value"],
        "opened download in left.files"
    );
    assert_eq!(out.changes.len(), 1, "{:?}", out.changes);
}

#[test]
fn get_view_since_returns_the_changes_in_between() {
    let mut s = session();
    s.dispatch(select("left.files", "documents").into())
        .unwrap();
    s.dispatch(select("left.files", "download").into()).unwrap();
    assert_eq!(
        s.view_since(0),
        json!({
            "revision": 2,
            "since": 0,
            "changes": [{"op": "replace", "path": "/widgets/left.files/selected", "value": "download"}],
        })
    );
    assert_eq!(s.view_since(2)["changes"], json!([]));
    assert_eq!(s.view_since(999)["full"], true);
}

#[test]
fn a_modal_is_the_whole_input_context() {
    let mut s = session();
    s.input(&InputEvent::Key(Key::F(2))).unwrap();
    let v = s.view();
    assert_eq!(v["modal"]["id"], "rename");
    assert_eq!(v["focus"], "rename.name");

    // The screen behind it cannot be operated ...
    let err = s
        .dispatch(select("left.files", "download").into())
        .unwrap_err();
    assert_eq!(err.code(), "blocked_by_modal");
    // ... and neither can commands.
    let err = s
        .dispatch(
            Action::Invoke {
                command: "rename".into(),
            }
            .into(),
        )
        .unwrap_err();
    assert_eq!(err.code(), "blocked_by_modal");

    s.input(&InputEvent::Key(Key::Esc)).unwrap();
    assert_eq!(s.view()["modal"], json!(null));
    assert_eq!(s.view()["focus"], "left.files");
}

/// A person types into the rename field — including a Japanese IME commit —
/// and the agent reads back the same text the person sees.
#[test]
fn typed_and_committed_text_land_in_the_same_input() {
    let mut s = session();
    s.input(&InputEvent::Key(Key::F(2))).unwrap();
    for _ in 0..4 {
        s.input(&InputEvent::Key(Key::Backspace)).unwrap(); // "old.txt" -> "old"
    }
    s.input(&InputEvent::Key(Key::Home)).unwrap();
    s.input(&InputEvent::Text("新しい".into())).unwrap();
    s.input(&InputEvent::Key(Key::Char('_'))).unwrap();
    let input = &s.view()["widgets"]["rename.name"];
    assert_eq!(input["value"], "新しい_old");
    assert_eq!(input["cursor"], 4);

    s.input(&InputEvent::Key(Key::Enter)).unwrap();
    assert_eq!(
        s.view()["widgets"]["status"]["value"],
        "renamed to 新しい_old"
    );
    assert_eq!(s.view()["modal"], json!(null));
}

#[test]
fn the_agent_types_with_set_text() {
    let mut s = session();
    s.dispatch(
        Action::Invoke {
            command: "rename".into(),
        }
        .into(),
    )
    .unwrap();
    s.dispatch(
        Action::SetText {
            target: "rename.name".into(),
            value: "abc.txt".into(),
        }
        .into(),
    )
    .unwrap();
    s.dispatch(
        Action::Activate {
            target: "rename.ok".into(),
            item: None,
        }
        .into(),
    )
    .unwrap();
    assert_eq!(s.view()["widgets"]["status"]["value"], "renamed to abc.txt");
}

#[test]
fn errors_name_the_problem() {
    let mut s = session();
    let e = |s: &mut Session, a: Action| s.dispatch(a.into()).unwrap_err().code();
    assert_eq!(e(&mut s, select("nope", "x")), "unknown_target");
    assert_eq!(e(&mut s, select("left.files", "nope")), "unknown_item");
    assert_eq!(e(&mut s, select("status", "x")), "not_supported");
    assert_eq!(
        e(
            &mut s,
            Action::Invoke {
                command: "nope".into()
            }
        ),
        "unknown_command"
    );
    assert_eq!(e(&mut s, Action::CloseModal), "no_modal");
    let err = s
        .dispatch(
            Action::Invoke {
                command: "fail".into(),
            }
            .into(),
        )
        .unwrap_err();
    assert_eq!(err.to_json()["message"], "that went wrong");
}
