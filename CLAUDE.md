# rs-hbui — Developer Guide

hbui ("half-breed UI") is a UI library that a person and an AI agent drive at
the same time. A single `UiState` is the source of truth. `hbui-terminal` draws
it for the person. The agent reaches it through `hbui-mcp-bridge`, a separate
long-lived process that forwards over `hbui-ipc` to the application, so the
application can restart without the agent reconnecting. The design rationale
is in `docs/DESIGN.md` and `docs/MCP_BRIDGE.md` (both Japanese). Read them
before changing anything structural.

## Layout

```text
crates/                 cargo workspace (run cargo from here, or use the Makefile)
  hbui-core/            state, widgets, layout, actions, keymap, session, view, diff
  hbui-terminal/        Surface, render, diff writer, TerminalGuard, crossterm input
  hbui-ipc/             app <-> bridge link: Unix socket, JSON lines, handshake, instances
                        (app side: Endpoint; bridge side: Bridge)
  hbui-mcp-bridge/      MCP server binary: wire, tools, stdio, HTTP; forwards over hbui-ipc
  commander/            two-pane file browser PoC (binary `commander`)
    tests/bridge_lifecycle.rs   the MCP_BRIDGE.md success scenario, with real processes
docs/DESIGN.md          the design
docs/MCP_BRIDGE.md      the bridge and the development lifecycle
```

Dependencies point only downward. `hbui-core` knows nothing of terminals,
IPC or MCP. `hbui-ipc` depends on `hbui-core`. `hbui-mcp-bridge` depends on
`hbui-ipc` only. An application depends on `hbui-core`, `hbui-ipc` and a
renderer, and never on the bridge.

## Build and verify

```bash
make check                  # test + clippy -D warnings + fmt --check (= CI)
cd crates && cargo test; echo $?
```

Judge success by the exit status, not by grepping the test output.

The tree is rustfmt-clean and CI checks it, so run `make fmt` before
committing.

## Invariants (do not break these)

- **The UiState is the only source of truth.** Renderers and MCP only read
  it. Screen coordinates, scroll offsets and the terminal cursor position are
  never stored in the state; the renderer recomputes them every frame.
- **No crossterm (or any backend) type in `hbui-core`.** Input reaches the
  core as `InputEvent` / `Key`, already normalized.
- **Stable ids, never indices.** Widgets are addressed by `WidgetId`, and list
  and tree selection is stored as an item id. Agents never send coordinates or
  keys.
- **The revision moves if and only if the view changed.** `Session::commit`
  diffs the view body and bumps the revision only when the diff is non-empty.
  An empty `changes` list is how an agent learns that an action did nothing.
  Every mutation path (`dispatch`, `input`, `update`) must end in `commit`.
- **`expected_revision` is checked before anything runs.** A stale request has
  no effect at all.
- **Priority is modal, then focused widget, then screen.** While a modal is
  open, only its widgets are reachable: actions elsewhere return
  `blocked_by_modal`, and so do commands.
- **One view builder.** `view::view_body` produces the agent's view, the
  semantic snapshot and the diff input. Do not build a second one.
- **Diff paths address id-keyed arrays by id**
  (`/widgets/left.files/items/notes.txt/label`). An array whose ids changed is
  replaced whole.
- **Text cursors count graphemes.** Measure width with `unicode-width`.
  Never store a byte offset.
- **Keep the cursor off the right edge.** An input keeps `SAFETY_MARGIN`
  cells free to its right, and the cursor never sits in the last column
  (Terminal.app IME instability).
- **Terminal cleanup comes first.** `TerminalGuard` restores the terminal on
  drop, and a panic hook restores it before the panic message prints. Draw
  only changed cells, and emit nothing when nothing changed, so that IME
  composition is not disturbed.
- **The application is never an MCP server.** MCP (transports, init,
  protocol versions) lives only in the bridge, and the application only speaks
  `hbui-ipc`. The bridge owns no UI state; it forwards and labels.
- **A revision only means something within its instance.** The bridge
  numbers each accepted connection (`instance`), and every view and outcome
  carries it. `expected_instance` is checked against the same connection the
  request is then sent on. `get_view since` is honoured only with a matching
  `instance`.
- **An absent application is a result, not a failure.** `get_view` returns
  `status: application_unavailable | application_disconnected |
  protocol_mismatch` as a normal tool result. The last pushed view is returned
  only as `last_view` with `stale: true`. If an app dies mid-`dispatch`, the
  reply is `outcome: "unknown"`, never a claim that the action wasn't applied.
- **The application's link never blocks or fails the application.**
  `Endpoint` retries in the background, and a newer connection replaces an
  older one on the bridge.
- **Bump `hbui_ipc::VERSION` on any incompatible IPC change.** Mismatches are
  refused by name (`protocol_mismatch`).
- **Keep socket paths short.** Unix socket paths are limited to about 104
  bytes on macOS, so keep sockets in the system temp dir and not in deep
  scratch directories.
- **MCP stdio: stdout is the protocol.** Diagnostics go to stderr only. HTTP
  binds 127.0.0.1 and rejects non-local `Origin`. The IPC socket is mode 0600.
- Tool refusals are tool results with `isError: true` and a stable `error`
  code (`ActionError::code`). JSON-RPC errors are only for protocol faults.

## Tests

- Semantic snapshots: compare `Session::view()` JSON (`hbui-core/src/tests.rs`).
- Visual snapshots: compare `render(..).surface.to_text()`
  (`hbui-terminal/src/tests.rs`). When a rendering change is intentional,
  update the expected text, and look at it before you do.
- Commander tests run against a scratch directory under the system temp dir.
- `commander/tests/bridge_lifecycle.rs` runs a bridge in-process against real
  `commander --headless` processes, killing and restarting them. Any change to
  the bridge, IPC or instance semantics must keep it passing.

## Adding things

- **A new widget:** add it to `Widget`, `role`/`focusable`/`actions`, then
  `view::widget_view`, `keymap::for_focused`, `action::apply` and
  `render::widget`. Each needs a semantic and a visual test.
- **A new action:** add it to `Action`, `action::apply`, `ActionError` if it
  needs a new refusal, and the `dispatch` tool's schema and description in
  `hbui-mcp-bridge/src/server.rs`.
- **A new tool:** add an IPC method in `hbui-ipc/src/app.rs` (`Endpoint::handle`)
  and a forwarding tool in `hbui-mcp-bridge/src/server.rs`. The bridge must
  still need no application-specific knowledge.
- **Application meaning** (what activating an item does) belongs in a
  `Controller`, not in the core.

## Not yet done

Tabs have no key binding for a person yet (the agent can `select` them).
Other gaps: modal lists and trees, Table/Viewer/Graphics widgets, a GUI
renderer, and `capture_view` returning an image for graphics panes. There is
no Windows support yet (the IPC link is a Unix socket) and no `hbui-dev`
launcher/watcher (non-goal of the bridge itself).
