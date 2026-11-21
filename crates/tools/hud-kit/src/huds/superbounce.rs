//! The superbounce HUD on the kit (docs/game/hud-kit.md, "The superbounce
//! HUD on the kit"): a block bottom left, above the shield meter and the
//! tracker, and the tutorial in the game's training box. Fullscreen only.
//!
//! [`View`] is what the HUD shows of `superbounce_mode`'s state; slice 6
//! fills it from `State` each tick. The "Hold X" hint is not in the
//! block: it is drawn in the world, on the strip.

use crate::model::{
    Anchor, Art, Describe, Element, Font, Hud, Justify, Kind, Place, Pulse, Style, Value, Values,
};

pub const NAME: &str = "superbounce";

pub const STATUS: &str = "sb.status";
pub const POINTER: &str = "sb.pointer";
pub const DISTANCE: &str = "sb.distance";
pub const DISTANCE_UNIT: &str = "sb.distance.unit";
pub const DROP_LABEL: &str = "sb.drop.label";
pub const DROP: &str = "sb.drop";
pub const DROP_UNIT: &str = "sb.drop.unit";
pub const LAUNCH_METER: &str = "sb.launch.meter";
pub const LAUNCH: &str = "sb.launch";
pub const TUTORIAL: &str = "sb.tutorial";

/// The status line while primed: the pair word's -3 is shown (the user's
/// choice).
pub const PRIMED_TEXT: &str = "PRIMED -3";
pub const NOT_PRIMED_TEXT: &str = "NOT PRIMED";
pub const SCANNING_TEXT: &str = "SCANNING";
pub const NO_STRIPS_TEXT: &str = "NO STRIPS HERE";
pub const SCAN_FAILED_TEXT: &str = "SCAN FAILED";
/// The widest status text.
pub const STATUS_LONGEST: &str = NO_STRIPS_TEXT;
pub const DROP_LABEL_TEXT: &str = "DROP";

/// The unit distances and drops are shown in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Metres,
    WorldUnits,
}

/// The drop's unit: metres, as the distance (the user's choice).
pub const DROP_IN: Unit = Unit::Metres;
/// The unit's label after a number in metres.
pub const METRES: &str = "m";
/// Metres in a world unit (10 feet; `superbounce_mode::METRES_PER_UNIT`).
pub const METRES_PER_UNIT: f32 = 3.048;
/// The launch, in world units a second, at which the meter is full.
pub const LAUNCH_FULL: f32 = 30.0;
/// The widest number: one decimal under 100.
pub const NUMBER_LONGEST: &str = "99.9";

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

/// The nearest strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Nearest {
    /// To its nearest edge, metres.
    pub distance_m: f32,
    /// World units a second.
    pub launch: f32,
    /// The fall it needs, world units.
    pub fall: f32,
}

/// What the HUD shows.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub primed: bool,
    pub scan: Scan,
    pub nearest: Option<Nearest>,
    /// Radians from the unit's aim, clockwise; `None` without an aim or
    /// standing on the strip.
    pub bearing: Option<f32>,
    pub airborne: bool,
    /// The tutorial step shown, 1 to 4.
    pub tutorial: Option<u8>,
    /// The tick the player became primed (the status's flash).
    pub primed_tick: Option<u32>,
}

/// The launch meter's fill: 0 at 0, full at [`LAUNCH_FULL`], clamped; 0
/// for a NaN.
pub fn gauge_fill(launch: f32) -> f32 {
    if launch.is_nan() {
        0.0
    } else {
        (launch / LAUNCH_FULL).clamp(0.0, 1.0)
    }
}

/// A length in metres: one decimal under 100, whole above.
pub fn metres_text(m: f32) -> String {
    if m < 99.95 {
        format!("{m:.1}")
    } else {
        format!("{m:.0}")
    }
}

/// The drop's number in [`DROP_IN`].
pub fn drop_text(fall_units: f32) -> String {
    match DROP_IN {
        Unit::Metres => metres_text(fall_units * METRES_PER_UNIT),
        Unit::WorldUnits => format!("{fall_units:.1}"),
    }
}

/// The launch to one decimal.
pub fn launch_text(launch: f32) -> String {
    format!("{launch:.1}")
}

/// The elements, in draw order. Rows are 22 apart for a 21-pixel line;
/// the block's bottom is y 333, above the Halo 1 health strip at 336.
pub fn hud() -> Hud {
    let bl = |x, y| Place::new(Anchor::BottomLeft, (x, y));
    let mut elements = vec![
        Element::text(
            STATUS,
            Font::Hud,
            Justify::Start,
            STATUS_LONGEST,
            bl(-1, -172),
        ),
        Element::pointer(
            POINTER,
            Art::WaypointArrow,
            bl(7, -140).registered([0.5, 0.5]),
        ),
        Element::text(
            DISTANCE,
            Font::Lcd,
            Justify::End,
            NUMBER_LONGEST,
            bl(62, -148),
        ),
        Element::text(
            DISTANCE_UNIT,
            Font::Hud,
            Justify::Start,
            METRES,
            bl(65, -150),
        ),
        Element::text(
            DROP_LABEL,
            Font::Hud,
            Justify::Start,
            DROP_LABEL_TEXT,
            bl(86, -150),
        ),
        Element::text(
            DROP,
            Font::Lcd,
            Justify::Start,
            NUMBER_LONGEST,
            bl(128, -148),
        ),
    ];
    if DROP_IN == Unit::Metres {
        // After the widest drop ("99.9" is 44 wide in font 8 at x 176).
        elements.push(Element::text(
            DROP_UNIT,
            Font::Hud,
            Justify::Start,
            METRES,
            bl(175, -150),
        ));
    }
    elements.extend([
        Element::meter(LAUNCH_METER, Art::StripMeter, bl(-1, -124)),
        Element::text(
            LAUNCH,
            Font::Lcd,
            Justify::Start,
            NUMBER_LONGEST,
            bl(97, -128),
        ),
        Element::channel(
            TUTORIAL,
            Kind::Training {
                longest: TUTORIAL_TEXT[2],
            },
        ),
    ]);
    Hud {
        name: NAME,
        elements,
    }
}

/// The values for `v`: every element of [`hud`], once.
pub fn values(v: &View) -> Values {
    let mut out = Values::default();
    let (status, style) = match v.scan {
        Scan::Scanning => (SCANNING_TEXT, Style::Flash(Pulse::Help)),
        Scan::NoStrips => (NO_STRIPS_TEXT, Style::Dim),
        Scan::Failed => (SCAN_FAILED_TEXT, Style::Dim),
        Scan::Ready if v.primed => (
            PRIMED_TEXT,
            if v.primed_tick.is_some() {
                Style::Flash(Pulse::Objective)
            } else {
                Style::Normal
            },
        ),
        Scan::Ready => (NOT_PRIMED_TEXT, Style::Dim),
    };
    let mut s = Value::text(status).styled(style);
    s.since_tick = v.primed_tick.unwrap_or(0);
    out.set(STATUS, s);
    let near = v.nearest.filter(|_| v.scan == Scan::Ready);
    let or_hide = |show: bool, val: Value| if show { val } else { Value::hidden() };
    out.set(
        POINTER,
        match (near, v.bearing) {
            (Some(_), Some(b)) => Value::angle(b),
            _ => Value::hidden(),
        },
    );
    out.set(
        DISTANCE,
        near.map_or_else(Value::hidden, |n| Value::text(metres_text(n.distance_m))),
    );
    out.set(DISTANCE_UNIT, or_hide(near.is_some(), Value::text(METRES)));
    out.set(
        DROP_LABEL,
        or_hide(near.is_some(), Value::text(DROP_LABEL_TEXT)),
    );
    out.set(
        DROP,
        near.map_or_else(Value::hidden, |n| Value::text(drop_text(n.fall))),
    );
    if DROP_IN == Unit::Metres {
        out.set(DROP_UNIT, or_hide(near.is_some(), Value::text(METRES)));
    }
    out.set(
        LAUNCH_METER,
        near.map_or_else(Value::hidden, |n| Value::fill(gauge_fill(n.launch))),
    );
    out.set(
        LAUNCH,
        near.map_or_else(Value::hidden, |n| Value::text(launch_text(n.launch))),
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

    /// The five states and the four tutorial steps.
    fn states(&self) -> Vec<(&'static str, Values)> {
        let not_primed = View {
            primed: false,
            scan: Scan::Ready,
            nearest: Some(Nearest {
                distance_m: 12.4,
                launch: 17.9,
                fall: 1.1,
            }),
            bearing: Some(0.6),
            airborne: false,
            tutorial: None,
            primed_tick: None,
        };
        let primed = View {
            primed: true,
            primed_tick: Some(0),
            ..not_primed.clone()
        };
        let airborne = View {
            airborne: true,
            nearest: Some(Nearest {
                distance_m: 3.1,
                launch: 24.6,
                fall: 0.4,
            }),
            bearing: Some(-0.2),
            primed_tick: None,
            ..primed.clone()
        };
        let scanning = View {
            scan: Scan::Scanning,
            nearest: None,
            bearing: None,
            ..not_primed.clone()
        };
        let no_strip = View {
            scan: Scan::NoStrips,
            nearest: None,
            bearing: None,
            ..not_primed.clone()
        };
        let step = |v: &View, n| View {
            tutorial: Some(n),
            ..v.clone()
        };
        vec![
            ("scanning", values(&scanning)),
            ("not-primed", values(&not_primed)),
            ("primed", values(&primed)),
            ("primed-airborne", values(&airborne)),
            ("no-strip", values(&no_strip)),
            ("tutorial-1", values(&step(&not_primed, 1))),
            ("tutorial-2", values(&step(&primed, 2))),
            ("tutorial-3", values(&step(&primed, 3))),
            ("tutorial-4", values(&step(&airborne, 4))),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_as_the_page_says() {
        assert_eq!(metres_text(12.44), "12.4");
        assert_eq!(metres_text(99.94), "99.9");
        assert_eq!(metres_text(250.4), "250");
        assert_eq!(drop_text(1.1), "3.4", "metres: 1.1 units is 3.35 m");
        assert_eq!(launch_text(17.85), "17.9");
        assert_eq!(gauge_fill(15.0), 0.5);
        assert_eq!(gauge_fill(45.0), 1.0);
        assert_eq!(gauge_fill(f32::NAN), 0.0);
    }

    #[test]
    fn the_status_reads_primed_minus_three() {
        let d = Superbounce;
        let states = d.states();
        let status = |name: &str| {
            let v = &states.iter().find(|(n, _)| *n == name).unwrap().1;
            v.get(STATUS).unwrap().clone()
        };
        assert_eq!(status("primed").text, "PRIMED -3");
        assert_eq!(status("primed").style, Style::Flash(Pulse::Objective));
        assert_eq!(status("primed-airborne").text, "PRIMED -3");
        assert_eq!(status("primed-airborne").style, Style::Normal);
        assert_eq!(status("not-primed").style, Style::Dim);
        assert_eq!(status("scanning").style, Style::Flash(Pulse::Help));
        assert_eq!(status("no-strip").text, NO_STRIPS_TEXT);
    }

    /// No state of the block carries a "Hold X" hint: it is drawn in the
    /// world. The fourth tutorial step still names the button.
    #[test]
    fn the_block_has_no_hold_x_hint() {
        let h = hud();
        assert!(h.elements.iter().all(|e| !e.id.contains("hint")));
        for (name, v) in Superbounce.states() {
            for (id, val) in &v.0 {
                if *id != TUTORIAL {
                    assert!(!val.text.contains('\u{e102}'), "{name} {id}");
                }
            }
        }
        assert!(TUTORIAL_TEXT[3].contains('\u{e102}'));
    }

    #[test]
    fn distance_and_drop_are_in_metres() {
        let h = hud();
        assert_eq!(DROP_IN, Unit::Metres);
        assert!(h.element(DISTANCE_UNIT).is_some() && h.element(DROP_UNIT).is_some());
        let states = Superbounce.states();
        let v = &states.iter().find(|(n, _)| *n == "not-primed").unwrap().1;
        assert_eq!(v.get(DISTANCE).unwrap().text, "12.4");
        assert_eq!(v.get(DROP).unwrap().text, "3.4");
        assert_eq!(v.get(DROP_UNIT).unwrap().text, "m");
        assert_eq!(v.get(DISTANCE_UNIT).unwrap().text, "m");
    }

    #[test]
    fn every_state_is_one_layout_and_the_tutorial_shows_in_its_steps() {
        let h = hud();
        let states = Superbounce.states();
        let names: Vec<_> = states.iter().map(|s| s.0).collect();
        assert_eq!(
            names,
            [
                "scanning",
                "not-primed",
                "primed",
                "primed-airborne",
                "no-strip",
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
        let scan = &states[0].1;
        assert!(!scan.get(DISTANCE).unwrap().shown && !scan.get(POINTER).unwrap().shown);
    }
}
