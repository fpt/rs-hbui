# rs-hbui

**hbui** — *half-breed UI* — is a Rust UI library for a person and an AI agent at
the same time.

> Pixels are a rendering detail. The UI is a structured interactive document.

An agent does not read your screen through screenshots or OCR, and does not
guess keystrokes or coordinates. The UI is one serializable state. The terminal
draws it for the person, and MCP serves the same state to the agent as
structured data. Both change it through the same semantic actions.

```text
              UiState
             /        \
       hbui-terminal   hbui-mcp
          person          agent
```

The first targets are a terminal UI and MCP. GUI renderers (egui / wgpu) come
next, once the core has proven itself, with rs-voxeler as the eventual user.
The design and its reasoning are in [docs/DESIGN.md](docs/DESIGN.md), which is
in Japanese.

## Try it

```bash
make run-mcp DIR=~                # the two-pane commander, also serving MCP
claude mcp add --transport http commander http://127.0.0.1:8740/mcp
```

Then use it from both sides at once. Press Tab, arrows and Enter in the
terminal, and ask the agent to "open Documents in the right pane and rename
notes.txt". The agent's change shows up on your screen. If you change something
while the agent is deciding, its stale action is refused and it has to look
again.

For a headless session that the agent spawns itself:

```bash
make build
claude mcp add commander -- $PWD/crates/target/debug/commander mcp ~
```

Commander keys: `Tab` switches pane, `Enter` opens, `F2` renames, `F5` copies,
`F7` makes a directory, `^R` refreshes, `Esc` closes a dialog, `^Q` quits.

## What the agent sees

`get_view` returns the semantic view:

```json
{
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
{ "type": "select", "target": "left.files", "item": "Downloads", "expected_revision": 42 }
→ { "ok": true, "revision": 43,
    "changes": [{ "op": "replace", "path": "/widgets/left.files/selected", "value": "Downloads" }] }
```

If `changes` is empty, the action had no visible effect. `get_view {since: 42}`
returns only the changes since revision 42. `capture_view` returns the
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
hbui_mcp::serve_http(std::sync::Arc::new(hbui_mcp::Server::new(shared.clone(), "app", "0.1")), 8740)?;
hbui_terminal::run(&shared)?;
```

## Crates

| crate | what it is |
| --- | --- |
| `hbui-core` | `UiState`, widgets (text, list, tree, input, button), layout (split, tabs, pane, modal), semantic actions, keymap, revisions and diffs, the semantic view. No terminal types. |
| `hbui-terminal` | Renders into a cell `Surface`, writes only the changed cells, normalizes crossterm input, and restores the terminal on drop and on panic. |
| `hbui-mcp` | `get_view` / `dispatch` / `capture_view` over MCP stdio or streamable HTTP (loopback only). |
| `commander` | A Norton Commander-style two-pane file browser, used as the proof of concept. |

## Development

```bash
make check     # test + lint + fmt-check, which is what CI runs
make test
make fmt
```

See [CLAUDE.md](CLAUDE.md) for the invariants the code relies on.
