# rs-hbui — Developer Guide

hbui ("half-breed UI") is a UI library that a person and an AI agent drive at
the same time. A single `UiState` is the source of truth. `hbui-terminal` draws
it for the person. Each application claims a named session (a socket in the
session dir). Agents reach sessions through `hbui-mcp-bridge`, a stdio MCP
server that finds every session by itself, so applications can restart
without the agent reconnecting. The design rationale is in `docs/DESIGN.md`,
`docs/MCP_BRIDGE.md` and `docs/SESSIONS.md` (all Japanese; SESSIONS.md takes
precedence over MCP_BRIDGE.md where they differ). Read them before changing
anything structural.

## Layout

```text
crates/                 cargo workspace (run cargo from here, or use the Makefile)
  hbui-core/            state, widgets, layout, actions, keymap, session, view, diff
  hbui-terminal/        Surface, render, diff writer, TerminalGuard, crossterm input
  hbui-ipc/             sessions: per-session socket + lock, JSON lines, handshake
                        (app side: Endpoint, listens; bridge side: Bridge, scans)
  hbui-mcp-bridge/      stdio MCP server binary: wire, tools; forwards over hbui-ipc
  commander/            two-pane file browser PoC (binary `commander`)
    examples/todo.rs    the smallest complete hbui program (the skills point at it)
    tests/bridge_lifecycle.rs   the MCP_BRIDGE.md success scenario, with real processes
docs/DESIGN.md          the design
docs/MCP_BRIDGE.md      the bridge and the development lifecycle
docs/SESSIONS.md        several applications: sessions, instances, pids
skills/hbui-building/   agent skill: writing a UI with hbui
skills/hbui-driving/    agent skill: operating a running app via the MCP tools
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
- **Priority is open menu, then modal, then focused widget, then screen.**
  While a modal is open, only its widgets are reachable: actions elsewhere
  return `blocked_by_modal`, and so do commands. A pulled-down menu takes
  every key a person presses, but it never blocks an agent. `invoke` works
  whether or not a menu is open, and closes it.
- **Menus hold command ids, not actions.** Choosing a menu item is `invoke`.
  Which menu is open, and what is highlighted, is `UiState` (`open_menu`), so
  it appears in the view and the renderer draws it from there.
- **Key conventions follow Midnight Commander** (`keymap.rs`):
  - F9 opens the menu bar.
  - In a dialog, Enter presses the default button, unless a button has focus.
  - An arrow the focused widget has no use for moves focus.
  - A `&` hotkey presses a button or toggles a checkbox, except while a text
    field has focus.
- **Derived state belongs in `Controller::settle`**, for example which
  commands are enabled. It runs before every commit, including the first
  view, because most changes (selection, typing) never reach `handle`.
- **Labels carry `&` hotkeys.** The view strips the `&` and reports
  `hotkey`, and the renderer marks the letter. Use `widget::hotkey`, and never
  show a raw `&` to anyone.
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
- **Session, instance and pid are separate.** `session` is the logical
  application and the socket name. `instance` counts that session's runs;
  it is kept in `<session>.lock` so every bridge sees the same number. `pid`
  is only a selector. A revision only means something within its instance,
  so `get_view since` is honoured only with a matching `instance`.
  `expected_instance` is checked against the same connection the request is
  then sent on.
- **No current session.** Every MCP call except `list_sessions` names its
  `session` (or `pid`). Do not add a "select session" tool.
- **Apps listen, bridges scan.** An app claims a session by holding
  `<session>.lock` (`File::try_lock`), which the kernel releases however the
  process dies, and binds `<session>.sock`. Bridges scan the dir, and several
  may be connected to one app, each getting the same answers and pushes.
- **An absent application is a result, not a failure.** `get_view` returns
  `status: application_disconnected | protocol_mismatch` as a normal tool
  result. The last pushed view is returned
  only as `last_view` with `stale: true`. If an app dies mid-`dispatch`, the
  reply is `outcome: "unknown"`, never a claim that the action wasn't applied.
- **Bridges never block the application.** Everything runs on the endpoint's
  threads; only an explicitly requested session name that is taken fails
  startup.
- **Bump `hbui_ipc::VERSION` on any incompatible IPC change.** Mismatches are
  refused by name (`protocol_mismatch`).
- **Keep socket paths short.** Unix socket paths are limited to about 104
  bytes on macOS, so keep sockets in the system temp dir and not in deep
  scratch directories.
- **The bridge speaks stdio MCP only, so stdout is the protocol.**
  Diagnostics go to stderr only. The session dir is mode 0700 and the sockets
  0600.
- Tool refusals are tool results with `isError: true` and a stable `error`
  code (`ActionError::code`). JSON-RPC errors are only for protocol faults.

## Tests

- Semantic snapshots: compare `Session::view()` JSON (`hbui-core/src/tests.rs`).
- Visual snapshots: compare `render(..).surface.to_text()`
  (`hbui-terminal/src/tests.rs`). When a rendering change is intentional,
  update the expected text, and look at it before you do.
- Commander tests run against a scratch directory under the system temp dir.
- `commander/tests/bridge_lifecycle.rs` runs bridges in-process against real
  `commander --headless` processes in two sessions, killing and restarting
  them. Any change to
  the bridge, IPC or instance semantics must keep it passing.

## Adding things

Keep `skills/` true: a change to widgets, actions, tools or error codes
updates `skills/hbui-building` / `skills/hbui-driving` in the same change.

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
Radio groups have no separate cursor: arrows change the choice directly, not
the way mc's cursor-then-Space works. There is also no combo box
(single-choice drop-down) and no input history.
Other gaps: modal lists and trees, Table/Viewer/Graphics widgets, a GUI
renderer, and `capture_view` returning an image for graphics panes. There is
no Windows support yet (the IPC link is a Unix socket) and no `hbui-dev`
launcher/watcher (non-goal of the bridge itself).
