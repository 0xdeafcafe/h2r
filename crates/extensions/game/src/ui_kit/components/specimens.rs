//! The design system's specimen pages (`design.<component>`, each
//! component's variants as named states) and the layout gallery
//! (`layout.<name>`, every layout a screen can take, with placeholder
//! copy so each reads as a template). Registered in
//! [`crate::ui_kit::screens::NAMES`], so they draw in the editor and pass
//! the checks like any screen.

use crate::list_class::TextBlock;
use crate::ui_kit::components::{Screen, legend, row};
use crate::ui_kit::dialog::{icon, quick_icon};
use crate::ui_kit::spec::{PaneText, ScreenSpec};
use crate::ui_kit::view::{Justify, Kind, Layer, Opener, Widget};

/// Every specimen and layout page, as `NAMES` lists them.
pub const PAGES: [&str; 32] = [
    "design.title",
    "design.body",
    "design.panel-text",
    "design.description",
    "design.row.action",
    "design.row.value-inline",
    "design.row.value-column",
    "design.row.setting",
    "design.row.info",
    "design.row.text",
    "design.row.disabled",
    "design.legend",
    "design.picker",
    "design.cells",
    "design.art",
    "design.focus",
    "design.lobby-tabs.a",
    "design.lobby-tabs.b",
    "design.lobby-tabs.c",
    "layout.list",
    "layout.list.description",
    "layout.list.panel-edge",
    "layout.list.level-select",
    "layout.list.texts",
    "layout.dialog.settings",
    "layout.dialog.settings.header",
    "layout.dialog.large",
    "layout.dialog.message",
    "layout.dialog.over-screen",
    "layout.code-entry",
    "layout.picker",
    "layout.hud",
];

/// Component `name`'s font and colour (a text component of design.yaml).
fn style(name: &str) -> (i16, [f32; 3]) {
    crate::ui_kit::data::design()
        .text_style(name)
        .unwrap_or_else(|e| panic!("{e}"))
}

/// A description block of component `name` at its preset place, or at
/// `rect` (`top, left, bottom, right`).
fn block(name: &str, rect: Option<[i16; 4]>) -> TextBlock {
    let d = crate::ui_kit::data::design();
    let c = d.component(name).unwrap_or_else(|e| panic!("{e}"));
    let (font, colour) = style(name);
    let rect = rect.unwrap_or_else(|| {
        let ([x, y], [w, h]) = (c.at.unwrap_or_default(), c.size.unwrap_or_default());
        [y, x, y - h as i16, x + w as i16]
    });
    TextBlock {
        rect,
        font,
        colour,
        animation: 0,
    }
}

/// A text of no row in component `component`'s style.
fn text(id: &str, component: &str, rect: [i16; 4], s: &str) -> PaneText {
    let (font, colour) = style(component);
    let right = crate::ui_kit::data::design()
        .component(component)
        .is_ok_and(|c| c.justify() == Justify::Right);
    PaneText {
        id: id.into(),
        rect,
        font,
        colour,
        right,
        text: s.into(),
    }
}

/// A heading and a table of three labels and values, from `top` down.
fn table(mut s: Screen, top: i16) -> Screen {
    s = s.text(text("text.heading", "text.heading", [top, -242, top - 26, 120], "Heading"));
    for i in 0..3i16 {
        let t = top - 30 - 22 * i;
        let n = i + 1;
        s = s
            .text(text(&format!("text.label.{i}"), "text.label", [t, -242, t - 20, -40], &format!("Label {n}")))
            .text(text(&format!("text.value.{i}"), "text.value", [t, -40, t - 20, 120], &format!("Value {n}")));
    }
    s
}

/// `spec` in each named focus.
fn focused(spec: ScreenSpec, at: &[(&str, usize)]) -> Vec<(String, ScreenSpec)> {
    at.iter()
        .map(|(n, f)| {
            (
                n.to_string(),
                ScreenSpec {
                    focus: *f,
                    ..spec.clone()
                },
            )
        })
        .collect()
}

/// `focus-0` to `focus-<n-1>`.
fn each_row(spec: ScreenSpec) -> Vec<(String, ScreenSpec)> {
    let n = spec.dialog.as_ref().map_or(spec.rows.len(), |d| d.rows.len());
    (0..n)
        .map(|f| {
            (
                format!("focus-{f}"),
                ScreenSpec {
                    focus: f,
                    ..spec.clone()
                },
            )
        })
        .collect()
}

/// `spec` with its code's cursor in each named cell.
fn cursors(spec: ScreenSpec, at: &[(&str, usize)]) -> Vec<(String, ScreenSpec)> {
    at.iter()
        .map(|(n, c)| {
            let mut s = spec.clone();
            if let Some(cells) = s.dialog.as_mut().and_then(|d| d.cells.as_mut()) {
                cells.cursor = Some(*c);
            }
            (n.to_string(), s)
        })
        .collect()
}

fn rows3(s: Screen, help: bool) -> Screen {
    (1..=3).fold(s, |s, n| {
        let r = row::action(&format!("Row {n}"));
        s.row(if help { r.describe(&format!("What row {n} does.")) } else { r })
    })
}

fn picks3(s: Screen) -> Screen {
    (1..=3).fold(s, |s, n| {
        s.row(row::pick(&format!("Row {n}"), &["On", "Off"], 0).describe(&format!("What row {n} sets.")))
    })
}

fn level_select_art(s: Screen) -> Screen {
    crate::ui_kit::screens::skulls_art().into_iter().fold(s, Screen::art)
}

fn hud_text(id: &str, component: &str, rect: [i16; 4], s: &str) -> Widget {
    let (font, colour) = style(component);
    Widget {
        id: id.into(),
        rect,
        layer: Layer::Content,
        alpha: 255,
        kind: Kind::Text {
            text: s.into(),
            font,
            justify: Justify::Left,
            colour: crate::ui_kit::components::rgb(colour),
        },
    }
}

const LONG_HELP: &str = "A longer description wraps onto a second line of the block.";

/// Page `name`'s named states; `None` for a name that isn't one.
pub fn states(name: &str) -> Option<Vec<(String, ScreenSpec)>> {
    let menu = Opener::MAIN_MENU;
    let setup = Opener::GAME_SETUP;
    let rest = |s: Screen| vec![("rest".to_string(), s.build())];
    Some(match name {
        "design.title" => rest(
            Screen::list(name, "Title")
                .row(row::action("Row 1"))
                .row(row::action("Row 2"))
                .legend(vec![legend::select(), legend::back()]),
        ),
        "design.body" => rest(
            Screen::message(
                name,
                "TITLE",
                "Body text wraps at the panel's width. A second sentence shows the wrap.",
                menu,
            )
            .status("A status line.")
            .legend(vec![legend::done(), legend::cancel()]),
        ),
        "design.panel-text" => rest(table(
            Screen::list(name, "Panel Text")
                .row(row::action("Row 1"))
                .row(row::action("Row 2"))
                .legend(vec![legend::select(), legend::back()]),
            -12,
        )),
        "design.description" => focused(
            Screen::list(name, "Description")
                .description(block("description", None))
                .row(row::action("Row 1").describe("What row 1 does."))
                .row(row::action("Row 2").describe("What row 2 does."))
                .row(row::action("Row 3").describe(LONG_HELP))
                .legend(vec![legend::select(), legend::back()])
                .build(),
            &[("first", 0), ("second", 1), ("long", 2)],
        ),
        "design.row.action" => focused(
            Screen::list(name, "Action Rows")
                .row(row::action("Action Row"))
                .row(row::action("Second Action Row"))
                .row(row::action("A Longer Action Row Label"))
                .legend(vec![legend::select(), legend::back()])
                .build(),
            &[("focused", 0), ("unfocused", 1), ("long", 2)],
        ),
        "design.row.value-inline" => focused(
            Screen::level_select(name, "Inline Values")
                .row(row::value("Value Row", &["On", "Off"], 0))
                .row(row::value("Second Value Row", &["Easy", "Normal", "Heroic", "Legendary"], 1))
                .row(row::value("A Longer Value Row", &["Off", "On"], 0))
                .legend(vec![legend::change(), legend::back()])
                .build(),
            &[("focused", 0), ("unfocused", 1), ("long", 2)],
        ),
        "design.row.value-column" => focused(
            Screen::list(name, "Column Values")
                .row(row::value("Value Row", &["On", "Off"], 0))
                .row(row::value("Second Value Row", &["Easy", "Normal", "Heroic", "Legendary"], 1))
                .row(row::value("A Longer Value Row", &["Off", "On"], 0))
                .legend(vec![legend::change(), legend::back()])
                .build(),
            &[("focused", 0), ("unfocused", 1), ("long", 2)],
        ),
        "design.row.setting" => focused(
            Screen::settings(name, "SETTINGS", setup)
                .row(row::pick("Setting Row", &["On", "Off"], 0).describe("What the setting does."))
                .row(
                    row::pick("Second Setting Row", &["Easy", "Normal", "Heroic", "Legendary"], 1)
                        .describe("What the second setting does."),
                )
                .row(row::pick("A Longer Setting Row", &["Off", "On"], 0).describe("What the longer one does."))
                .legend(vec![legend::change(), legend::select(), legend::back()])
                .build(),
            &[("focused", 0), ("unfocused", 1), ("long", 2)],
        ),
        "design.row.info" => focused(
            Screen::list(name, "Info Rows")
                .row(row::info("Info Row", "Value"))
                .row(row::info("Second Info Row", "Ready"))
                .row(row::info("A Longer Info Row", "A Longer Value"))
                .legend(vec![legend::back()])
                .build(),
            &[("focused", 0), ("unfocused", 1), ("long", 2)],
        ),
        "design.row.text" => focused(
            Screen::list(name, "Text Rows")
                .row(row::text("Text Row", "Chief", 15))
                .row(row::text("Second Text Row", "Spartan", 15))
                .row(row::text("Empty Text Row", "", 15))
                .legend(vec![legend::select(), legend::back()])
                .build(),
            &[("focused", 0), ("unfocused", 1), ("empty", 2)],
        ),
        "design.row.disabled" => focused(
            Screen::list(name, "Disabled Rows")
                .row(row::action("Action Row"))
                .row(row::disabled("Disabled Row"))
                .legend(vec![legend::back()])
                .build(),
            &[("focused", 1), ("unfocused", 0)],
        ),
        "design.legend" => rest(
            Screen::list(name, "Legend")
                .row(row::value("Value Row", &["On", "Off"], 0))
                .row(row::value("Second Value Row", &["Off", "On"], 0))
                .legend(vec![legend::change(), legend::select(), legend::back()]),
        ),
        "design.picker" => focused(
            Screen::picker(
                name,
                "SETTING",
                &["Value 1", "Value 2", "Value 3", "Value 4", "Value 5", "Value 6", "Value 7", "Value 8", "Value 9"],
                0,
                "What the setting does, and what its values mean.",
                setup,
            )
            .build(),
            &[("first", 0), ("scrolled", 4), ("last", 8)],
        ),
        "design.cells" => cursors(
            Screen::code_entry(
                name,
                "TITLE",
                "Enter the code.",
                crate::ui_kit::components::dialog_cells("123456", Some(0), Some(2)),
                menu,
            )
            .legend(vec![legend::digit(), legend::move_cursor(), legend::done(), legend::cancel()])
            .build(),
            &[("first", 0), ("middle", 2), ("last", 5)],
        ),
        "design.art" => rest(level_select_art(
            Screen::level_select(name, "Art")
                .row(row::action("Row 1"))
                .row(row::action("Row 2"))
                .legend(vec![legend::select(), legend::back()]),
        )),
        "design.focus" => focused(
            (1..=5)
                .fold(Screen::list(name, "Focus"), |s, n| s.row(row::action(&format!("Row {n}"))))
                .legend(vec![legend::select(), legend::back()])
                .build(),
            &[("first", 0), ("middle", 2), ("last", 4)],
        ),
        "layout.list" => each_row(
            rows3(Screen::list(name, "Title"), false)
                .legend(vec![legend::select(), legend::back()])
                .build(),
        ),
        "layout.list.description" => each_row(
            rows3(Screen::list(name, "Title").description(block("description", None)), true)
                .legend(vec![legend::select(), legend::back()])
                .build(),
        ),
        "layout.list.panel-edge" => each_row(
            rows3(
                Screen::list(name, "Title").description(block("legend.panel", Some([-24, -242, -114, 120]))),
                true,
            )
            .legend(vec![legend::select(), legend::back()])
            .legend_on_panel()
            .build(),
        ),
        "layout.list.level-select" => each_row(
            level_select_art(rows3(
                Screen::level_select(name, "Title").description(block("description.pane", None)),
                true,
            ))
            .legend(vec![legend::select(), legend::back()])
            .build(),
        ),
        "layout.list.texts" => each_row(
            table(
                rows3(Screen::list(name, "Title").description(block("description", None)), true),
                -70,
            )
            .legend(vec![legend::select(), legend::back()])
            .build(),
        ),
        "layout.dialog.settings" => each_row(
            picks3(Screen::settings(name, "TITLE", setup))
                .legend(vec![legend::change(), legend::select(), legend::back()])
                .build(),
        ),
        "layout.dialog.settings.header" => each_row({
            let mut s = picks3(Screen::settings(name, "TITLE", setup))
                .art(quick_icon(icon::SPARTAN))
                .legend(vec![legend::change(), legend::select(), legend::back()])
                .build();
            s.content.required_icons.push("art.0".into());
            s
        }),
        "layout.dialog.large" => each_row(
            Screen::large(name, "TITLE", menu)
                .row(row::action("Row 1"))
                .row(row::action("Row 2"))
                .legend(vec![legend::select(), legend::back()])
                .build(),
        ),
        "layout.dialog.message" => rest(
            Screen::message(name, "TITLE", "Body text.", menu)
                .legend(vec![legend::done(), legend::cancel()]),
        ),
        "layout.dialog.over-screen" => each_row(
            Screen::large(name, "TITLE", menu)
                .row(row::action("Row 1"))
                .row(row::action("Row 2"))
                .legend(vec![legend::select(), legend::back()])
                .over(crate::ui_kit::screens::sample("layout.list")?)
                .build(),
        ),
        "layout.code-entry" => cursors(
            Screen::code_entry(
                name,
                "TITLE",
                "Body text.",
                crate::ui_kit::components::dialog_cells("000000", Some(0), Some(2)),
                menu,
            )
            .legend(vec![legend::digit(), legend::move_cursor(), legend::done(), legend::cancel()])
            .build(),
            &[("cursor-0", 0), ("cursor-5", 5)],
        ),
        "layout.picker" => each_row(
            Screen::picker(name, "TITLE", &["Value 1", "Value 2", "Value 3"], 0, "Description.", setup).build(),
        ),
        "layout.hud" => rest(Screen::hud(
            name,
            "Title",
            vec![
                hud_text("hud.caption", "hud.label", [198, -264, 178, -100], "Caption"),
                hud_text("hud.line", "hud.text", [176, -264, 156, -100], "Line"),
            ],
        )),
        // The lobby strip's designs (`crate::lobby_tabs::designs`).
        _ if crate::lobby_tabs::designs::PAGES.contains(&name) => {
            crate::lobby_tabs::designs::states(name)?
        }
        _ => return None,
    })
}
