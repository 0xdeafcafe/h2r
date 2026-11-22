use super::*;
use crate::ui_kit::screens::NAMES;

/// Every specimen and layout page is a screen of `NAMES`, and every
/// `design.`/`layout.` screen is a page.
#[test]
fn the_pages_are_the_design_and_layout_screens() {
    for p in specimens::PAGES {
        assert!(NAMES.contains(&p), "{p} is not in screens::NAMES");
        assert!(p.starts_with("design.") || p.starts_with("layout."), "{p}");
        assert!(specimens::states(p).is_some_and(|s| !s.is_empty()), "{p} has no states");
    }
    for n in NAMES.iter().filter(|n| n.starts_with("design.") || n.starts_with("layout.")) {
        assert!(specimens::PAGES.contains(n), "{n} has no page");
    }
}

/// A component's variants are its specimen page's states, in order.
#[test]
fn each_component_variant_is_a_state_of_its_page() {
    for (name, c) in &crate::ui_kit::data::design().components {
        let Some(page) = &c.specimen else { continue };
        let states: Vec<String> = specimens::states(page)
            .unwrap_or_else(|| panic!("{name}: no page {page}"))
            .into_iter()
            .map(|(s, _)| s)
            .collect();
        assert_eq!(states, c.variants, "{name} on {page}");
    }
}

#[test]
fn a_legend_is_glyphs_one_space_words_in_capitals_two_spaces_apart() {
    let a = Glyph::A.char();
    let b = Glyph::B.char();
    let l = Glyph::DpadLeft.char();
    let r = Glyph::DpadRight.char();
    for good in [
        format!("{a} SELECT"),
        format!("{a} SELECT  {b} BACK"),
        format!("{l}  {r} CHANGE  {b} BACK"),
        format!("{a} START LEVEL  {b} BACK"),
    ] {
        assert_eq!(legend::well_formed(&good, 2, 2), Ok(()), "{good:?}");
    }
    for bad in [
        "SELECT".to_string(),
        format!("{a} Select"),
        format!("{a}  SELECT"),
        format!("{a} SELECT   {b} BACK"),
        format!("{a} SELECT {b} BACK"),
        format!("{a} SELECT  "),
    ] {
        assert!(legend::well_formed(&bad, 2, 2).is_err(), "{bad:?}");
    }
}

#[test]
fn a_screen_takes_the_inputs_its_rows_and_legend_need() {
    let s = Screen::list("t", "T")
        .row(row::action("Go"))
        .legend(vec![legend::select(), legend::back()])
        .build();
    assert_eq!(s.inputs, [Input::Up, Input::Down, Input::A, Input::B]);
    let s = Screen::list("t", "T")
        .row(row::value("Count", &["1", "2"], 0))
        .legend(vec![legend::change(), Prompt::new(&[Glyph::Y], "Reset")])
        .build();
    for i in [Input::Left, Input::Right, Input::Y] {
        assert!(s.inputs.contains(&i), "{i:?}");
    }
    let hud = Screen::hud("t", "T", vec![]).build();
    assert!(hud.inputs.is_empty());
}

/// A specimen page takes every input it lists and ignores what it doesn't.
#[test]
fn a_specimen_steps_its_rows_and_ignores_what_it_does_not_take() {
    let mut p = live("design.row.value-inline").unwrap();
    let before = p.spec();
    p.press(Input::X);
    assert_eq!(p.spec(), before, "X is not a value row's input");
    p.press(Input::Right);
    assert_ne!(p.spec(), before, "right steps the focused value");
}

#[test]
fn the_catalogue_lists_every_component_and_layout() {
    let c = catalogue_json();
    let names: Vec<&str> = c["components"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    for n in ["title", "legend", "description", "row.action", "picker", "cells"] {
        assert!(names.contains(&n), "{n}");
    }
    let layouts = c["layouts"].as_array().unwrap();
    assert_eq!(layouts.len(), NAMES.iter().filter(|n| n.starts_with("layout.")).count());
    assert!(layouts.iter().all(|l| l["kind"].is_string() && !l["states"].as_array().unwrap().is_empty()));
}
