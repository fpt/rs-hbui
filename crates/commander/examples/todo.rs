//! The smallest complete hbui application: a to-do list that a person uses
//! in the terminal and an agent uses over MCP, at the same time.
//!
//! ```text
//! cargo run -p hbui-commander --example todo
//! ```
//!
//! Type into the field and press Enter to add; F8 (or Todo → Done) removes
//! the selected item; ^Q quits. An agent sees session "todo".

use std::sync::Arc;

use hbui_core::{
    Command, Controller, Event, Input, Item, Key, Layout, List, Menu, MenuItem, Session, Shared,
    Size, UiState, Widget,
};

const ITEMS: &str = "todo.items";
const NEW: &str = "todo.new";
const STATUS: &str = "status";

/// Application meaning lives here; everything else is hbui's.
struct Todo {
    next_id: u64,
}

impl Controller for Todo {
    fn handle(&mut self, ui: &mut UiState, event: &Event) -> Result<(), String> {
        match event {
            // Enter in the text field (or an agent's `activate` on it).
            Event::Activated { target, .. } if target == NEW => {
                let text = ui.input(NEW).map(|i| i.buffer.text().trim().to_string());
                let Some(text) = text.filter(|t| !t.is_empty()) else {
                    // Refuse with a message: the person sees it in the status
                    // line, the agent gets it back as the error.
                    ui.set_text(STATUS, "nothing to add");
                    return Err("nothing to add".into());
                };
                self.next_id += 1;
                // A stable id, never an index: the agent addresses items by it.
                let id = format!("t{}", self.next_id);
                if let Some(list) = ui.list_mut(ITEMS) {
                    list.items.push(Item::new(&id, &text));
                    list.select(&id);
                }
                ui.insert(NEW, Widget::Input(Input::new("New", "")));
                ui.set_text(STATUS, format!("added {text:?}"));
                Ok(())
            }
            Event::Invoked { command } if command == "done" => {
                let list = ui.list_mut(ITEMS).ok_or("no list")?;
                let id = list.selected.clone().ok_or("nothing selected")?;
                let at = list.selected_index().unwrap_or(0);
                list.items.retain(|i| i.id != id);
                // Keep the selection near where it was.
                let next = list.items.get(at.min(list.items.len().saturating_sub(1)));
                list.selected = next.map(|i| i.id.clone());
                ui.set_text(STATUS, "done");
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Derived state: Done is only usable with something selected. Runs
    /// after every change, however it was made.
    fn settle(&mut self, ui: &mut UiState) {
        let selected = matches!(ui.widget(ITEMS), Some(Widget::List(l)) if l.selected.is_some());
        ui.set_enabled("done", selected);
    }
}

fn build() -> UiState {
    let mut ui = UiState::new(Layout::vsplit(vec![
        (Size::Fill(1), Layout::pane("items", "To do", ITEMS)),
        (Size::Fixed(1), Layout::Widget(NEW.into())),
        (Size::Fixed(1), Layout::Widget(STATUS.into())),
    ]))
    .with(ITEMS, Widget::List(List::new("To do", Vec::new())))
    .with(NEW, Widget::Input(Input::new("New", "")))
    .with(
        STATUS,
        Widget::text("Enter: add  F8: done  F9: menu  ^Q: quit"),
    );
    ui.commands = vec![Command::new("done", "&Done", Some(Key::F(8)))];
    // Start in the text field: the first thing to do in an empty list is type.
    ui.set_focus(NEW)
        .expect("the field exists and is on screen");
    ui.menus = vec![Menu::new(
        "todo",
        "&Todo",
        vec![MenuItem::Command("done".into())],
    )];
    ui
}

fn main() -> std::io::Result<()> {
    let shared = Shared::new(Session::new(build(), Todo { next_id: 0 }));

    // Open session "todo" so agents can reach it through hbui-mcp-bridge.
    // Keep `_running` alive: dropping it closes the session.
    let _running = hbui_ipc::Endpoint::new(shared.clone(), "todo")
        .with_capture(Arc::new(|ui, w, h| {
            hbui_terminal::render(ui, w, h).surface.to_text()
        }))
        .start()?;

    // The person's side. Returns on ^Q or ^C, with the terminal restored.
    hbui_terminal::run(&shared)
}
