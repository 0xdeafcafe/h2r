//! A HUD as data (`data`): each YAML of the first-party packages is the HUD
//! the Rust of `expected` built, element by element, and a YAML that does
//! not make a HUD is refused by name. Invented text only.
use crate::data::{BUILTIN, builtin, parse};
use crate::expected;
use crate::model::{Anchor, Art, Hud, Kind, Origin, PillPart, Place};
use crate::views;

fn rust_hud(name: &str) -> Option<Hud> {
    Some(match name {
        "superbounce" => expected::superbounce::hud(),
        "superbounce-readout" => expected::superbounce_readout::hud(),
        "pill-block" => expected::pill_block::hud(),
        "forge" => expected::forge::hud(),
        "forge-readout" => expected::forge_readout::hud(),
        "firefight" => expected::firefight::hud(),
        // The display HUDs are YAML first (`views::objective`'s tests).
        "base" | "ctf" | "assault" | "oddball" | "koth" | "territories" | "siege" | "script-objective" => return None,
        other => panic!("no expected HUD {other}"),
    })
}

#[test]
fn each_yaml_is_the_hud_its_rust_was() {
    for (name, yaml) in BUILTIN {
        let got = parse(yaml).unwrap_or_else(|e| panic!("{name}: {e}"));
        let Some(want) = rust_hud(name) else { continue };
        assert_eq!(got.name, want.name, "{name}");
        assert_eq!(got.elements.len(), want.elements.len(), "{name}: how many elements");
        for (g, w) in got.elements.iter().zip(&want.elements) {
            assert_eq!(g, w, "{name}: element {}", w.id);
        }
        assert_eq!(got, want, "{name}");
        // The HUD the views serve is the same one.
        assert_eq!(builtin(name), want, "{name}");
    }
}

#[test]
fn the_views_serve_the_yaml() {
    assert_eq!(views::superbounce::hud().elements.len(), 6);
    assert_eq!(views::superbounce_readout::hud().elements.len(), 1 + 9 + 4 + 3 + 5);
    assert_eq!(views::forge::hud().elements.len(), 14 + 4 + 3);
    assert_eq!(views::forge_readout::hud().elements.len(), 3 + 5 + 12 + 8);
    assert_eq!(views::firefight::hud().elements.len(), 12 + 1 + 3 + 5);
    assert_eq!(views::pill_block::hud().elements.len(), 4);
    for d in views::all() {
        let hud = d.hud();
        assert_eq!(hud.problems(), Vec::<String>::new(), "{}", hud.name);
    }
}

#[test]
fn a_channel_with_no_place_is_the_games() {
    let hud = parse("name: t\nelements:\n  - {id: a, kind: prompt, longest: hi}\n").unwrap();
    assert_eq!(hud.elements[0].at, Place::GAME);
    assert!(matches!(hud.elements[0].kind, Kind::Prompt { longest: "hi" }));
}

#[test]
fn a_place_reads_its_origin_and_registration() {
    let yaml = "name: t\nelements:\n  - id: a\n    kind: icon\n    art: {type: grenade-pill, part: end}\n    at: {anchor: bottom-left, full: [1, -2], half: [3, 4], registration: [0.5, 0.5], split_origin: next-grenade-pill}\n";
    let e = &parse(yaml).unwrap().elements[0];
    assert_eq!(e.kind, Kind::Icon { art: Art::GrenadePill(PillPart::End) });
    assert_eq!(e.at.anchor, Anchor::BottomLeft);
    assert_eq!((e.at.full, e.at.half, e.at.quarter), ((1, -2), Some((3, 4)), None));
    assert_eq!(e.at.registration, [0.5, 0.5]);
    assert_eq!((e.at.origin, e.at.split_origin), (Origin::Anchor, Some(Origin::NextGrenadePill)));
}

fn refused(yaml: &str, needle: &str) {
    let err = parse(yaml).expect_err(yaml).to_string();
    assert!(err.contains(needle), "{needle:?} not in {err:?}");
}

#[test]
fn a_yaml_that_is_not_a_hud_is_refused_by_name() {
    refused("name: t\nelements: []\nextra: 1\n", "extra");
    refused("name: ''\nelements: []\n", "no name");
    refused(
        "name: t\nelements:\n  - {id: a, kind: prompt, longest: x}\n  - {id: a, kind: prompt, longest: y}\n",
        "a is named twice",
    );
    refused("name: t\nelements:\n  - {id: a, kind: prompt}\n", "needs `longest`");
    refused(
        "name: t\nelements:\n  - {id: a, kind: text, longest: x, art: {type: strip-meter}, at: {full: [0, 0]}}\n",
        "takes no `art`",
    );
    refused("name: t\nelements:\n  - {id: a, kind: meter, at: {full: [0, 0]}}\n", "needs `art`");
    refused("name: t\nelements:\n  - {id: a, kind: text, longest: x}\n", "needs `at`");
    refused(
        "name: t\nelements:\n  - {id: a, kind: meter, art: {type: strip-meter-end}, at: {full: [0, 0]}}\n",
        "needs `columns`",
    );
    refused(
        "name: t\nelements:\n  - {id: a, kind: icon, art: {type: strip-meter, columns: 3}, at: {full: [0, 0]}}\n",
        "takes no `columns`",
    );
    refused("name: t\nelements:\n  - {id: a, kind: world-mark}\n", "needs `shape`");
    refused("name: t\nelements:\n  - {id: a, kind: sparkle}\n", "sparkle");
    refused("name: t\nelements: [", "line");
}
