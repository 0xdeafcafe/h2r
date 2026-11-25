//! The HUDs as the Rust that preceded their YAML built them: the
//! expected values the YAML is held to (`tests::each_yaml_is_the_hud_its_rust_was`).
//! Test-only; the layout constants it reads are the views' own.
//!
//! A HUD's element list is data now (`content/<package>/huds/`, read by
//! [`crate::data`]); this is the list the code gave, element by element, so a
//! YAML that drifts from it fails by element.
use crate::model::{
    Anchor, Art, Element, Font, Hud, HudRef, Justify, Kind, Origin, PILL_BODY, PillPart, Place,
};
use crate::world::Shape;

/// `superbounce`.
pub mod superbounce {
    use super::*;
    use crate::views::superbounce::{
        ARM, ARM_FULL, ARM_HUD, ARM_QUARTER, ARM_SPLIT, ARM_WIDGET, ASSAULT_ARMING_D,
        DISTANCE_LONGEST, MEDAL, MEDAL_LONGEST, MESSAGE, MESSAGE_LONGEST, NAME, NAVPOINT, PROMPT,
        PROMPT_LONGEST, TUTORIAL, TUTORIAL_TEXT,
    };

    pub fn hud() -> Hud {
        Hud {
            name: NAME,
            elements: vec![
                // Assault's arming bar, at the bottom left fullscreen and
                // where split screen has room; it gives way to retail's own
                // in an Assault game.
                Element::meter(
                    ARM,
                    Art::Widget {
                        hud: HudRef::Named(ARM_HUD),
                        name: ARM_WIDGET,
                    },
                    Place::new(Anchor::TopLeft, ARM_FULL)
                        .split(ARM_SPLIT, ARM_QUARTER)
                        .registered([0.5, 0.5]),
                )
                .hidden_on_d(ASSAULT_ARMING_D),
                Element::channel(
                    PROMPT,
                    Kind::Prompt {
                        longest: PROMPT_LONGEST,
                    },
                ),
                Element::channel(
                    MESSAGE,
                    Kind::Message {
                        longest: MESSAGE_LONGEST,
                    },
                ),
                Element::channel(
                    MEDAL,
                    Kind::Message {
                        longest: MEDAL_LONGEST,
                    },
                ),
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
}

/// `superbounce-readout`.
pub mod superbounce_readout {
    use super::*;
    use crate::views::superbounce::DISTANCE_LONGEST;
    use crate::views::{superbounce_guide as guide, superbounce_segments as segs};
    use crate::views::superbounce_readout::{
        FALL, FALL_COLUMNS, FALL_DOWN, FALL_UP, INFO, INFO_LONGEST, INFO_UP, KINDS, KINDS_LONGEST,
        KINDS_UP, LEFT, NAME, SPEED, SPEED_LONGEST, SPEED_UP, STEER, STEER_LEFT, STEER_LONGEST,
        TAG, TARGET_1, TARGET_2,
    };

    pub fn hud() -> Hud {
        let row = |up: i16| Place::new(Anchor::BottomLeft, (LEFT, up));
        let navpoint = |id| {
            Element::channel(
                id,
                Kind::Navpoint {
                    longest: DISTANCE_LONGEST,
                },
            )
        };
        let mut elements = vec![Element::meter(
            FALL,
            Art::StripMeterEnd(FALL_COLUMNS),
            Place::new(Anchor::BottomLeft, (LEFT, FALL_UP + FALL_DOWN)),
        )];
        elements.extend(segment_elements());
        let mut hud = Hud {
            name: NAME,
            elements,
        };
        hud.elements.extend([
            Element::text(
                SPEED,
                Font::Hud,
                Justify::Start,
                SPEED_LONGEST,
                row(SPEED_UP),
            ),
            Element::text(
                STEER,
                Font::Hud,
                Justify::Start,
                STEER_LONGEST,
                Place::new(Anchor::BottomLeft, (STEER_LEFT, FALL_UP)),
            ),
            Element::text(INFO, Font::Hud, Justify::Start, INFO_LONGEST, row(INFO_UP)),
            Element::text(
                KINDS,
                Font::Hud,
                Justify::Start,
                KINDS_LONGEST,
                row(KINDS_UP),
            ),
            navpoint(TAG),
            navpoint(TARGET_1),
            navpoint(TARGET_2),
        ]);
        hud.elements.extend(guide_elements());
        hud
    }

    /// The charge segments' elements, in fill order.
    fn segment_elements() -> Vec<Element> {
        segs::IDS.iter()
            .zip(segs::AT)
            .enumerate()
            .map(|(i, (id, (x, y)))| {
                Element::gauge(
                    id,
                    segs::glyphs(i),
                    Place::new(Anchor::BottomLeft, (x - segs::CORNER.0, y - segs::CORNER.1)),
                )
                .outside(segs::OUTSIDE)
            })
            .collect()
    }

    /// The approach guide's world marks.
    fn guide_elements() -> Vec<Element> {
        let mark = |id, shape, longest| Element::channel(id, Kind::WorldMark { shape, longest });
        vec![
            mark(guide::BOX, Shape::Bracket, guide::BOX_LONGEST),
            mark(guide::PIPPER, Shape::Pipper, ""),
            mark(guide::TICKS, Shape::Ticks, ""),
            mark(guide::DRIFT, Shape::Arrow, ""),
            mark(guide::TAKEOFF, Shape::Bar, ""),
        ]
    }
}

/// `pill-block`.
pub mod pill_block {
    use super::*;
    use crate::views::pill_block::{
        CHARGE, CHARGE_COLUMNS, CHARGE_DOWN, ICON_AT, ICON_TEXT, NAME, PILL, PILL_END, PILL_FULL,
        PILL_ICON,
    };

    const fn place(by: (i16, i16), split: (i16, i16), origin: Origin) -> Place {
        Place::new(Anchor::TopLeft, (PILL_FULL.0 + by.0, PILL_FULL.1 + by.1))
            .split(split, split)
            .split_from(origin)
    }

    pub fn hud() -> Hud {
        Hud {
            name: NAME,
            elements: vec![
                Element::icon(
                    PILL,
                    Art::GrenadePill(PillPart::Body),
                    place((0, 0), (0, 0), Origin::NextGrenadePill),
                ),
                // The right end, against the body's right edge.
                Element::icon(
                    PILL_END,
                    Art::GrenadePill(PillPart::End),
                    place((PILL_BODY, 0), (PILL_BODY, 0), Origin::NextGrenadePill),
                ),
                // Under the pill, as wide as it, in every mode the pill has.
                Element::meter(
                    CHARGE,
                    Art::StripMeterEnd(CHARGE_COLUMNS),
                    place((0, CHARGE_DOWN), (0, CHARGE_DOWN), Origin::NextGrenadePill),
                ),
                // Centred where a pill's count is, as the counts are.
                Element::text(
                    PILL_ICON,
                    Font::Hud,
                    Justify::Centre,
                    ICON_TEXT,
                    place(ICON_AT, (0, 0), Origin::NextGrenadeCount),
                )
                .over(PILL),
            ],
        }
    }
}

/// `forge`.
pub mod forge {
    use super::*;
    use crate::views::forge::{
        BUDGET_COUNT, BUDGET_END, BUDGET_LONGEST, BUDGET_METER, BUDGET_PILL, GIZMO, METER_DOWN,
        NAME, NAV_FROM, NAV_LONGEST, NAV_MONITORS, NAV_TARGET, NOTICE, NOTICE_LONGEST, PROMPT,
        PROMPT_LONGEST, REBUILD_COUNT, REBUILD_END, REBUILD_LONGEST, REBUILD_PILL, REBUILD_STEP,
        TEACH, TEACH_LONGEST, TOOL, TOOL_LONGEST,
    };

    const fn row(origin: Origin, dx: i16, dy: i16) -> Place {
        Place::new(Anchor::TopLeft, (dx, dy))
            .split((dx, dy), (dx, dy))
            .from(origin)
    }

    fn pill(pill: &'static str, end: &'static str, count: &'static str, longest: &'static str, dx: i16) -> [Element; 3] {
        [
            Element::icon(pill, Art::GrenadePill(PillPart::Body), row(Origin::NextGrenadePill, dx, 0)),
            Element::icon(
                end,
                Art::GrenadePill(PillPart::End),
                row(Origin::NextGrenadePill, dx + PILL_BODY, 0),
            ),
            Element::text(
                count,
                Font::Lcd,
                Justify::Centre,
                longest,
                row(Origin::NextGrenadeCount, dx, 0),
            )
            .over(pill),
        ]
    }

    pub fn hud() -> Hud {
        let mut elements = vec![
            Element::channel(PROMPT, Kind::Prompt { longest: PROMPT_LONGEST }),
            Element::channel(NOTICE, Kind::Message { longest: NOTICE_LONGEST }),
            Element::channel(TEACH, Kind::Training { longest: TEACH_LONGEST }),
        ];
        elements.extend(pill(BUDGET_PILL, BUDGET_END, BUDGET_COUNT, BUDGET_LONGEST, 0));
        elements.push(Element::meter(
            BUDGET_METER,
            Art::StripMeter,
            row(Origin::NextGrenadePill, 0, METER_DOWN),
        ));
        elements.push(Element::text(
            TOOL,
            Font::Hud,
            Justify::End,
            TOOL_LONGEST,
            Place::new(Anchor::TopRight, (-6, 6)),
        ));
        elements.extend(pill(REBUILD_PILL, REBUILD_END, REBUILD_COUNT, REBUILD_LONGEST, REBUILD_STEP));
        let nav = |id| Element::channel(id, Kind::Navpoint { longest: NAV_LONGEST });
        elements.extend([NAV_TARGET, NAV_FROM].map(nav));
        elements.extend(NAV_MONITORS.map(nav));
        elements.extend(GIZMO.map(|id| {
            Element::channel(id, Kind::WorldMark { shape: Shape::Segment, longest: "" })
        }));
        Hud {
            name: NAME,
            elements,
        }
    }
}

/// `forge-readout`.
pub mod forge_readout {
    use super::*;
    use crate::views::forge_readout::{
        FACE, FACTS, FACTS_LONGEST, FACTS_UP, FROM, GUIDE, GUIDE_LONGEST, GUIDE_UP, NAME,
        NAME_LONGEST, NAME_ROW, NAME_UP, NEAR_LONGEST, NEARS, RING, SEL, UNDER, ZONES,
    };
    use crate::views::superbounce_readout::LEFT;

    pub fn hud() -> Hud {
        let row = |up: i16| Place::new(Anchor::BottomLeft, (LEFT, up));
        let mark = |id, shape, longest| Element::channel(id, Kind::WorldMark { shape, longest });
        let mut elements = vec![
            Element::text(NAME_ROW, Font::Hud, Justify::Start, NAME_LONGEST, row(NAME_UP)),
            Element::text(FACTS, Font::Hud, Justify::Start, FACTS_LONGEST, row(FACTS_UP)),
            Element::text(GUIDE, Font::Hud, Justify::Start, GUIDE_LONGEST, row(GUIDE_UP)),
            mark(SEL, Shape::Bracket, NAME_LONGEST),
            mark(RING, Shape::Pipper, ""),
            mark(FACE, Shape::Arrow, ""),
            mark(UNDER, Shape::Pipper, ""),
            mark(FROM, Shape::Bracket, ""),
        ];
        elements.extend(NEARS.iter().map(|id| mark(*id, Shape::Pipper, NEAR_LONGEST)));
        elements.extend(ZONES.iter().map(|id| mark(*id, Shape::Line, "")));
        Hud {
            name: NAME,
            elements,
        }
    }
}

/// `firefight`.
pub mod firefight {
    use super::*;
    use crate::views::firefight::{
        BANNER, BANNER_LONGEST, COUNT_LONGEST, ENEMIES_COUNT, ENEMIES_END, ENEMIES_PILL,
        LIVES_COUNT, LIVES_END, LIVES_PILL, NAME, NOTE, NOTE_LONGEST, PILL_STEP,
    };

    const fn row(origin: Origin, dx: i16) -> Place {
        Place::new(Anchor::TopLeft, (dx, 0))
            .split((dx, 0), (dx, 0))
            .from(origin)
    }

    const EVERY_MODE: Place = Place::GAME.split((0, 0), (0, 0));

    fn pill(pill: &'static str, end: &'static str, count: &'static str, dx: i16) -> [Element; 3] {
        [
            Element::icon(
                pill,
                Art::GrenadePill(PillPart::Body),
                row(Origin::NextGrenadePill, dx),
            ),
            Element::icon(
                end,
                Art::GrenadePill(PillPart::End),
                row(Origin::NextGrenadePill, dx + PILL_BODY),
            ),
            // Where a pill's count sits, as the grenade counts.
            Element::text(
                count,
                Font::Lcd,
                Justify::Centre,
                COUNT_LONGEST,
                row(Origin::NextGrenadeCount, dx),
            )
            .over(pill),
        ]
    }

    pub fn hud() -> Hud {
        let mut banner = Element::channel(
            BANNER,
            Kind::Banner {
                longest: BANNER_LONGEST,
            },
        );
        banner.at = EVERY_MODE;
        let mut note = Element::channel(
            NOTE,
            Kind::Message {
                longest: NOTE_LONGEST,
            },
        );
        note.at = EVERY_MODE;
        let mut elements = vec![banner];
        elements.extend(pill(LIVES_PILL, LIVES_END, LIVES_COUNT, 0));
        elements.extend(pill(ENEMIES_PILL, ENEMIES_END, ENEMIES_COUNT, PILL_STEP));
        elements.push(note);
        Hud {
            name: NAME,
            elements,
        }
    }
}
