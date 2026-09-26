# rs-hbui

**hbui** — *half-breed UI* — is a Rust UI library for a person and an AI agent at
the same time.

> Pixels are a rendering detail. The UI is a structured interactive document.

An agent does not read your screen through screenshots or OCR, and does not
guess keystrokes or coordinates. The UI is one serializable state. The terminal
draws it for the person, and MCP serves the same state to the agent as
structured data. Both change it through the same semantic actions.

The agent reaches applications through `hbui-mcp-bridge`, a stdio MCP server
that the agent's client starts. Each application claims a named **session**.
The bridge finds every session by itself, so you can run several applications
at once and rebuild or restart them as often as you like (they can even
crash) without the agent losing its MCP connection:

```text
agent ──MCP── hbui-mcp-bridge ──┬── session "commander" ── commander ── terminal ── person
      (stdio)                   └── session "voxeler"   ── rs-voxeler
```

Each application is identified three ways:

- **session**: the logical application, stable across restarts. Agents name
  it on every call.
- **instance**: which run of that session. It goes up by one on each restart.
- **pid**: the current process. It can be used as a selector.

The first targets are a terminal UI and MCP. GUI renderers (egui / wgpu) come
next, once the core has proven itself, with rs-voxeler as the eventual user.
The design and its reasoning are in [docs/DESIGN.md](docs/DESIGN.md), the
bridge's in [docs/MCP_BRIDGE.md](docs/MCP_BRIDGE.md), and sessions in
[docs/SESSIONS.md](docs/SESSIONS.md). All three are in Japanese.

## Try it

Register the bridge once. The agent's client starts it:

```bash
make build
claude mcp add hbui -- $PWD/crates/target/debug/hbui-mcp-bridge
```

Start an application in a terminal, and restart it whenever you like:

```bash
make run DIR=~                      # the two-pane commander, session "commander"
```

Then use it from both sides at once. Press Tab, arrows and Enter in the
terminal, and ask the agent to "open Documents in the right pane and rename
notes.txt". The agent's change shows up on your screen. If you change something
while the agent is deciding, its stale action is refused and it has to look
again. If you quit and rerun the commander, the agent sees a new `instance` and
carries on over the same connection.

More applications just take more sessions:

```bash
./crates/target/debug/commander --hbui-session work ~/work   # named explicitly
./crates/target/debug/commander --headless /tmp              # no terminal; only agents drive it
```

Without `--hbui-session`, an application takes its own name, or `name-2`,
`name-3`, and so on if that is taken. A name held by a running process can't
be claimed twice.

Sessions live in `$HBUI_DIR`, or `hbui-$USER` in the temp dir, as
`<session>.sock` (0600) plus a `<session>.lock`. The lock is held while the
process lives and also stores the instance counter. Several agents, each with
its own bridge, can reach the same sessions at once.

Commander keys: `Tab` switches pane, `Enter` opens, `F2` renames, `F5` copies,
`F7` makes a directory, `^R` refreshes, `F9` opens the menu bar (File,
Options → Panel options), `Esc` closes a dialog or menu, and `^Q` quits.

## Widgets and conventions

The widgets are text, list, tree, input, button, checkbox and radio group,
laid out in splits, tabs and panes, with one modal dialog on top. A menu bar
of pull-down menus sits along the top. The keyboard conventions come from
[Midnight Commander](https://github.com/MidnightCommander/mc):

- **Menu bar:** `F9` pulls the menus down. `←`/`→` switch menus, `↑`/`↓` move
  the highlight, `Enter` chooses, `Esc` closes, and a letter picks an item or
  menu by its hotkey. Menu items are commands, so an agent just `invoke`s
  them and never opens a menu.
- **Dialogs:** `Enter` presses the default button (`[< OK >]`) from anywhere
  in the dialog, and `Esc` cancels. `Tab`, or an arrow the focused widget has
  no use for, moves between widgets. A button's or checkbox's hotkey letter
  presses it, except while typing in a text field.
- **Hotkeys:** a label marks its hotkey with `&`, as in `&Copy`. The view
  reports the label without the `&` and adds `"hotkey": "c"`.
- **Disabled commands:** they are drawn dimmed, skipped in menus, and refused
  with `command_disabled`. An application keeps them current in
  `Controller::settle`, which runs after every change.

## What the agent sees

`list_sessions` shows what is running:

```json
{ "sessions": [
  { "session": "commander", "application": "commander", "pid": 1234,
    "cwd": "/Users/me", "instance": 7, "status": "ready", "revision": 12 },
  { "session": "work", "application": "commander", "pid": 2345,
    "cwd": "/Users/me/work", "instance": 1, "status": "disconnected", "revision": 4 }
] }
```

`get_view {session: "commander"}` returns the semantic view:

```json
{
  "status": "ready",
  "session": "commander",
  "instance": 3,
  "revision": 42,
  "focus": "left.files",
  "modal": null,
  "commands": [{ "id": "rename", "label": "Rename", "key": "F2" }],
  "widgets": {
    "left.files": {
      "role": "list",
      "selected": "Documents",
      "items": [{ "id": "..", "label": "../" }, { "id": "Documents", "label": "Documents/" }],
      "actions": ["focus", "select", "activate"]
    }
  },
  "layout": { "split": "vertical", "children": ["..."] }
}
```

`dispatch` runs one action and reports exactly what changed:

```json
{ "session": "commander", "type": "select", "target": "left.files", "item": "Downloads",
  "expected_instance": 3, "expected_revision": 42 }
→ { "ok": true, "session": "commander", "instance": 3, "revision": 43,
    "changes": [{ "op": "replace", "path": "/widgets/left.files/selected", "value": "Downloads" }] }
```

If `changes` is empty, the action had no visible effect. The action is
refused with `stale_revision` if the person changed something in between, and
with `stale_instance` if the application restarted. `get_view {since: 42,
instance: 3}` returns only the changes since revision 42. For the last 64
revisions that is an exact diff. Further back (up to 4,096 revisions) it is
marked `"merged": true`: every path that changed, with its current value, as
`replace` or `remove` ops only.

While the application is down, `get_view` still answers, with `status:
"application_disconnected"` and the last view it saw, marked `"stale": true`.
That is the UI state just before a crash, available to the agent for
diagnosis. `capture_view` returns the
terminal's drawing as text, for when the agent needs to check the rendering
itself.

The actions are `focus`, `select`, `activate`, `set_text`, `set_checked`,
`expand`, `collapse`, `invoke` and `close_modal`.

## Using the library

```rust
use hbui_core::*;

let ui = UiState::new(Layout::hsplit(vec![
    (Size::Fill(1), Layout::pane("left", "Files", "files")),
]))
.with("files", Widget::List(List::new("Files", vec![Item::new("a", "a.txt")])));

struct App;
impl Controller for App {
    fn handle(&mut self, ui: &mut UiState, event: &Event) -> Result<(), String> {
        // Activated / Invoked / ModalClosed: the application's meaning goes here.
        Ok(())
    }
}

let shared = Shared::new(Session::new(ui, App));
// Open a session for agents. Keep `_running` alive: dropping it closes the
// session. Bridges find it by themselves.
let _running = hbui_ipc::Endpoint::new(shared.clone(), "my-app").start()?;
hbui_terminal::run(&shared)?;
```

## Crates

| crate | what it is |
| --- | --- |
| `hbui-core` | `UiState`, widgets (text, list, tree, input, button, checkbox, radio group), layout (split, tabs, pane, modal), menus, commands, semantic actions, the keymap, revisions and diffs, and the semantic view. No terminal types. |
| `hbui-terminal` | Renders into a cell `Surface`, writes only the changed cells, normalizes crossterm input, and restores the terminal on drop and on panic. |
| `hbui-ipc` | Sessions and the application ↔ bridge link: one Unix socket per session carrying JSON lines, a versioned handshake, per-session instance counters, and pushed views. The app side is `Endpoint` and the bridge side is `Bridge`. |
| `hbui-mcp-bridge` | The stdio MCP server. It finds sessions and forwards `get_view` / `dispatch` / `capture_view` to the session named. It owns no UI state. |
| `commander` | A Norton Commander-style two-pane file browser, used as the proof of concept. |

The link is a Unix socket, so macOS and Linux only for now.

## Skills

Two reusable agent skills:

- [`skills/hbui-building`](skills/hbui-building/SKILL.md) covers writing a UI
  with hbui: layout, widgets, commands and menus, dialogs, the `Controller`,
  and opening a session.
- [`skills/hbui-driving`](skills/hbui-driving/SKILL.md) covers operating a
  running app through the MCP tools: reading the view, semantic actions,
  dialogs, errors, and working alongside a person.

`crates/commander/examples/todo.rs` is the smallest complete program:
`cargo run -p hbui-commander --example todo`.

## Development

```bash
make check     # test + lint + fmt-check, which is what CI runs
make test
make fmt
```

See [CLAUDE.md](CLAUDE.md) for the invariants the code relies on.
