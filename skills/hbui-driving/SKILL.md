---
name: hbui-driving
description: Operate a running hbui application through the hbui MCP tools (list_sessions, get_view, dispatch, capture_view) — read its UI as data, act with semantic actions, fill dialogs, use menus, and cope with a person using it at the same time or the app restarting. Use when asked to do something in, inspect, or verify an hbui app, not when writing its code (that is hbui-building).
---

# Driving an hbui application over MCP

An hbui application's UI is a structured document, not a screen. You read it
as JSON and change it with semantic actions that name widgets and items by
id. You never send keys and never guess coordinates. A person may be using
the same UI in a terminal at the same moment. They see what you do, and you
see what they do.

## The four tools

| Tool | Use |
|---|---|
| `list_sessions` | Which apps are running: `session`, `application`, `pid`, `cwd`, `instance`, `status`, `revision` |
| `get_view {session}` | The whole UI as data. `{session, since, instance}` returns only what changed |
| `dispatch {session, type, …, expected_instance, expected_revision}` | One semantic action |
| `capture_view {session, width?, height?}` | What the person's screen shows, as text. For checking the drawing, not for working from |

The server has these tools and nothing else: no resources, no prompts. Don't
look for them; start with `list_sessions`.

Name the `session` on every call. There is no current session. `pid` also
works as a selector, but pids change on restart and sessions don't.

## The loop

1. `list_sessions`, and pick the session. If several could match, use `cwd`
   and `application` to choose, or ask.
2. `get_view {session}`. Note `instance` and `revision`.
3. `dispatch` one action, with `expected_instance` and `expected_revision`
   from the view you decided on.
4. Read the result's `changes`, which list exactly what changed.
   - **An empty list means the action had no visible effect.** Don't assume
     it worked; look at why.
   - The result carries the new `revision`, so use it as `expected_revision`
     for the next action instead of re-reading the whole view.
5. When you need to catch up, call `get_view {session, since: <revision>,
   instance: <instance>}`. That returns just the changes, which is much
   cheaper than a full view.

## Reading the view

```json
{
  "status": "ready", "session": "commander", "instance": 2, "revision": 41,
  "focus": "left.files",
  "modal": null,
  "layout":   { "split": "vertical", "children": [ … { "pane": "left", "title": "/home/me", "content": "left.files" } … ] },
  "widgets":  { "left.files": { "role": "list", "selected": "notes.txt", "items": [{ "id": "notes.txt", "label": "notes.txt" }],
                                "actions": ["focus", "select", "activate"] } },
  "commands": [{ "id": "copy", "label": "Copy", "hotkey": "c", "key": "F5", "enabled": false }],
  "menus":    [{ "id": "file", "label": "File", "items": ["rename", "copy", "-", "refresh"] }],
  "menu": null
}
```

- **`widgets`** is keyed by id. Each widget has a `role` and lists the
  `actions` it accepts. Use only those.
- **`layout`** tells you where widgets sit (panes, their titles) without any
  coordinates.
- **`focus`** is where a person's next key goes. You don't need to focus
  things before acting: `select`, `activate`, `set_text` and `set_checked`
  move focus to their target themselves.
- **`modal`**, when set, is the only thing that accepts actions. Its
  `children` are the dialog's widget ids.
- **`commands`** are screen-level operations. `"enabled": false` means it
  would be refused now; the reason is usually the current selection.
- **`menus`** only group commands; to use a menu item, `invoke` its command.
  `menu` shows when a person has a menu pulled down. Your `invoke` closes it.
- **Labels** have their `&` removed. `hotkey` is the person's letter for it,
  which you can ignore.
- **`capture_view`** shows what the person sees: truncation, layout,
  whether something is actually on screen. Use it to verify, never to
  decide what to do.

## Actions

| `type` | Fields | Does |
|---|---|---|
| `select` | `target`, `item` | Choose an item in a list, tree or radio group, or a tab (target = tab-set id) |
| `activate` | `target`, `item?` | The widget's main action: open a list/tree item (selecting `item` first), press a button, submit an input, toggle a checkbox |
| `set_text` | `target`, `value` | Replace an input's whole text. It is not typing, so give the complete value |
| `set_checked` | `target`, `checked` | Set a checkbox on or off. Prefer it to `activate`: repeating it is harmless |
| `expand` / `collapse` | `target`, `item` | Open or close a tree node. `select` on a hidden node reveals it anyway |
| `invoke` | `command` | Run a command, whether it lives in a menu or the key bar |
| `focus` | `target` | Move focus. Rarely needed |
| `close_modal` | — | Cancel the open dialog, like Esc |

### Recipes

- **Open something in a list:**
  `activate {target: "left.files", item: "Documents"}`.
- **Fill a dialog and confirm:**
  1. `invoke {command: "rename"}`, then read `modal.children`.
  2. `set_text` / `set_checked` / `select` each field.
  3. `activate` the button with `"default": true`, usually `<dialog>.ok`.
  4. The dialog is gone from `modal` if it succeeded. On `rejected`, a
     well-behaved app keeps it open with your values, so fix them and try
     again.
- **Use a menu item:** find its command id in `menus[].items`, then
  `invoke {command}`. Never open menus.
- **Check a result:** read the `changes` of your last action. For anything
  visual, `capture_view`.

## Errors, and what to do

| `error` | Meaning | Do |
|---|---|---|
| `stale_revision` | The UI changed since you read it, usually because the person did something | `get_view` again, re-decide, retry. Don't blindly resend |
| `stale_instance` | The app restarted | `get_view` again. Ids are probably the same, but check |
| `blocked_by_modal` | A dialog is open | Act inside it, or `close_modal` if it's yours to cancel |
| `command_disabled` | The command doesn't apply right now | Change what it depends on (often the selection), or tell the user |
| `rejected` | The app refused; `message` says why | Fix the input the message points at |
| `unknown_target` / `unknown_item` / `unknown_command` | That id doesn't exist | Re-read the view; ids are exact and case-sensitive |
| `not_supported` / `not_visible` | Wrong action for that role, or the widget is on a hidden tab | Check `actions`; `select` the tab first |
| `session_required` / `unknown_session` | No session named, or the wrong name | Use a name from the `sessions` list in the error |
| `application_disconnected` | The app isn't running (or it died mid-action: `outcome: "unknown"`) | See below |

When `get_view` returns `status: "application_disconnected"`, the bridge is
fine and the app is not running. A `last_view` with `"stale": true` is its
state just before it went away. Use that to diagnose a crash, never to act
on. Wait for the app to come back as a new `instance`, or ask the user to
restart it. After `outcome: "unknown"`, read the view again before retrying.
The action may already have happened.

## Working alongside a person

- They can see everything you do. Before a large or destructive change
  (deleting, overwriting, closing their dialog), say what you're about to
  do, or ask.
- A `stale_revision` means they're active. Re-read and adapt; don't race
  them.
- Don't cancel a modal you didn't open unless you've been asked to.
- A command's effects are real. Copy copies and delete deletes, exactly as
  if the person pressed the key.
