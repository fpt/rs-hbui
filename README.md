# rs-hbui

**hbui** — *half-breed UI* — is a Rust UI library for a person and an AI agent at
the same time.

> Pixels are a rendering detail. The UI is a structured interactive document.

An agent does not read your screen through screenshots or OCR, and does not
guess keystrokes or coordinates. The UI is one serializable state. The terminal
draws it for the person, and MCP serves the same state to the agent as
structured data. Both change it through the same semantic actions.

The agent reaches the application through a separate, long-lived
`hbui-mcp-bridge` process. So you can rebuild and restart the application as
often as you like, and it can even crash, without the agent losing its MCP
connection:

```text
agent ──MCP── hbui-mcp-bridge ──hbui-ipc── application ── UiState ── terminal ── person
             (long-lived)                 (short-lived)
```

The first targets are a terminal UI and MCP. GUI renderers (egui / wgpu) come
next, once the core has proven itself, with rs-voxeler as the eventual user.
The design and its reasoning are in [docs/DESIGN.md](docs/DESIGN.md), and the
bridge's in [docs/MCP_BRIDGE.md](docs/MCP_BRIDGE.md). Both are in Japanese.

## Try it

In one terminal, start the bridge and leave it running:

```bash
make bridge                         # MCP on http://127.0.0.1:8740/mcp
claude mcp add --transport http hbui http://127.0.0.1:8740/mcp
```

In another terminal, start the application. Restart it whenever you like:

```bash
make run DIR=~                      # the two-pane commander
```

Then use it from both sides at once. Press Tab, arrows and Enter in the
terminal, and ask the agent to "open Documents in the right pane and rename
notes.txt". The agent's change shows up on your screen. If you change something
while the agent is deciding, its stale action is refused and it has to look
again. If you quit and rerun the commander, the agent sees a new `instance` and
carries on over the same connection.

The bridge can also be a stdio server spawned by the agent's client, for
example `claude mcp add hbui -- $PWD/crates/target/debug/hbui-mcp-bridge`.
The application still runs on its own:

```bash
./crates/target/debug/commander --headless ~   # no terminal; only the agent drives it
```

The application and the bridge find each other through a Unix socket,
`$HBUI_SOCKET` or `hbui-dev-$USER.sock` in the temp dir. Both take
`--socket PATH`.

Commander keys: `Tab` switches pane, `Enter` opens, `F2` renames, `F5` copies,
`F7` makes a directory, `^R` refreshes, `Esc` closes a dialog, `^Q` quits.

## What the agent sees

`get_view` returns the semantic view:

```json
{
  "status": "ready",
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
{ "type": "select", "target": "left.files", "item": "Downloads",
  "expected_instance": 3, "expected_revision": 42 }
→ { "ok": true, "instance": 3, "revision": 43,
    "changes": [{ "op": "replace", "path": "/widgets/left.files/selected", "value": "Downloads" }] }
```

If `changes` is empty, the action had no visible effect. The action is
refused with `stale_revision` if the person changed something in between, and
with `stale_instance` if the application restarted. `get_view {since: 42,
instance: 3}` returns only the changes since revision 42.

While the application is down, `get_view` still answers, with `status:
"application_disconnected"` and the last view it saw, marked `"stale": true`.
That is the UI state just before a crash, available to the agent for
diagnosis. `capture_view` returns the
terminal's drawing as text, for when the agent needs to check the rendering
itself.

The actions are `focus`, `select`, `activate`, `set_text`, `expand`,
`collapse`, `invoke` and `close_modal`.

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
// Reach the agent through the bridge. This retries in the background and
// never blocks or fails the application.
hbui_ipc::Endpoint::new(shared.clone(), hbui_ipc::default_socket(), "my-app").spawn();
hbui_terminal::run(&shared)?;
```

## Crates

| crate | what it is |
| --- | --- |
| `hbui-core` | `UiState`, widgets (text, list, tree, input, button), layout (split, tabs, pane, modal), semantic actions, keymap, revisions and diffs, the semantic view. No terminal types. |
| `hbui-terminal` | Renders into a cell `Surface`, writes only the changed cells, normalizes crossterm input, and restores the terminal on drop and on panic. |
| `hbui-ipc` | The application ↔ bridge link: a Unix socket carrying JSON lines, a versioned handshake, instance ids, and a pushed view cache. The app side is `Endpoint` and the bridge side is `Bridge`. |
| `hbui-mcp-bridge` | The long-lived MCP server (stdio, or streamable HTTP on loopback). It forwards `get_view` / `dispatch` / `capture_view` and owns no UI state. |
| `commander` | A Norton Commander-style two-pane file browser, used as the proof of concept. |

The link is a Unix socket, so macOS and Linux only for now.

## Development

```bash
make check     # test + lint + fmt-check, which is what CI runs
make test
make fmt
```

See [CLAUDE.md](CLAUDE.md) for the invariants the code relies on.
