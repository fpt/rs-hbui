---
name: hbui-building
description: Write a user interface with hbui (rs-hbui) — lay out widgets, give commands and menus meaning in a Controller, open dialogs, and open a session so an agent can drive it over MCP. Use when creating or changing a Rust program's UI with hbui-core / hbui-terminal / hbui-ipc, not when operating a running hbui app (that is hbui-driving).
---

# Building a UI with hbui

hbui is a UI that a person and an agent use at the same time. You do not draw
screens. You build a `UiState` — widgets with stable ids, a layout, commands —
and hbui draws it in the terminal for the person and serves it as structured
data to the agent. Both change it through the same semantic actions.

The smallest complete program is `crates/commander/examples/todo.rs`
(`cargo run -p hbui-commander --example todo`). Read it first; it is about a
hundred lines and uses every piece below. `crates/commander/src/app.rs` is the
larger example: two panes, menus, and several dialogs.

## Dependencies

```toml
[dependencies]
hbui-core = { path = "../rs-hbui/crates/hbui-core" }        # state, widgets, actions
hbui-terminal = { path = "../rs-hbui/crates/hbui-terminal" } # the person's renderer
hbui-ipc = { path = "../rs-hbui/crates/hbui-ipc" }          # the session agents reach
```

Never depend on `hbui-mcp-bridge`. The application is never an MCP server; the
bridge is a separate process the agent's client starts.

## The shape of every hbui program

```rust
let shared = Shared::new(Session::new(build_ui(), MyController { .. }));
let _running = hbui_ipc::Endpoint::new(shared.clone(), "myapp")   // session name
    .with_capture(Arc::new(|ui, w, h| hbui_terminal::render(ui, w, h).surface.to_text()))
    .start()?;                         // keep `_running` alive; dropping it closes the session
hbui_terminal::run(&shared)?;          // the person's side; returns on ^Q / ^C
```

`build_ui()` returns a `UiState`. `MyController` implements `Controller`,
which is where the application's meaning lives. Everything else — selection,
focus, typing, drawing, the agent's view, revisions — is hbui's.

## 1. Lay it out

```rust
UiState::new(Layout::vsplit(vec![
    (Size::Fill(1), Layout::hsplit(vec![
        (Size::Fill(1), Layout::pane("left", "Files", "left.files")),
        (Size::Fill(1), Layout::pane("right", "Preview", "right.text")),
    ])),
    (Size::Fixed(1), Layout::Widget("status".into())),
]))
.with("left.files", Widget::List(List::new("Files", items)))
.with("right.text", Widget::text(""))
.with("status", Widget::text("ready"))
```

- `hsplit` is side by side and `vsplit` is stacked. `Size::Fixed(n)` takes
  cells and `Size::Fill(w)` a share of the rest.
- `Layout::pane(id, title, widget)` frames one widget. `Layout::Widget(id)`
  shows one widget unframed, like a status line. `Layout::Tabs` shows only
  its active tab.
- Only these shapes, plus one modal on top. There are no floating windows
  and no coordinates.
- A widget in the layout but missing from the map is simply not drawn, so
  check your ids.

## 2. Pick widgets

| Widget | Constructor | The person uses | The agent uses |
|---|---|---|---|
| text | `Widget::text("…")` | reads it | reads `value` |
| list | `Widget::List(List::new(label, items))` | ↑↓ PgUp/PgDn Home/End, Enter | `select`, `activate` |
| tree | `Widget::Tree(Tree::new(label, roots))` | ↑↓, ←→ collapse/expand, Enter | `select`, `expand`, `collapse`, `activate` |
| input | `Widget::Input(Input::new(label, text))` | typing, IME, paste, Enter | `set_text`, `activate` |
| button | `Widget::button("&OK")` / `Widget::default_button(..)` | Enter, Space, hotkey | `activate` |
| checkbox | `Widget::checkbox("Show &hidden", false)` | Space, hotkey | `set_checked`, `activate` (toggles) |
| radio | `Widget::Radio(RadioGroup::new(label, items))` | ↑↓ | `select` |

**Ids are the API.** The agent addresses everything by id, so:

- Name widgets `area.thing`, like `left.files` or `rename.name`. Prefix a
  dialog's widgets with the dialog's id.
- Item ids (`Item::new(id, label)`) must be stable and meaningful: a file
  name, a database key, a counter. Never use an index. Selection is stored by
  id and survives a refresh (`List::set_items`) when the id does.
- An `&` in a label marks the hotkey: `&Copy` has hotkey `c`, and `&&` is a
  literal `&`. The view shows the label without the `&` plus `"hotkey"`, and
  the renderer marks the letter.

## 3. Commands and menus

```rust
ui.commands = vec![
    Command::new("copy", "&Copy", Some(Key::F(5))),   // in the key bar at the bottom
    Command::new("options", "&Panel options...", None), // menu only
];
ui.menus = vec![
    Menu::new("file", "&File", vec![MenuItem::Command("copy".into()), MenuItem::Separator]),
    Menu::new("options", "&Options", vec![MenuItem::Command("options".into())]),
];
```

- A command is anything done to the screen as a whole. Its key is drawn in
  the key bar, and the agent `invoke`s it by id. Both read the same list.
- Menus hold command ids, never actions. F9 opens the menu bar for a person.
  An agent never opens a menu; it just invokes the command.
- `ui.set_enabled("copy", false)` dims a command: it is drawn grey, skipped
  in menus, its key is ignored, and `invoke` is refused with
  `command_disabled`. Keep this up to date in `settle` (see 5).

## 4. Give it meaning: `Controller::handle`

```rust
impl Controller for App {
    fn handle(&mut self, ui: &mut UiState, event: &Event) -> Result<(), String> {
        match event {
            Event::Activated { target, item } => { /* Enter on a list item, a button, an input */ }
            Event::Invoked { command } => { /* a command, from its key, menu or an agent */ }
            Event::ModalClosed { modal } => { /* Esc or close_modal: forget pending work */ }
        }
        Ok(())
    }
}
```

- You receive only what needs application meaning. Selecting, typing,
  checking a box and moving focus happen in hbui without an event. You read
  their results when they matter, typically when a dialog's OK is pressed.
- You may change anything in `ui`: refresh a list, retitle a pane
  (`ui.layout.set_pane_title`), write the status line, open or close a modal.
- **To refuse, return `Err(message)`.** The agent gets it as a `rejected`
  error. The person does not see it, so also write it to your status line.
  Changes made before the refusal stand.
- A person and an agent can trigger the same event. Never ask who did it;
  the answer must be the same.

## 5. Derived state: `Controller::settle`

```rust
fn settle(&mut self, ui: &mut UiState) {
    let file_selected = /* read the selection from ui */;
    ui.set_enabled("copy", file_selected);
}
```

`settle` runs before every commit, including the first view. Most changes
(a selection, a keystroke) never reach `handle`, so this is the only reliable
place for state that follows other state. Keep it cheap and idempotent.

## 6. Dialogs

```rust
ui.open_modal("rename", "Rename", vec![
    ("rename.prompt".into(), Widget::text("Rename notes.txt to:")),
    ("rename.name".into(),   Widget::Input(Input::new("Name", "notes.txt"))),
    ("rename.ok".into(),     Widget::default_button("&OK")),
    ("rename.cancel".into(), Widget::button("&Cancel")),
]);
```

- While a modal is open it is the whole input context. Focus stays inside,
  and actions or commands aimed elsewhere are refused (`blocked_by_modal`).
- The conventions are Midnight Commander's:
  - Enter presses the **default button** from anywhere in the dialog.
  - Esc cancels.
  - Tab, or an arrow the focused widget has no use for, moves between widgets.
  - Hotkey letters press buttons and toggle checkboxes, except while typing.
- Handle `Activated { target: "rename.ok" }`: read the fields, do the work,
  then `ui.close_modal()`. On failure, keep the dialog open, `Err(msg)`, and
  put the message in the status line so it can be corrected.
- Closing removes the dialog's widgets and gives focus back. Remember any
  pending work in the controller and clear it on `ModalClosed`.
- Give buttons and checkboxes distinct hotkeys within one dialog.

## 7. Open the session

`Endpoint::new(shared, "app-name")` claims a session:

- Without `.session(..)` the name is the application name, or `name-2`,
  `name-3`, … if that is taken.
- `.session("voxeler")` asks for exactly that name, and startup fails if a
  running process holds it. Use it when agents must find you by a fixed name.
- `.with_capture(..)` answers the agent's `capture_view` with your renderer's
  text. Always add it.
- `.on_link(|link| ..)` tells you when agents attach. Show it in a status
  line if you like, but remember every status change is a revision.

## Rules that are easy to break

- **Don't store coordinates, scroll offsets or cursor cells.** The renderer
  computes them every frame. State holds ids and grapheme positions only.
- **`handle` and `settle` run inside the session lock.** The person's
  keystrokes and every agent wait on them, so do slow work (network, large
  directories) outside and apply the result with `shared.lock().update(|ui| ..)`.
- **Every visible change is a revision, and a revision makes a waiting
  agent's action stale.** Don't write chatter into the view: no clock, no
  spinner, no "last updated" line.
- **Set the initial focus** (`ui.set_focus(id)?`) where the person will
  start. Otherwise it falls to the first focusable widget.
- **Start in a known state.** `Session::new` runs `settle` once before
  revision 0.

## Testing

Test without a terminal. Drive `Session` directly, as a person
(`s.input(&InputEvent::Key(Key::Down))`, `InputEvent::Text("…")` for IME
text) and as an agent (`s.dispatch(Action::Select { .. }.into())`). Assert on
`s.view()`, the semantic snapshot. For the drawing, assert on
`hbui_terminal::render(ui, w, h).surface.to_text()`, the visual snapshot.
`crates/hbui-core/src/tests.rs` (module `mc`) and
`crates/commander/src/app.rs` show both.

Then check it live: run the app, and with the `hbui` MCP server connected,
`list_sessions` → `get_view` → `dispatch` (see the hbui-driving skill). If
the agent can't do something without guessing, the UI is missing an id, a
command or a label.
