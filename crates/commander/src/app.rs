//! The commander itself: two directory panes, a status line, and the
//! commands that act between them.
//!
//! Everything here is application meaning — what Enter on a directory does,
//! what F5 copies where. Everything about *how* a list is selected, focused,
//! drawn or reported to an agent comes from hbui, which is the point of the
//! exercise.

use std::fs;
use std::path::{Path, PathBuf};

use hbui_core::{
    Command, Controller, Event, Input, Item, Key, Layout, List, Size, UiState, Widget,
};

pub const LEFT: &str = "left.files";
pub const RIGHT: &str = "right.files";
pub const STATUS: &str = "status";
const PARENT: &str = "..";

/// An operation waiting on its modal.
enum Pending {
    Rename { dir: PathBuf, from: String },
    Mkdir { dir: PathBuf },
    Copy { from: PathBuf, to: PathBuf },
}

pub struct Commander {
    left: PathBuf,
    right: PathBuf,
    pending: Option<Pending>,
}

/// Build the UI and its controller, listing both directories.
pub fn build(left: PathBuf, right: PathBuf) -> (UiState, Commander) {
    let mut ui = UiState::new(Layout::vsplit(vec![
        (
            Size::Fill(1),
            Layout::hsplit(vec![
                (Size::Fill(1), Layout::pane("left", "", LEFT)),
                (Size::Fill(1), Layout::pane("right", "", RIGHT)),
            ]),
        ),
        (Size::Fixed(1), Layout::Widget(STATUS.into())),
    ]))
    .with(LEFT, Widget::List(List::new("Files", Vec::new())))
    .with(RIGHT, Widget::List(List::new("Files", Vec::new())))
    .with(
        STATUS,
        Widget::text("Tab: switch pane  Enter: open  ^Q: quit"),
    );
    ui.commands = vec![
        Command::new("rename", "Rename", Some(Key::F(2))),
        Command::new("copy", "Copy", Some(Key::F(5))),
        Command::new("mkdir", "Mkdir", Some(Key::F(7))),
        Command::new("refresh", "Refresh", Some(Key::Ctrl('r'))),
    ];
    let mut app = Commander {
        left,
        right,
        pending: None,
    };
    app.refresh(&mut ui, LEFT);
    app.refresh(&mut ui, RIGHT);
    (ui, app)
}

impl Commander {
    fn dir(&self, pane: &str) -> &Path {
        if pane == LEFT {
            &self.left
        } else {
            &self.right
        }
    }

    fn other(pane: &str) -> &'static str {
        if pane == LEFT {
            RIGHT
        } else {
            LEFT
        }
    }

    /// The file pane that has focus — or, under a modal, had it.
    fn active_pane(ui: &UiState) -> &'static str {
        match ui.focus().map(|f| f.as_str()) {
            Some(RIGHT) => RIGHT,
            _ => LEFT,
        }
    }

    /// Re-read a pane's directory. The selection is kept by name where the
    /// name still exists, which is what the list's id-based selection buys.
    fn refresh(&mut self, ui: &mut UiState, pane: &str) {
        let dir = self.dir(pane).to_path_buf();
        let items = match list_dir(&dir) {
            Ok(items) => items,
            Err(e) => {
                ui.set_text(STATUS, format!("cannot read {}: {e}", dir.display()));
                Vec::new()
            }
        };
        if let Some(list) = ui.list_mut(pane) {
            list.set_items(items);
        }
        let pane_id = if pane == LEFT { "left" } else { "right" };
        ui.layout.set_pane_title(pane_id, dir.display().to_string());
    }

    fn open(&mut self, ui: &mut UiState, pane: &str, item: &str) -> Result<(), String> {
        let dir = self.dir(pane).to_path_buf();
        let (target, select) = if item == PARENT {
            let parent = dir.parent().ok_or("already at the root")?.to_path_buf();
            // Coming back up, land on the directory we came out of.
            let came_from = dir.file_name().map(|n| n.to_string_lossy().into_owned());
            (parent, came_from)
        } else {
            let path = dir.join(item);
            if !path.is_dir() {
                let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                ui.set_text(STATUS, format!("{item}: {size} bytes"));
                return Ok(());
            }
            (path, None)
        };
        fs::read_dir(&target)
            .map_err(|e| self.fail(ui, format!("cannot open {}: {e}", target.display())))?;
        if pane == LEFT {
            self.left = target;
        } else {
            self.right = target;
        }
        self.refresh(ui, pane);
        if let (Some(name), Some(list)) = (select, ui.list_mut(pane)) {
            list.select(&name);
        }
        Ok(())
    }

    /// Report a failure in the status line — where the person sees it — and
    /// hand the same message back for the agent.
    fn fail(&self, ui: &mut UiState, message: String) -> String {
        ui.set_text(STATUS, &message);
        message
    }

    fn selected_name(ui: &UiState, pane: &str) -> Option<String> {
        match ui.widget(pane)? {
            Widget::List(l) => l.selected.clone().filter(|s| s != PARENT),
            _ => None,
        }
    }

    fn invoke(&mut self, ui: &mut UiState, command: &str) -> Result<(), String> {
        let pane = Self::active_pane(ui);
        let dir = self.dir(pane).to_path_buf();
        match command {
            "refresh" => {
                self.refresh(ui, LEFT);
                self.refresh(ui, RIGHT);
            }
            "rename" => {
                let from = Self::selected_name(ui, pane)
                    .ok_or_else(|| self.fail(ui, "nothing to rename".into()))?;
                self.pending = Some(Pending::Rename {
                    dir,
                    from: from.clone(),
                });
                ui.open_modal(
                    "rename",
                    "Rename",
                    form("rename", &format!("Rename {from} to:"), &from),
                );
            }
            "mkdir" => {
                self.pending = Some(Pending::Mkdir { dir });
                ui.open_modal(
                    "mkdir",
                    "Make directory",
                    form("mkdir", "New directory name:", ""),
                );
            }
            "copy" => {
                let name = Self::selected_name(ui, pane)
                    .ok_or_else(|| self.fail(ui, "nothing to copy".into()))?;
                let from = dir.join(&name);
                if from.is_dir() {
                    return Err(
                        self.fail(ui, format!("{name} is a directory; only files are copied"))
                    );
                }
                let to = self.dir(Self::other(pane)).join(&name);
                let question = format!("Copy {name}\nto {}?", to.parent().unwrap_or(&to).display());
                self.pending = Some(Pending::Copy { from, to });
                ui.open_modal(
                    "copy",
                    "Copy",
                    vec![
                        ("copy.question".into(), Widget::text(question)),
                        ("copy.ok".into(), Widget::button("Copy")),
                        ("copy.cancel".into(), Widget::button("Cancel")),
                    ],
                );
            }
            other => return Err(format!("unhandled command {other}")),
        }
        Ok(())
    }

    /// OK in a modal (or Enter in its input): carry out what it asked.
    fn confirm(&mut self, ui: &mut UiState, modal: &str) -> Result<(), String> {
        let value = ui
            .input(&format!("{modal}.name"))
            .map(|i| i.buffer.text().trim().to_string())
            .unwrap_or_default();
        let Some(pending) = self.pending.take() else {
            ui.close_modal();
            return Ok(());
        };
        let result = match &pending {
            Pending::Rename { dir, from } => valid_name(&value)
                .and_then(|()| move_to(&dir.join(from), &dir.join(&value)).map(|()| value.clone())),
            Pending::Mkdir { dir } => valid_name(&value).and_then(|()| {
                fs::create_dir(dir.join(&value))
                    .map(|()| value.clone())
                    .map_err(|e| e.to_string())
            }),
            Pending::Copy { from, to } => copy_new(from, to).map(|()| name_of(to)),
        };
        match result {
            Ok(name) => {
                ui.close_modal();
                self.refresh(ui, LEFT);
                self.refresh(ui, RIGHT);
                let pane = match &pending {
                    Pending::Copy { .. } => None,
                    _ => Some(Self::active_pane(ui)),
                };
                if let Some(list) = pane.and_then(|p| ui.list_mut(p)) {
                    list.select(&name);
                }
                let done = match pending {
                    Pending::Rename { from, .. } => format!("renamed {from} to {name}"),
                    Pending::Mkdir { .. } => format!("made directory {name}"),
                    Pending::Copy { to, .. } => format!("copied to {}", to.display()),
                };
                ui.set_text(STATUS, done);
                Ok(())
            }
            Err(e) => {
                // Keep the modal open with what was typed, so it can be fixed.
                self.pending = Some(pending);
                Err(self.fail(ui, e))
            }
        }
    }
}

impl Controller for Commander {
    fn handle(&mut self, ui: &mut UiState, event: &Event) -> Result<(), String> {
        match event {
            Event::Activated {
                target,
                item: Some(item),
            } if target == LEFT || target == RIGHT => self.open(ui, target, item),
            Event::Activated { target, .. } => match target.split_once('.') {
                Some((modal, "cancel")) if ui.modal().is_some_and(|m| m.id == modal) => {
                    self.pending = None;
                    ui.close_modal();
                    Ok(())
                }
                Some((modal, "ok" | "name")) if ui.modal().is_some_and(|m| m.id == modal) => {
                    let modal = modal.to_string();
                    self.confirm(ui, &modal)
                }
                _ => Ok(()),
            },
            Event::Invoked { command } => self.invoke(ui, command),
            Event::ModalClosed { .. } => {
                self.pending = None;
                Ok(())
            }
        }
    }
}

/// A prompt, a text field and OK / Cancel.
fn form(id: &str, prompt: &str, value: &str) -> Vec<(hbui_core::WidgetId, Widget)> {
    vec![
        (format!("{id}.prompt").into(), Widget::text(prompt)),
        (
            format!("{id}.name").into(),
            Widget::Input(Input::new("Name", value)),
        ),
        (format!("{id}.ok").into(), Widget::button("OK")),
        (format!("{id}.cancel").into(), Widget::button("Cancel")),
    ]
}

fn list_dir(dir: &Path) -> std::io::Result<Vec<Item>> {
    let mut entries: Vec<(bool, String)> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| {
            (
                e.path().is_dir(),
                e.file_name().to_string_lossy().into_owned(),
            )
        })
        .collect();
    // Directories first, then by name — the order every commander uses.
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut items = Vec::with_capacity(entries.len() + 1);
    if dir.parent().is_some() {
        items.push(Item::new(PARENT, "../"));
    }
    items.extend(entries.into_iter().map(|(is_dir, name)| {
        let label = if is_dir {
            format!("{name}/")
        } else {
            name.clone()
        };
        Item::new(name, label)
    }));
    Ok(items)
}

fn valid_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == PARENT || name.contains(['/', '\\']) {
        return Err(format!("{name:?} is not a file name"));
    }
    Ok(())
}

/// Rename, refusing to overwrite. `fs::rename` would replace silently.
fn move_to(from: &Path, to: &Path) -> Result<(), String> {
    if to.exists() {
        return Err(format!("{} already exists", to.display()));
    }
    fs::rename(from, to).map_err(|e| e.to_string())
}

/// Copy, refusing to overwrite.
fn copy_new(from: &Path, to: &Path) -> Result<(), String> {
    if to.exists() {
        return Err(format!("{} already exists", to.display()));
    }
    fs::copy(from, to).map(|_| ()).map_err(|e| e.to_string())
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hbui_core::{Action, ActionRequest, InputEvent, Session};
    use serde_json::Value;

    /// A scratch directory with `a/` and `b/`, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("hbui-commander-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(p.join("a/sub")).unwrap();
            fs::create_dir_all(p.join("b")).unwrap();
            fs::write(p.join("a/notes.txt"), "hello").unwrap();
            Self(p)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn session(s: &Scratch) -> Session {
        let (ui, app) = build(s.0.join("a"), s.0.join("b"));
        Session::new(ui, app)
    }

    fn act(s: &mut Session, action: Action) -> Result<hbui_core::Outcome, hbui_core::ActionError> {
        let rev = s.revision();
        s.dispatch(ActionRequest {
            action,
            expected_revision: Some(rev),
        })
    }

    fn items(v: &Value, pane: &str) -> Vec<String> {
        v["widgets"][pane]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn lists_directories_first_with_a_parent_entry() {
        let s = Scratch::new("list");
        let v = session(&s).view();
        assert_eq!(items(&v, LEFT), ["..", "sub", "notes.txt"]);
        assert_eq!(v["widgets"][LEFT]["items"][1]["label"], "sub/");
    }

    #[test]
    fn enter_opens_a_directory_and_dot_dot_comes_back_to_it() {
        let s = Scratch::new("nav");
        let mut ses = session(&s);
        act(
            &mut ses,
            Action::Activate {
                target: LEFT.into(),
                item: Some("sub".into()),
            },
        )
        .unwrap();
        assert_eq!(items(&ses.view(), LEFT), [".."]);
        ses.input(&InputEvent::Key(Key::Enter)).unwrap();
        assert_eq!(ses.view()["widgets"][LEFT]["selected"], "sub");
    }

    /// The agent renames through the same modal a person would, with
    /// Japanese in the name.
    #[test]
    fn the_agent_renames_a_file_through_the_modal() {
        let s = Scratch::new("rename");
        let mut ses = session(&s);
        act(
            &mut ses,
            Action::Select {
                target: LEFT.into(),
                item: "notes.txt".into(),
            },
        )
        .unwrap();
        act(
            &mut ses,
            Action::Invoke {
                command: "rename".into(),
            },
        )
        .unwrap();
        assert_eq!(ses.view()["focus"], "rename.name");
        act(
            &mut ses,
            Action::SetText {
                target: "rename.name".into(),
                value: "メモ.txt".into(),
            },
        )
        .unwrap();
        act(
            &mut ses,
            Action::Activate {
                target: "rename.ok".into(),
                item: None,
            },
        )
        .unwrap();

        assert!(s.0.join("a/メモ.txt").exists());
        let v = ses.view();
        assert_eq!(v["modal"], Value::Null);
        assert_eq!(v["widgets"][LEFT]["selected"], "メモ.txt");
        assert_eq!(
            v["widgets"][STATUS]["value"],
            "renamed notes.txt to メモ.txt"
        );
    }

    #[test]
    fn a_refused_rename_keeps_the_modal_open() {
        let s = Scratch::new("refuse");
        let mut ses = session(&s);
        act(
            &mut ses,
            Action::Select {
                target: LEFT.into(),
                item: "notes.txt".into(),
            },
        )
        .unwrap();
        act(
            &mut ses,
            Action::Invoke {
                command: "rename".into(),
            },
        )
        .unwrap();
        act(
            &mut ses,
            Action::SetText {
                target: "rename.name".into(),
                value: "sub".into(),
            },
        )
        .unwrap();
        let err = act(
            &mut ses,
            Action::Activate {
                target: "rename.ok".into(),
                item: None,
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), "rejected");
        assert_eq!(ses.view()["modal"]["id"], "rename");
        assert!(s.0.join("a/notes.txt").exists());
    }

    /// A person starts the copy with F5; the agent confirms it.
    #[test]
    fn copy_asks_first_and_never_overwrites() {
        let s = Scratch::new("copy");
        let mut ses = session(&s);
        ses.input(&InputEvent::Key(Key::End)).unwrap();
        ses.input(&InputEvent::Key(Key::F(5))).unwrap();
        assert_eq!(ses.view()["modal"]["id"], "copy");
        act(
            &mut ses,
            Action::Activate {
                target: "copy.ok".into(),
                item: None,
            },
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(s.0.join("b/notes.txt")).unwrap(),
            "hello"
        );
        assert_eq!(items(&ses.view(), RIGHT), ["..", "notes.txt"]);

        ses.input(&InputEvent::Key(Key::F(5))).unwrap();
        let err = act(
            &mut ses,
            Action::Activate {
                target: "copy.ok".into(),
                item: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
    }

    #[test]
    fn esc_cancels_and_forgets_the_pending_operation() {
        let s = Scratch::new("cancel");
        let mut ses = session(&s);
        ses.input(&InputEvent::Key(Key::F(7))).unwrap();
        ses.input(&InputEvent::Text("new".into())).unwrap();
        ses.input(&InputEvent::Key(Key::Esc)).unwrap();
        assert_eq!(ses.view()["modal"], Value::Null);
        assert!(!s.0.join("a/new").exists());
    }
}
