# rs-hbui — Developer Guide

hbui ("half-breed UI") is a UI library that a person and an AI agent drive at
the same time. A single `UiState` is the source of truth. `hbui-terminal` draws
it for the person, and `hbui-mcp` serves it to the agent as structured data.
The design rationale is in `docs/DESIGN.md` (Japanese). Read it before
changing anything structural.

## Layout

```text
crates/                 cargo workspace (run cargo from here, or use the Makefile)
  hbui-core/            state, widgets, layout, actions, keymap, session, view, diff
  hbui-terminal/        Surface, render, diff writer, TerminalGuard, crossterm input
  hbui-mcp/             JSON-RPC wire, tools (get_view/dispatch/capture_view), stdio, HTTP
  commander/            two-pane file browser PoC (binary `commander`)
docs/DESIGN.md          the design
```

Dependencies point only downward. `hbui-core` depends on neither the terminal
crate nor the MCP crate, and they do not depend on each other.

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
- **MCP stdio: stdout is the protocol.** Diagnostics go to stderr only. HTTP
  binds 127.0.0.1 and rejects non-local `Origin`.
- Tool refusals are tool results with `isError: true` and a stable `error`
  code (`ActionError::code`). JSON-RPC errors are only for protocol faults.

## Tests

- Semantic snapshots: compare `Session::view()` JSON (`hbui-core/src/tests.rs`).
- Visual snapshots: compare `render(..).surface.to_text()`
  (`hbui-terminal/src/tests.rs`). When a rendering change is intentional,
  update the expected text, and look at it before you do.
- Commander tests run against a scratch directory under the system temp dir.

## Adding things

- **A new widget:** add it to `Widget`, `role`/`focusable`/`actions`, then
  `view::widget_view`, `keymap::for_focused`, `action::apply` and
  `render::widget`. Each needs a semantic and a visual test.
- **A new action:** add it to `Action`, `action::apply`, `ActionError` if it
  needs a new refusal, and the `dispatch` tool's schema and description in
  `hbui-mcp/src/server.rs`.
- **Application meaning** (what activating an item does) belongs in a
  `Controller`, not in the core.

## Not yet done

Tabs have no key binding for a person yet (the agent can `select` them).
Other gaps: modal lists and trees, Table/Viewer/Graphics widgets, a GUI
renderer, and `capture_view` returning an image for graphics panes.
