use super::*;
use crate::ui_kit::preview::From;
use crate::ui_kit::screens::{self, NAMES};

fn doc(yaml: &str) -> Result<ScreenDoc, DataError> {
    ScreenDoc::parse("t", yaml, design())
}

#[test]
fn the_design_system_parses_and_names_what_it_uses() {
    let d = design();
    for role in ["title", "legend", "body", "cell", "row_controls", "row_default"] {
        d.font(role).unwrap();
    }
    for c in ["hint_blue", "pale_blue", "item_blue", "white"] {
        d.colour(c).unwrap();
    }
    for c in ["title", "legend", "body", "status", "description", "text.label", "text.value"] {
        d.text_style(c).unwrap();
    }
    assert!(d.component("no-such").is_err());
}

/// Every screen of `NAMES` has its doc, and every doc is a screen's: run
/// `h2 ui export-yaml --out assets/ui` to write them.
#[test]
fn every_screen_has_its_doc_and_every_doc_a_screen() {
    let missing: Vec<&str> = NAMES.iter().copied().filter(|n| screen(n).is_none()).collect();
    assert!(missing.is_empty(), "no doc in assets/ui/screens for {missing:?}");
    let extra: Vec<&str> = names().into_iter().filter(|n| !NAMES.contains(n)).collect();
    assert!(extra.is_empty(), "docs of no screen: {extra:?}");
}

/// A load and a save with no edit is the file, byte for byte.
#[test]
fn every_doc_saves_as_it_loaded() {
    for (name, text) in baked::SCREENS {
        let d = ScreenDoc::parse(name, text, design()).unwrap_or_else(|e| panic!("{e}"));
        let saved = to_yaml(&d);
        assert_eq!(saved, *text, "{name}: a load and a save changed the file");
    }
}

/// The proof the move changed nothing: each screen in each state, its doc
/// laid over the spec its Rust builds, draws the view the Rust alone
/// draws.
#[test]
fn each_state_drawn_from_its_doc_is_the_view_rust_drew() {
    let from = From::Synthetic;
    let mut diffs = Vec::new();
    for name in NAMES {
        let Some(d) = screen(name) else { continue };
        for (state, spec) in screens::raw_states(name).unwrap() {
            let rust = from.view(&spec, &state).unwrap();
            let presented = present(&spec, &d, design()).unwrap_or_else(|e| panic!("{e}"));
            let yaml = from.view(&presented, &state).unwrap();
            if yaml != rust {
                let ids: Vec<&str> = rust
                    .widgets
                    .iter()
                    .filter(|w| yaml.widget(&w.id) != Some(w))
                    .map(|w| w.id.as_str())
                    .collect();
                diffs.push(format!("{name}/{state}: {ids:?}"));
            }
        }
    }
    assert!(diffs.is_empty(), "{diffs:#?}");
}

/// The baked files are parsed once a process; a screen's doc from the
/// cache presents as a fresh parse of its file does.
#[test]
fn the_baked_docs_are_parsed_once() {
    for _ in 0..3 {
        for name in NAMES {
            let _ = screen(name);
        }
    }
    assert_eq!(baked_parses(), 1);
    let from = From::Synthetic;
    for (name, text) in baked::SCREENS.iter().take(8) {
        let fresh = ScreenDoc::parse(name, text, design()).unwrap();
        let cached = screen(name).unwrap();
        assert_eq!(*cached, fresh, "{name}");
        if let Some((state, spec)) = screens::raw_states(name).and_then(|s| s.into_iter().next()) {
            let a = from.view(&present(&spec, &cached, design()).unwrap(), &state).unwrap();
            let b = from.view(&present(&spec, &fresh, design()).unwrap(), &state).unwrap();
            assert_eq!(a, b, "{name}");
        }
    }
    assert_eq!(baked_parses(), 1);
}

#[test]
fn a_doc_round_trips_its_texts_however_they_read() {
    let yaml = "screen: t\nkind: dialog\nlayout: large\nbacking: over\nopener: main-menu\nstates:\n  \"code wrong\":\n    focus: 0\n    cursor: 2\nwidgets:\n  title:\n    component: title\n    text: \"Say \\\"hi\\\" {{x}}\"\n  body:\n    component: body\n    text: \"Line one\\nLine two \u{e100} \\\\ done\"\n  legend:\n    component: legend\n    prompts: [[A, \"SELECT\"], [[Left, Right], \"CHANGE\"]]\n  art.0:\n    component: art\n    at: [-267, 166]\n    size: [40, 40]\n    art: { screen: 222, pane: 0, index: 1, image: 3 }\nrows:\n  row.0:\n    kind: value\n    text: \"Count\"\n    values: [\"On\", \"Off\"]\n    current: 1\n    help: \"How many.\"\n";
    let d = doc(yaml).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(to_yaml(&d), yaml);
    assert_eq!(d.text("title", &[]).unwrap(), "Say \"hi\" {x}");
    assert_eq!(d.prompts("legend").unwrap()[1].glyphs, [Glyph::DpadLeft, Glyph::DpadRight]);
    assert_eq!(d.row_text(0), Some("Count"));
}

#[test]
fn a_doc_naming_what_does_not_exist_is_a_named_error() {
    let base = "screen: t\nkind: list\nlayout: controls\nbacking: shell\n";
    for (extra, at) in [
        ("widgets:\n  title:\n    component: nope\n", "widgets.title.component"),
        ("widgets:\n  legend:\n    component: legend\n    prompts: [[Z, \"GO\"]]\n", "widgets.legend.prompts.0"),
        ("widgets:\n  legend:\n    component: legend\n    prompts: [[A, \"Go\"]]\n", "widgets.legend.prompts.0"),
        ("widgets:\n  title:\n    text: \"Hi {name\"\n", "widgets.title.text"),
        ("rows:\n  row.0:\n    kind: value\n    text: \"X\"\n    values: [\"A\"]\n    current: 3\n", "rows.row.0.values"),
        ("rows:\n  first:\n    text: \"X\"\n", "rows.first"),
    ] {
        let e = doc(&format!("{base}{extra}")).unwrap_err();
        assert_eq!(e.at, at, "{e}");
    }
    assert_eq!(doc("screen: t\nkind: list\nlayout: large\n").unwrap_err().at, "layout");
    assert_eq!(doc("screen: t\nkind: shelf\nlayout: controls\n").unwrap_err().at, "kind");
    assert_eq!(doc("screen: t\nkind: dialog\nlayout: large\nbacking: over\nopener: attic\n").unwrap_err().at, "opener");
    assert_eq!(ScreenDoc::parse("u", "screen: t\nkind: list\nlayout: controls\n", design()).unwrap_err().at, "screen");
    assert!(doc("screen: t\nkind: list\nlayout: controls\nsurprise: 1\n").is_err(), "an unknown key");
}

#[test]
fn slots_are_filled_or_named_as_missing() {
    assert_eq!(slots_of("Sign in as {gamertag} {{x}}").unwrap(), ["gamertag"]);
    assert_eq!(fill("Hi {name}", &[("name", "Chief")]).unwrap(), "Hi Chief");
    assert!(fill("Hi {name}", &[]).unwrap_err().contains("{name}"));
    assert!(slots_of("Hi {").is_err() && slots_of("}").is_err() && slots_of("{}").is_err());
}

/// The editor's reload shows over the baked doc until forgotten. It uses
/// a screen of its own, so the tests reading baked docs alongside it
/// never see it.
#[test]
fn a_reloaded_doc_shows_until_the_reloads_are_forgotten() {
    let name = "test.reloaded";
    let mut d = doc("screen: t\nkind: list\nlayout: controls\nwidgets:\n  title:\n    component: title\n    text: \"Edited\"\n").unwrap();
    d.screen = name.into();
    assert!(screen(name).is_none());
    reload(d.clone()).unwrap();
    assert_eq!(screen(name).unwrap().widget("title").unwrap().text.as_deref(), Some("Edited"));
    forget_reloads();
    assert!(screen(name).is_none());
    d.widgets.get_mut("title").unwrap().component = Some("nope".into());
    assert!(reload(d).is_err(), "a reload is validated");
}

#[test]
fn a_doc_path_is_its_screen_under_assets_ui() {
    assert_eq!(path_of("coop.join"), std::path::Path::new("assets/ui/screens/coop.join.yaml"));
}

/// The tokens are the numbers the kit's geometry draws with: the Controls
/// header's font and colour, row pitch and alphas.
#[test]
fn the_design_tokens_are_the_geometry_the_kit_draws() {
    use crate::ui_kit::spec::ListPlace;
    use crate::ui_kit::view::{FRAME_DIM, Geometry, UNFOCUSED_ALPHA};
    let d = design();
    let g = Geometry::fallback(ListPlace::Controls);
    assert_eq!(d.text_style("title").unwrap(), (g.header_font, g.header_colour));
    assert_eq!(d.colour("pale_blue").unwrap(), g.item_colour);
    assert_eq!(d.font("row_controls").unwrap(), g.label.1);
    assert_eq!(d.spacing("row_controls").unwrap(), g.item_height);
    assert_eq!(d.alpha["unfocused"], UNFOCUSED_ALPHA);
    assert_eq!(d.alpha["frame_dim"], FRAME_DIM);
}

#[test]
fn the_screens_drawn_from_their_docs_are_screens_with_docs() {
    for name in FROM_DOC {
        assert!(NAMES.contains(&name), "{name} is not in screens::NAMES");
        require(name).unwrap_or_else(|e| panic!("{e}"));
        assert!(screens::live(name).is_some(), "{name}");
    }
}
