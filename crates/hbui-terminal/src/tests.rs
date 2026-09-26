//! Visual snapshots: what a person would see, checked as text.

use hbui_core::*;

use crate::render::{render, SAFETY_MARGIN};
use crate::surface::Color;

fn commander() -> UiState {
    let mut ui = UiState::new(Layout::vsplit(vec![
        (
            Size::Fill(1),
            Layout::hsplit(vec![
                (Size::Fill(1), Layout::pane("left", "Left", "left.files")),
                (Size::Fill(1), Layout::pane("right", "Right", "right.files")),
            ]),
        ),
        (Size::Fixed(1), Layout::Widget("status".into())),
    ]))
    .with(
        "left.files",
        Widget::List(List::new(
            "Files",
            vec![
                Item::new("documents", "Documents"),
                Item::new("downloads", "Downloads"),
                Item::new("src", "src"),
            ],
        )),
    )
    .with(
        "right.files",
        Widget::List(List::new("Files", vec![Item::new("tmp", "/tmp")])),
    )
    .with("status", Widget::text("ready"));
    ui.commands = vec![
        Command::new("copy", "Copy", Some(Key::F(5))),
        Command::new("move", "Move", Some(Key::F(6))),
    ];
    ui.set_focus("left.files").unwrap();
    ui
}

#[test]
fn two_panes_a_status_line_and_a_key_bar() {
    let frame = render(&commander(), 40, 8);
    assert_eq!(
        frame.surface.to_text(),
        "\
┌────── Left ──────┐┌───── Right ──────┐
│> Documents       ││> /tmp            │
│  Downloads       ││                  │
│  src             ││                  │
│                  ││                  │
└──────────────────┘└──────────────────┘
ready
F5 Copy  F6 Move
"
    );
    assert_eq!(frame.cursor, None, "no input has focus, so no cursor");
}

#[test]
fn a_long_list_scrolls_to_keep_the_selection_visible() {
    let mut ui = commander();
    let items = (0..20)
        .map(|i| Item::new(format!("f{i}"), format!("file {i}")))
        .collect();
    ui.list_mut("left.files").unwrap().set_items(items);
    ui.list_mut("left.files").unwrap().select("f10");
    let text = render(&ui, 40, 8).surface.to_text();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[4].starts_with("│> file 10"), "{text}");
    assert!(lines[1].starts_with("│  file 7"), "{text}");
}

#[test]
fn a_modal_is_drawn_over_the_screen_with_the_cursor_in_its_input() {
    let mut ui = commander();
    ui.open_modal(
        "rename",
        "Rename",
        vec![
            (
                "rename.name".into(),
                Widget::Input(Input::new("", "日本.txt")),
            ),
            ("rename.ok".into(), Widget::button("OK")),
            ("rename.cancel".into(), Widget::button("Cancel")),
        ],
    );
    let frame = render(&ui, 40, 12);
    assert_eq!(
        frame.surface.to_text(),
        "\
┌────── Left ──────┐┌───── Right ──────┐
│> Documents       ││> /tmp            │
│ ┌───────────── Rename ─────────────┐ │
│ │                                  │ │
│ │ 日本.txt                         │ │
│ │                                  │ │
│ │ [ OK ]  [ Cancel ]               │ │
│ │                                  │ │
│ └──────────────────────────────────┘ │
└──────────────────┘└──────────────────┘
ready
F5 Copy  F6 Move
"
    );
    // After "日本.txt": two wide graphemes and four narrow ones.
    assert_eq!(frame.cursor, Some((4 + 8, 4)));
}

/// However long the text, the cursor stops short of the field's right edge —
/// the safety margin Terminal.app's IME needs.
#[test]
fn the_input_cursor_keeps_its_distance_from_the_right_edge() {
    let mut ui = UiState::new(Layout::Widget("name".into())).with(
        "name",
        Widget::Input(Input::new("", "あいうえおかきくけこさしすせそ")),
    );
    ui.set_focus("name").unwrap();
    let frame = render(&ui, 12, 1);
    let (x, _) = frame.cursor.unwrap();
    assert!(x <= 12 - 1 - SAFETY_MARGIN, "cursor at {x}");
    // The text scrolled by whole graphemes: no half of a wide one on screen.
    assert!(frame
        .surface
        .row(0)
        .iter()
        .all(|c| c.grapheme != "\u{fffd}"));
    let text = frame.surface.to_text();
    assert!(text.trim_end().ends_with("そ"), "{text:?}");
}

fn with_menus() -> UiState {
    let mut ui = commander();
    ui.commands = vec![
        Command::new("copy", "&Copy", Some(Key::F(5))),
        Command::new("mkdir", "&Make directory", Some(Key::F(7))),
        Command::new("options", "&Panel options...", None),
    ];
    ui.set_enabled("copy", false);
    ui.menus = vec![
        Menu::new(
            "file",
            "&File",
            vec![
                MenuItem::Command("copy".into()),
                MenuItem::Separator,
                MenuItem::Command("mkdir".into()),
            ],
        ),
        Menu::new(
            "options",
            "&Options",
            vec![MenuItem::Command("options".into())],
        ),
    ];
    ui
}

#[test]
fn a_pulled_down_menu_lies_over_the_panes() {
    let mut ui = with_menus();
    ui.pull_down("file");
    let frame = render(&ui, 40, 9);
    assert_eq!(
        frame.surface.to_text(),
        "  File   Options\n\
┌┌─────────────────────┐── Right ──────┐
││ Copy             F5 │tmp            │
│├─────────────────────┤               │
││ Make directory   F7 │               │
│└─────────────────────┘               │
└──────────────────┘└──────────────────┘
ready
F5 Copy  F7 Make directory
"
    );
    let s = &frame.surface;
    // Copy is disabled: dimmed, and the highlight skipped it.
    assert_eq!(s.cell(3, 2).style.fg, Color::Grey);
    assert!(s.cell(3, 4).style.reverse, "Make directory is highlighted");
    // The hotkey of "File" is marked.
    assert!(s.cell(2, 0).style.underline);
    assert_eq!(frame.cursor, None);
}

#[test]
fn a_dialog_with_every_kind_of_field() {
    let mut ui = commander();
    ui.open_modal(
        "opts",
        "Options",
        vec![
            ("opts.hidden".into(), Widget::checkbox("Show &hidden", true)),
            (
                "opts.sort".into(),
                Widget::Radio(RadioGroup::new(
                    "Sort",
                    vec![Item::new("name", "Name"), Item::new("size", "Size")],
                )),
            ),
            (
                "opts.mask".into(),
                Widget::Input(Input::new("Mask", "*.rs")),
            ),
            ("opts.ok".into(), Widget::default_button("&OK")),
            ("opts.cancel".into(), Widget::button("&Cancel")),
        ],
    );
    let frame = render(&ui, 44, 18);
    assert_eq!(
        frame.surface.to_text(),
        "\
┌─────── Left ───────┐┌────── Right ───────┐
│> Documents         ││> /tmp              │
│ ┌────────────── Options ───────────────┐ │
│ │                                      │ │
│ │ [x] Show hidden                      │ │
│ │                                      │ │
│ │ Sort:                                │ │
│ │ (*) Name                             │ │
│ │ ( ) Size                             │ │
│ │                                      │ │
│ │ Mask: *.rs                           │ │
│ │                                      │ │
│ │ [< OK >]  [ Cancel ]                 │ │
│ │                                      │ │
│ └──────────────────────────────────────┘ │
└────────────────────┘└────────────────────┘
ready
F5 Copy  F6 Move
"
    );
    // The checkbox has focus; the text field does not, so no cursor.
    assert!(frame.surface.cell(4, 4).style.reverse);
    assert_eq!(frame.cursor, None);
}
