//! A `Session`: one `UiState`, the application behind it, and the history of
//! views that gives every change a revision.
//!
//! Both halves drive the UI through here. A person's input arrives through
//! [`Session::input`]; an agent's actions through [`Session::dispatch`]. Each
//! ends in [`Session::commit`], which rebuilds the view, and moves the
//! revision only if the view actually changed. That rule is what lets an agent
//! read an empty change list as "that did nothing" rather than guess.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use serde_json::{json, Value};

use crate::action::{self, Action, ActionError, ActionRequest, Event};
use crate::diff::{diff, merge, Change};
use crate::input::{InputEvent, Key};
use crate::keymap::{self, Human, MenuNav};
use crate::state::UiState;
use crate::view::view_body;
use crate::widget::Widget;

/// How many past views are kept for `get_view(since)`. Older than this, the
/// caller gets the whole view again — correct, just larger.
const HISTORY: usize = 64;

/// How many revisions' touched paths are kept, for merged diffs further back
/// than [`HISTORY`]. A revision's paths are a few short strings — a person
/// arrowing through a long list makes one per keystroke — so this can be
/// generous.
const LOG: usize = 4096;

/// The application: whatever gives activation and commands their meaning.
///
/// It may change anything in `ui` — refresh a list, open a modal, write a
/// status line. It returns `Err` to refuse; the message goes back to whoever
/// asked, and any change it made before refusing still stands.
pub trait Controller: Send {
    fn handle(&mut self, ui: &mut UiState, event: &Event) -> Result<(), String>;

    /// Called before every commit, whoever changed what: the place to keep
    /// derived state in step — which commands are enabled for the current
    /// selection, say — since most changes never reach [`Self::handle`].
    fn settle(&mut self, _ui: &mut UiState) {}
}

/// A controller that handles nothing, for UIs that are only widgets.
pub struct NoController;

impl Controller for NoController {
    fn handle(&mut self, _: &mut UiState, _: &Event) -> Result<(), String> {
        Ok(())
    }
}

/// What an operation did to the view.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Outcome {
    pub ok: bool,
    pub revision: u64,
    pub changes: Vec<Change>,
}

pub struct Session {
    ui: UiState,
    controller: Box<dyn Controller>,
    /// `(revision, view body)`, oldest first. Never empty.
    history: VecDeque<(u64, Value)>,
    /// `(revision, paths that revision changed)`, oldest first.
    log: VecDeque<(u64, Vec<String>)>,
}

impl Session {
    pub fn new(mut ui: UiState, controller: impl Controller + 'static) -> Self {
        let mut controller: Box<dyn Controller> = Box::new(controller);
        // Revision 0 is a settled state too.
        controller.settle(&mut ui);
        ui.ensure_focus();
        ui.set_revision(0);
        let body = view_body(&ui);
        Self {
            ui,
            controller,
            history: VecDeque::from([(0, body)]),
            log: VecDeque::new(),
        }
    }

    pub fn ui(&self) -> &UiState {
        &self.ui
    }

    pub fn revision(&self) -> u64 {
        self.ui.revision()
    }

    /// The whole semantic view, with its revision.
    pub fn view(&self) -> Value {
        let (revision, body) = self.history.back().expect("history is never empty");
        let mut v = body.clone();
        v["revision"] = json!(revision);
        v
    }

    /// What changed since `since`.
    ///
    /// Within the last [`HISTORY`] revisions, an exact diff. Further back,
    /// within [`LOG`], a merged diff marked `"merged": true` (see
    /// [`crate::diff::merge`]). Further still, or for a revision that never
    /// existed, the whole view instead, marked `"full": true`.
    pub fn view_since(&self, since: u64) -> Value {
        let current = self.revision();
        let (_, now) = self.history.back().expect("history is never empty");
        if let Some((_, old)) = self.history.iter().find(|(r, _)| *r == since) {
            return json!({ "revision": current, "since": since, "changes": diff(old, now) });
        }
        let logged = self
            .log
            .front()
            .is_some_and(|(first, _)| *first <= since + 1)
            && since < current;
        if logged {
            let touched = self
                .log
                .iter()
                .filter(|(r, _)| *r > since)
                .flat_map(|(_, paths)| paths.iter().map(String::as_str));
            return json!({
                "revision": current,
                "since": since,
                "merged": true,
                "changes": merge(touched, now),
            });
        }
        let mut v = self.view();
        v["full"] = json!(true);
        v
    }

    /// Run an agent's action.
    ///
    /// Refused without effect when `expected_revision` is stale. Otherwise the
    /// change is committed even if the action or the application failed
    /// partway, so the revision always describes the state as it really is.
    pub fn dispatch(&mut self, req: ActionRequest) -> Result<Outcome, ActionError> {
        if let Some(expected) = req.expected_revision {
            if expected != self.revision() {
                return Err(ActionError::StaleRevision {
                    current_revision: self.revision(),
                });
            }
        }
        let result = self.run(&req.action);
        let outcome = self.commit();
        result.map(|()| outcome)
    }

    /// Feed one normalized input event from a person.
    pub fn input(&mut self, event: &InputEvent) -> Result<Outcome, ActionError> {
        let result = match event {
            InputEvent::Key(key) => self.key(*key),
            InputEvent::Text(text) | InputEvent::Paste(text) => {
                if let Some(Widget::Input(input)) = self.focused_mut() {
                    input.buffer.insert(text);
                    Ok(())
                } else {
                    // A one-character commit outside a text field is a key
                    // press that went through the IME; anything longer has
                    // nowhere to go.
                    let mut chars = text.chars();
                    match (chars.next(), chars.next(), event) {
                        (Some(c), None, InputEvent::Text(_)) => self.key(Key::Char(c)),
                        _ => Ok(()),
                    }
                }
            }
            InputEvent::Resize { .. } => Ok(()),
        };
        let outcome = self.commit();
        result.map(|()| outcome)
    }

    /// Change the state from the application's side — a timer, a file
    /// watcher — and commit it like any other change.
    pub fn update(&mut self, f: impl FnOnce(&mut UiState)) -> Outcome {
        f(&mut self.ui);
        self.commit()
    }

    fn key(&mut self, key: Key) -> Result<(), ActionError> {
        match keymap::translate(&self.ui, key) {
            Human::Action(action) => self.run(&action),
            Human::FocusBy(delta) => {
                self.ui.focus_by(delta);
                Ok(())
            }
            Human::Edit(key) => {
                if let Some(Widget::Input(input)) = self.focused_mut() {
                    let b = &mut input.buffer;
                    match key {
                        Key::Char(c) => b.insert(c.encode_utf8(&mut [0; 4])),
                        Key::Backspace => b.backspace(),
                        Key::Delete => b.delete(),
                        Key::Left => b.left(),
                        Key::Right => b.right(),
                        Key::Home => b.home(),
                        Key::End => b.end(),
                        _ => {}
                    }
                }
                Ok(())
            }
            Human::Menu(nav) => {
                match nav {
                    MenuNav::PullDown(id) => {
                        self.ui.pull_down(&id);
                    }
                    MenuNav::Close => self.ui.close_menu(),
                    MenuNav::Switch(delta) => self.ui.switch_menu(delta),
                    MenuNav::Move(delta) => self.ui.move_highlight(delta),
                }
                Ok(())
            }
            Human::Ignored => Ok(()),
        }
    }

    fn focused_mut(&mut self) -> Option<&mut Widget> {
        let id = self.ui.focus()?.to_string();
        self.ui.widget_mut(&id)
    }

    fn run(&mut self, action: &Action) -> Result<(), ActionError> {
        if let Some(event) = action::apply(&mut self.ui, action)? {
            self.controller
                .handle(&mut self.ui, &event)
                .map_err(|message| ActionError::Rejected { message })?;
        }
        Ok(())
    }

    /// Settle focus, rebuild the view, and advance the revision if and only
    /// if the view changed.
    fn commit(&mut self) -> Outcome {
        self.controller.settle(&mut self.ui);
        self.ui.ensure_focus();
        let body = view_body(&self.ui);
        let (last_rev, last) = self.history.back().expect("history is never empty");
        let changes = diff(last, &body);
        let mut revision = *last_rev;
        if !changes.is_empty() {
            revision += 1;
            self.log
                .push_back((revision, changes.iter().map(|c| c.path.clone()).collect()));
            if self.log.len() > LOG {
                self.log.pop_front();
            }
            self.history.push_back((revision, body));
            if self.history.len() > HISTORY {
                self.history.pop_front();
            }
        }
        self.ui.set_revision(revision);
        Outcome {
            ok: true,
            revision,
            changes,
        }
    }
}

/// A session shared between the person's event loop and the agent's server.
///
/// A plain mutex, held for one operation at a time: an action and a
/// keystroke never interleave, so there is never a question of which one a
/// revision describes.
#[derive(Clone)]
pub struct Shared(Arc<Mutex<Session>>);

impl Shared {
    pub fn new(session: Session) -> Self {
        Self(Arc::new(Mutex::new(session)))
    }

    /// A poisoned lock is recovered rather than propagated: one panicking
    /// request must not take the person's UI down with it.
    pub fn lock(&self) -> MutexGuard<'_, Session> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}
