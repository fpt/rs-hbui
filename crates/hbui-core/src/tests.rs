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

/// Past the kept views, `since` still answers with what changed — merged,
/// and exact as to state.
#[test]
fn get_view_since_merges_beyond_the_kept_views() {
    let mut s = session();
    // An early change that nothing later touches again ...
    s.dispatch(
        Action::Focus {
            target: "right.files".into(),
        }
        .into(),
    )
    .unwrap();
    // ... then enough traffic to push revision 0's view out of history.
    for i in 0..100 {
        s.update(|ui| ui.set_text("status", format!("tick {i}")));
    }
    let v = s.view_since(0);
    assert_eq!(v["merged"], true);
    assert_eq!(v["since"], 0);
    let changes = v["changes"].as_array().unwrap();
    let paths: Vec<&str> = changes
        .iter()
        .map(|c| c["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["/focus", "/widgets/status/value"]);
    assert_eq!(changes[0]["value"], "right.files");
    assert_eq!(changes[1]["value"], "tick 99");
    // A revision that never existed still gets the whole view.
    assert_eq!(s.view_since(10_000)["full"], true);
}

/// Midnight Commander-style menus and dialogs.
mod mc {
    use serde_json::json;

    use crate::*;

    /// A file list, a menu bar, and an Options dialog with a checkbox, a
    /// radio group, an input and OK / Cancel. Copy is disabled while `..` is
    /// selected, which `settle` keeps in step.
    struct App {
        applied: Vec<String>,
    }

    impl Controller for App {
        fn handle(&mut self, ui: &mut UiState, event: &Event) -> Result<(), String> {
            match event {
                Event::Invoked { command } if command == "options" => ui.open_modal(
                    "opts",
                    "Options",
                    vec![
                        (
                            "opts.hidden".into(),
                            Widget::checkbox("Show &hidden", false),
                        ),
                        (
                            "opts.sort".into(),
                            Widget::Radio(RadioGroup::new(
                                "Sort",
                                vec![Item::new("name", "Name"), Item::new("size", "Size")],
                            )),
                        ),
                        ("opts.mask".into(), Widget::Input(Input::new("Mask", "*"))),
                        ("opts.ok".into(), Widget::default_button("&OK")),
                        ("opts.cancel".into(), Widget::button("&Cancel")),
                    ],
                ),
                Event::Activated { target, .. } if target == "opts.ok" => {
                    let hidden =
                        matches!(ui.widget("opts.hidden"), Some(Widget::Checkbox(c)) if c.checked);
                    let sort = match ui.widget("opts.sort") {
                        Some(Widget::Radio(r)) => r.selected.clone().unwrap_or_default(),
                        _ => String::new(),
                    };
                    let mask = ui.input("opts.mask").map(|i| i.buffer.text().to_string());
                    self.applied.push(format!(
                        "hidden={hidden} sort={sort} mask={}",
                        mask.unwrap_or_default()
                    ));
                    ui.close_modal();
                    ui.set_text("status", self.applied.last().unwrap().clone());
                }
                Event::Activated { target, .. } if target == "opts.cancel" => {
                    ui.close_modal();
                }
                Event::Invoked { command } => ui.set_text("status", format!("ran {command}")),
                _ => {}
            }
            Ok(())
        }

        fn settle(&mut self, ui: &mut UiState) {
            let on_parent = matches!(ui.widget("files"), Some(Widget::List(l)) if l.selected.as_deref() == Some(".."));
            ui.set_enabled("copy", !on_parent);
        }
    }

    fn session() -> Session {
        let mut ui = UiState::new(Layout::vsplit(vec![
            (Size::Fill(1), Layout::pane("p", "Files", "files")),
            (Size::Fixed(1), Layout::Widget("status".into())),
        ]))
        .with(
            "files",
            Widget::List(List::new(
                "Files",
                vec![Item::new("..", "../"), Item::new("a.txt", "a.txt")],
            )),
        )
        .with("status", Widget::text(""));
        ui.commands = vec![
            Command::new("copy", "&Copy", Some(Key::F(5))),
            Command::new("mkdir", "&Make directory", Some(Key::F(7))),
            Command::new("options", "&Panel options...", None),
        ];
        ui.menus = vec![
            Menu::new(
                "file",
                "&File",
                vec![
                    MenuItem::Command("copy".into()),
                    MenuItem::Separator,
                    MenuItem::Command("mkdir".into()),
                ],
            ),
            Menu::new(
                "options",
                "&Options",
                vec![MenuItem::Command("options".into())],
            ),
        ];
        Session::new(
            ui,
            App {
                applied: Vec::new(),
            },
        )
    }

    fn key(s: &mut Session, k: Key) {
        s.input(&InputEvent::Key(k)).unwrap();
    }

    #[test]
    fn the_view_lists_menus_and_dims_disabled_commands() {
        let v = session().view();
        assert_eq!(
            v["menus"][0],
            json!({"id": "file", "label": "File", "hotkey": "f", "items": ["copy", "-", "mkdir"]})
        );
        assert_eq!(v["menu"], json!(null));
        // `..` is selected, so settle disabled Copy before the first view.
        assert_eq!(
            v["commands"][0],
            json!({"id": "copy", "label": "Copy", "hotkey": "c", "key": "F5", "enabled": false})
        );
    }

    #[test]
    fn f9_pulls_down_a_menu_that_skips_what_cannot_be_chosen() {
        let mut s = session();
        key(&mut s, Key::F(9));
        // Copy is disabled, so the highlight starts on mkdir.
        assert_eq!(
            s.view()["menu"],
            json!({"menu": "file", "highlighted": "mkdir"})
        );
        key(&mut s, Key::Up);
        assert_eq!(s.view()["menu"]["highlighted"], "mkdir");
        key(&mut s, Key::Right);
        assert_eq!(
            s.view()["menu"],
            json!({"menu": "options", "highlighted": "options"})
        );
        key(&mut s, Key::Right); // wraps
        assert_eq!(s.view()["menu"]["menu"], "file");
        key(&mut s, Key::Enter);
        assert_eq!(s.view()["menu"], json!(null));
        assert_eq!(s.view()["widgets"]["status"]["value"], "ran mkdir");
    }

    #[test]
    fn letters_in_a_menu_choose_items_then_menus() {
        let mut s = session();
        key(&mut s, Key::F(9));
        key(&mut s, Key::Char('o')); // no item "o" in File: the Options menu
        assert_eq!(s.view()["menu"]["menu"], "options");
        key(&mut s, Key::Char('p')); // "&Panel options..."
        assert_eq!(s.view()["modal"]["id"], "opts");
        assert_eq!(
            s.view()["menu"],
            json!(null),
            "the dialog replaced the menu"
        );
    }

    #[test]
    fn disabled_commands_are_refused_and_their_keys_ignored() {
        let mut s = session();
        let err = s
            .dispatch(
                Action::Invoke {
                    command: "copy".into(),
                }
                .into(),
            )
            .unwrap_err();
        assert_eq!(err.code(), "command_disabled");
        key(&mut s, Key::F(5));
        assert_eq!(s.view()["widgets"]["status"]["value"], "");
        // Selecting a file enables it again.
        key(&mut s, Key::Down);
        assert!(s.view()["commands"][0].get("enabled").is_none());
        key(&mut s, Key::F(5));
        assert_eq!(s.view()["widgets"]["status"]["value"], "ran copy");
    }

    #[test]
    fn an_agent_invoking_a_command_puts_the_persons_menu_away() {
        let mut s = session();
        key(&mut s, Key::F(9));
        s.dispatch(
            Action::Invoke {
                command: "mkdir".into(),
            }
            .into(),
        )
        .unwrap();
        assert_eq!(s.view()["menu"], json!(null));
    }

    /// A person fills the dialog with the keys Midnight Commander uses.
    #[test]
    fn a_person_fills_a_dialog() {
        let mut s = session();
        s.dispatch(
            Action::Invoke {
                command: "options".into(),
            }
            .into(),
        )
        .unwrap();
        assert_eq!(s.view()["focus"], "opts.hidden");
        key(&mut s, Key::Char(' ')); // toggle the checkbox
        key(&mut s, Key::Down); // a checkbox has no use for Down: next widget
        assert_eq!(s.view()["focus"], "opts.sort");
        key(&mut s, Key::Down); // a radio group does: next choice
        assert_eq!(s.view()["widgets"]["opts.sort"]["selected"], "size");
        key(&mut s, Key::Tab);
        assert_eq!(s.view()["focus"], "opts.mask");
        key(&mut s, Key::Char('h')); // in a text field, a letter is text
        assert_eq!(s.view()["widgets"]["opts.mask"]["value"], "*h");
        key(&mut s, Key::Enter); // from the input: the default button
        assert_eq!(
            s.view()["widgets"]["status"]["value"],
            "hidden=true sort=size mask=*h"
        );
    }

    #[test]
    fn hotkeys_press_buttons_and_toggle_checkboxes_outside_text_fields() {
        let mut s = session();
        s.dispatch(
            Action::Invoke {
                command: "options".into(),
            }
            .into(),
        )
        .unwrap();
        key(&mut s, Key::Char('h'));
        key(&mut s, Key::Char('h'));
        key(&mut s, Key::Char('h'));
        assert_eq!(s.view()["widgets"]["opts.hidden"]["checked"], true);
        key(&mut s, Key::Char('c'));
        assert_eq!(s.view()["modal"], json!(null));
    }

    /// An agent fills the same dialog with semantic actions only.
    #[test]
    fn an_agent_fills_a_dialog() {
        let mut s = session();
        let mut act = |a: Action| s.dispatch(a.into()).unwrap();
        act(Action::Invoke {
            command: "options".into(),
        });
        act(Action::SetChecked {
            target: "opts.hidden".into(),
            checked: true,
        });
        // Idempotent, unlike a toggle: twice is still on.
        act(Action::SetChecked {
            target: "opts.hidden".into(),
            checked: true,
        });
        act(Action::Select {
            target: "opts.sort".into(),
            item: "size".into(),
        });
        act(Action::SetText {
            target: "opts.mask".into(),
            value: "*.rs".into(),
        });
        act(Action::Activate {
            target: "opts.ok".into(),
            item: None,
        });
        assert_eq!(
            s.view()["widgets"]["status"]["value"],
            "hidden=true sort=size mask=*.rs"
        );
    }

    #[test]
    fn the_dialog_view_names_the_default_button_and_hotkeys() {
        let mut s = session();
        s.dispatch(
            Action::Invoke {
                command: "options".into(),
            }
            .into(),
        )
        .unwrap();
        let w = &s.view()["widgets"];
        assert_eq!(
            w["opts.ok"],
            json!({"role": "button", "label": "OK", "hotkey": "o", "default": true, "actions": ["focus", "activate"]})
        );
        assert_eq!(
            w["opts.hidden"],
            json!({"role": "checkbox", "label": "Show hidden", "hotkey": "h", "checked": false,
                   "actions": ["focus", "set_checked", "activate"]})
        );
        assert_eq!(w["opts.sort"]["role"], "radio");
    }
}
