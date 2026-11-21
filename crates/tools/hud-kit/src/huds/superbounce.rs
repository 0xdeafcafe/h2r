//! The superbounce HUD on the kit (docs/game/hud-kit.md, "The superbounce
//! HUD on the kit"), made of things Halo 2 already draws:
//!
//! - a pill in the grenade row, the next one after the plasma and frag
//!   pills: a short pill cut from the frag pill's own art (its body and
//!   its right end, without the frag "x" and picture), with a chevron
//!   where a pill's count sits: lit while primed, inactive while not, dim
//!   while the map is scanned, hidden on a map with no strips;
//! - a navpoint over the nearest strip's stand point, the game's arrow
//!   pointing down at it and its waypoint marker above, with the distance
//!   in whole metres over them: lit while primed, dim while not;
//! - the tutorial in the game's training box.
//!
//! [`View`] is what the HUD shows of `superbounce_mode`'s state; slice 6
//! fills it from `State` each tick. Launch and drop are not on the HUD:
//! they are on the strips' labels in the world and in the agent API.

use crate::model::{
    Art, Describe, Element, Font, Hud, Justify, Kind, Origin, PILL_BODY, PillPart, Place, Style,
    Value, Values,
};

pub const NAME: &str = "superbounce";

pub const PILL: &str = "sb.pill";
pub const PILL_END: &str = "sb.pill.end";
pub const PILL_ICON: &str = "sb.pill.icon";
pub const NAVPOINT: &str = "sb.navpoint";
pub const TUTORIAL: &str = "sb.tutorial";

/// The pill's icon: the HUD font's chevron (`^` in `conduit-13`, the font
/// of the grenade counts; its arrow characters U+2191 and U+25B2 are the
/// missing box, and its d-pad glyph U+E108 is swapped for the bound key's
/// picture on the other input sets, docs/input/button-icons.md).
pub const ICON_TEXT: &str = "^";
/// The widest navpoint number: whole metres under 1000.
pub const DISTANCE_LONGEST: &str = "999";
/// The distance the preview's navpoint shows.
pub const SAMPLE_DISTANCE_M: f32 = 12.0;

/// The four tutorial steps, two lines each (X is the font's button glyph,
/// U+E102).
pub const TUTORIAL_TEXT: [&str; 4] = [
    "Crouch under a cyan crouch spot,\nthen stand up.",
    "You are primed.\nDo not crouch again.",
    "Find a strip. Green is a standing jump.\nRed needs a big drop.",
    "Land on the strip.\nHold \u{e102} while falling.",
];

/// The scan of the map for strips.
#[derive(Clone, Debug, PartialEq)]
pub enum Scan {
    Scanning,
    Ready,
    /// Done, and the map has none.
    NoStrips,
    Failed,
}

/// What the HUD shows.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub primed: bool,
    pub scan: Scan,
    /// Metres to the nearest strip's stand point; `None` without one.
    pub nearest_m: Option<f32>,
    /// The tutorial step shown, 1 to 4.
    pub tutorial: Option<u8>,
}

/// A distance as the navpoint shows it: whole metres, as the game's
/// counts are whole (the count fonts have no `.`), clamped to
/// [`DISTANCE_LONGEST`].
pub fn distance_text(m: f32) -> String {
    if m.is_nan() {
        return "0".into();
    }
    format!("{:.0}", m.clamp(0.0, 999.0))
}

/// The elements, in draw order.
pub fn hud() -> Hud {
    Hud {
        name: NAME,
        elements: vec![
            Element::icon(
                PILL,
                Art::GrenadePill(PillPart::Body),
                Place::in_grenade_row(Origin::NextGrenadePill),
            ),
            // The right end, against the body's right edge.
            Element::icon(
                PILL_END,
                Art::GrenadePill(PillPart::End),
                Place {
                    full: (PILL_BODY, 0),
                    half: Some((PILL_BODY, 0)),
                    quarter: Some((PILL_BODY, 0)),
                    ..Place::in_grenade_row(Origin::NextGrenadePill)
                },
            ),
            // Centred where a pill's count is, as the counts are.
            Element::text(
                PILL_ICON,
                Font::Hud,
                Justify::Centre,
                ICON_TEXT,
                Place::in_grenade_row(Origin::NextGrenadeCount),
            )
            .over(PILL),
            Element {
                at: Place::GAME.split((0, 0), (0, 0)),
                ..Element::channel(
                    NAVPOINT,
                    Kind::Navpoint {
                        longest: DISTANCE_LONGEST,
                    },
                )
            },
            Element::channel(
                TUTORIAL,
                Kind::Training {
                    longest: TUTORIAL_TEXT[2],
                },
            ),
        ],
    }
}

/// The values for `v`: every element of [`hud`], once.
pub fn values(v: &View) -> Values {
    let mut out = Values::default();
    let pill = match v.scan {
        Scan::NoStrips | Scan::Failed => None,
        Scan::Scanning => Some(Style::Dim),
        Scan::Ready if v.primed => Some(Style::Normal),
        Scan::Ready => Some(Style::Inactive),
    };
    let (p, icon) = match pill {
        Some(style) => (
            Value::shown().styled(style),
            Value::text(ICON_TEXT).styled(style),
        ),
        None => (Value::hidden(), Value::hidden()),
    };
    out.set(PILL, p.clone());
    out.set(PILL_END, p);
    out.set(PILL_ICON, icon);
    out.set(
        NAVPOINT,
        match v.nearest_m.filter(|_| v.scan == Scan::Ready) {
            Some(m) => Value::text(distance_text(m)).styled(if v.primed {
                Style::Normal
            } else {
                Style::Dim
            }),
            None => Value::hidden(),
        },
    );
    out.set(
        TUTORIAL,
        match v.tutorial {
            Some(step @ 1..=4) => Value::text(TUTORIAL_TEXT[usize::from(step - 1)]),
            _ => Value::hidden(),
        },
    );
    out
}

/// The superbounce HUD's description.
pub struct Superbounce;

impl Describe for Superbounce {
    fn hud(&self) -> Hud {
        hud()
    }

    /// The four states and the four tutorial steps.
    fn states(&self) -> Vec<(&'static str, Values)> {
        let not_primed = View {
            primed: false,
            scan: Scan::Ready,
            nearest_m: Some(SAMPLE_DISTANCE_M),
            tutorial: None,
        };
        let primed = View {
            primed: true,
            ..not_primed.clone()
        };
        let scanning = View {
            scan: Scan::Scanning,
            nearest_m: None,
            ..not_primed.clone()
        };
        let no_strips = View {
            scan: Scan::NoStrips,
            nearest_m: None,
            ..not_primed.clone()
        };
        let step = |v: &View, n| View {
            tutorial: Some(n),
            ..v.clone()
        };
        vec![
            ("not-primed", values(&not_primed)),
            ("primed", values(&primed)),
            ("scanning", values(&scanning)),
            ("no-strips", values(&no_strips)),
            ("tutorial-1", values(&step(&not_primed, 1))),
            ("tutorial-2", values(&step(&primed, 2))),
            ("tutorial-3", values(&step(&primed, 3))),
            ("tutorial-4", values(&step(&primed, 4))),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str) -> Values {
        Superbounce
            .states()
            .into_iter()
            .find(|(n, _)| *n == name)
            .unwrap()
            .1
    }

    #[test]
    fn the_distance_is_whole_metres() {
        assert_eq!(distance_text(12.4), "12");
        assert_eq!(distance_text(12.6), "13");
        assert_eq!(distance_text(4000.0), "999");
        assert_eq!(distance_text(f32::NAN), "0");
        assert!(!distance_text(3.4).contains('.'));
    }

    /// The pill is lit primed, inactive not primed, dim scanning, hidden
    /// with no strips; the navpoint lit primed, dim not, hidden without a
    /// strip.
    #[test]
    fn the_pill_and_the_navpoint_follow_the_state() {
        let style = |s: &str, id: &str| {
            let v = state(s);
            let x = v.get(id).unwrap();
            x.shown.then_some(x.style)
        };
        assert_eq!(style("primed", PILL), Some(Style::Normal));
        assert_eq!(style("not-primed", PILL_END), Some(Style::Inactive));
        assert_eq!(style("no-strips", PILL_END), None);
        assert_eq!(style("primed", PILL_ICON), Some(Style::Normal));
        assert_eq!(style("not-primed", PILL), Some(Style::Inactive));
        assert_eq!(style("scanning", PILL), Some(Style::Dim));
        assert_eq!(style("no-strips", PILL), None);
        assert_eq!(style("no-strips", PILL_ICON), None);
        assert_eq!(style("primed", NAVPOINT), Some(Style::Normal));
        assert_eq!(style("not-primed", NAVPOINT), Some(Style::Dim));
        assert_eq!(style("scanning", NAVPOINT), None);
        assert_eq!(style("no-strips", NAVPOINT), None);
        assert_eq!(state("primed").get(NAVPOINT).unwrap().text, "12");
        assert_eq!(state("primed").get(PILL_ICON).unwrap().text, ICON_TEXT);
    }

    /// No state carries a "Hold X" hint or a launch or drop number; the
    /// fourth tutorial step still names the button.
    #[test]
    fn the_hud_has_no_numbers_but_the_distance() {
        let h = hud();
        let ids: Vec<_> = h.elements.iter().map(|e| e.id).collect();
        assert_eq!(ids, [PILL, PILL_END, PILL_ICON, NAVPOINT, TUTORIAL]);
        for (name, v) in Superbounce.states() {
            for (id, val) in &v.0 {
                if *id != TUTORIAL {
                    assert!(!val.text.contains('\u{e102}'), "{name} {id}");
                    assert!(!val.text.contains('.'), "{name} {id}");
                }
            }
        }
        assert!(TUTORIAL_TEXT[3].contains('\u{e102}'));
    }

    #[test]
    fn every_state_is_one_layout_and_the_tutorial_shows_in_its_steps() {
        let h = hud();
        let states = Superbounce.states();
        let names: Vec<_> = states.iter().map(|s| s.0).collect();
        assert_eq!(
            names,
            [
                "not-primed",
                "primed",
                "scanning",
                "no-strips",
                "tutorial-1",
                "tutorial-2",
                "tutorial-3",
                "tutorial-4"
            ]
        );
        for (name, v) in &states {
            assert_eq!(v.layout_problems(&h), Vec::<String>::new(), "{name}");
            let t = v.get(TUTORIAL).unwrap();
            assert_eq!(t.shown, name.starts_with("tutorial"), "{name}");
        }
    }
}
