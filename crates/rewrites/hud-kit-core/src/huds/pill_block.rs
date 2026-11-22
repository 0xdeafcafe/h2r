//! The kit's grenade-row pieces as one HUD, for the kit's own tests
//! (docs/game/hud-kit.md, "The pill"): a short pill cut from the frag
//! pill's art, a text where a pill's count sits, and the battery strip's
//! end under it, beside the motion tracker fullscreen and the grenade
//! row's next pill in split screen. No game draws it: it is not
//! registered ([`super::all`]) and is never compiled into a map. It
//! keeps the parts of the kit no registered HUD uses under test: the
//! pill's crops (`Art::GrenadePill`), a place in the grenade row
//! (`Origin::NextGrenadePill`, `Origin::NextGrenadeCount`), a text drawn
//! `over` a bitmap, and the `Inactive` and `Dim` styles on a bitmap.
//!
//! Its states are the superbounce HUD's, by name
//! ([`super::superbounce::views`]), with the block styled by each: lit
//! primed, inactive not primed, dim while the map is scanned, hidden
//! with no strips; the strip the charge's fill, primed and lost.

use crate::model::{
    self, Anchor, Art, Describe, Element, Font, Hud, Justify, Origin, PILL_BODY, PillPart, Place,
    Pulse, Style, Value, Values,
};

use super::superbounce::{self, LOW_CHARGE, Phase, Scan, View};

pub const NAME: &str = "pill-block";

pub const PILL: &str = "pb.pill";
pub const PILL_END: &str = "pb.pill.end";
pub const PILL_ICON: &str = "pb.pill.icon";
pub const CHARGE: &str = "pb.charge";

/// The icon: the HUD font's chevron (`^` in `conduit-13`, the font of
/// the grenade counts).
pub const ICON_TEXT: &str = "^";
/// The strip's width: its last columns, as wide as the pill
/// ([`PILL_BODY`] and [`model::PILL_END`]).
pub const CHARGE_COLUMNS: u8 = (PILL_BODY + model::PILL_END) as u8;
/// The strip's top below the pill's: the pill's 19 rows (V:
/// `grenade_bkd`, 103 by 19) and 2 more.
pub const CHARGE_DOWN: i16 = 21;
/// The pill's top left fullscreen, from the top-left anchor: screen
/// (138, 376), 8 right of the motion tracker. In half and quarter screen
/// the pill is the grenade row's next (`Origin::NextGrenadePill`), and
/// the row's anchor is the top left: the one anchor the widget has.
pub const PILL_FULL: (i16, i16) = (90, 340);
/// The icon's point from the pill's top left: where a grenade pill's
/// count is in its pill (V: the plasma count at (12, -1), its pill at
/// (0, 0)).
pub const ICON_AT: (i16, i16) = (12, -1);

/// A place of the block: fullscreen `by` from the pill's top left
/// ([`PILL_FULL`]); in half and quarter screen `split` from `origin` in
/// the grenade row.
const fn place(by: (i16, i16), split: (i16, i16), origin: Origin) -> Place {
    Place::new(Anchor::TopLeft, (PILL_FULL.0 + by.0, PILL_FULL.1 + by.1))
        .split(split, split)
        .split_from(origin)
}

/// The elements, in draw order.
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

/// The values for the superbounce HUD's view `v`.
pub fn values(v: &View) -> Values {
    let mut out = Values::default();
    let ready = v.scan == Scan::Ready;
    let pill = match v.scan {
        Scan::NoStrips | Scan::Failed => None,
        Scan::Scanning => Some(Style::Dim),
        Scan::Ready if v.primed => Some(Style::Normal),
        Scan::Ready => Some(Style::Inactive),
    };
    let charge = v.charge.filter(|_| ready);
    let strip = charge.filter(|c| matches!(c.phase, Phase::Primed | Phase::Lost));
    let icon_style = |style: Style| match charge {
        Some(c) if v.primed && c.phase == Phase::Primed && c.fill <= LOW_CHARGE => {
            Style::Flash(Pulse::TimeWarning)
        }
        Some(c) if !v.primed && c.phase == Phase::Held => Style::Flash(Pulse::Help),
        _ => style,
    };
    let (p, icon) = match pill {
        Some(style) => (
            Value::shown().styled(style),
            Value::text(ICON_TEXT).styled(icon_style(style)),
        ),
        None => (Value::hidden(), Value::hidden()),
    };
    out.set(PILL, p.clone());
    out.set(PILL_END, p);
    out.set(
        CHARGE,
        match strip {
            Some(c) => Value::fill(c.fill.clamp(0.0, 1.0)),
            None => Value::hidden(),
        },
    );
    out.set(PILL_ICON, icon);
    out
}

/// The block's description.
pub struct PillBlock;

impl Describe for PillBlock {
    fn hud(&self) -> Hud {
        hud()
    }

    fn states(&self) -> Vec<(&'static str, Values)> {
        superbounce::views()
            .into_iter()
            .map(|(n, v)| (n, values(&v)))
            .collect()
    }

    fn multiplayer_only(&self) -> Option<&'static str> {
        Some("a campaign player HUD may have no grenade row")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str) -> Values {
        PillBlock.states().into_iter().find(|(n, _)| *n == name).unwrap().1
    }

    fn style(s: &str, id: &str) -> Option<Style> {
        let v = state(s);
        let x = v.get(id).unwrap();
        x.shown.then_some(x.style)
    }

    /// The pill is lit primed, inactive not primed, dim scanning, hidden
    /// with no strips; the strip shows primed and lost; the icon flashes
    /// at half the charge and in the held step.
    #[test]
    fn the_block_follows_the_state() {
        let h = hud();
        for (name, v) in PillBlock.states() {
            assert_eq!(v.layout_problems(&h), Vec::<String>::new(), "{name}");
        }
        assert_eq!(style("primed-full", PILL), Some(Style::Normal));
        assert_eq!(style("not-primed", PILL_END), Some(Style::Inactive));
        assert_eq!(style("scanning", PILL), Some(Style::Dim));
        assert_eq!(style("no-strips", PILL), None);
        assert_eq!(style("no-strips", PILL_ICON), None);
        assert_eq!(state("primed-full").get(PILL_ICON).unwrap().text, ICON_TEXT);
        let fill = |s: &str| {
            let v = state(s);
            let c = v.get(CHARGE).unwrap();
            c.shown.then_some(c.fill)
        };
        assert_eq!(fill("not-primed"), None);
        assert_eq!(fill("held"), None);
        assert_eq!(fill("primed-full"), Some(1.0));
        assert_eq!(fill("primed-draining"), Some(0.25));
        assert_eq!(fill("lost"), Some(0.0));
        assert_eq!(fill("primed-still-warning"), Some(LOW_CHARGE));
        assert_eq!(
            style("primed-still-warning", PILL_ICON),
            Some(Style::Flash(Pulse::TimeWarning))
        );
        assert_eq!(style("held", PILL_ICON), Some(Style::Flash(Pulse::Help)));
        assert_eq!(style("under", PILL_ICON), Some(Style::Inactive));
    }

    /// Fullscreen the pill, its end, the strip and the icon are one block
    /// beside the motion tracker, placed from the top-left anchor; in
    /// split screen they are the grenade row's next pill, as its count.
    #[test]
    fn the_block_is_beside_the_tracker_fullscreen_and_in_the_grenade_row_in_split_screen() {
        let h = hud();
        let at = |id: &str| {
            let p = h.element(id).unwrap().at;
            assert_eq!((p.anchor, p.origin), (Anchor::TopLeft, Origin::Anchor), "{id}");
            assert_eq!(p.half, p.quarter, "{id}");
            (p.full, p.half.unwrap(), p.split_origin)
        };
        let pill = Some(Origin::NextGrenadePill);
        assert_eq!(at(PILL), ((90, 340), (0, 0), pill));
        assert_eq!(at(PILL_END), ((110, 340), (20, 0), pill));
        assert_eq!(at(CHARGE), ((90, 361), (0, 21), pill));
        assert_eq!(at(PILL_ICON), ((102, 339), (0, 0), Some(Origin::NextGrenadeCount)));
        assert!(h.uses_grenade_row());
        let p = h.element(PILL).unwrap().at;
        assert_eq!(p.origin_in(crate::Mode::Full), Origin::Anchor);
        assert_eq!(p.origin_in(crate::Mode::Half), Origin::NextGrenadePill);
        assert_eq!(p.origin_in(crate::Mode::Quarter), Origin::NextGrenadePill);
        assert_eq!(usize::from(CHARGE_COLUMNS), 27);
    }
}
