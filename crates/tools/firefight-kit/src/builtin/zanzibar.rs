//! Zanzibar: the players hold the fortress against a landing from the
//! beach. One team only, so Crossfire is not offered. Every constant
//! marked `V` is a position the census run settles
//! (`h2 firefight kit zanzibar --layout-report`).
use super::*;

/// V: which team's CTF stand the base building holds. Every start is team
/// 0's: Zanzibar's starts carry no team split, so the beach landing is a
/// weapon pad on the sand.
const HOME: Team = Team::Red;

const STAND: Anchor = Anchor::Flag {
    kind: FlagKind::CtfStand,
    team: Some(HOME),
    nth: 0,
};
/// V: the beach's first plasma rifle pad, on the sand clear of the far
/// stand's base exclusion.
const LANDING: Anchor = Anchor::Item {
    suffix: "plasma_rifle",
    nth: 0,
};
/// The wheel: its hill, else its territory.
const WHEEL: Anchor = Anchor::Or(
    &Anchor::Flag {
        kind: FlagKind::Hill,
        team: None,
        nth: 0,
    },
    &Anchor::Flag {
        kind: FlagKind::Territory,
        team: None,
        nth: 0,
    },
);
/// V: the seaside hut's and the stone ruin's weapon pads.
const HUT_PAD: Anchor = Anchor::Item {
    suffix: "sniper_rifle",
    nth: 0,
};
const RUIN_PAD: Anchor = Anchor::Item {
    suffix: "ammo_rocket_launcher",
    nth: 0,
};
/// V: the sea wall's turret end, as a weapon pad on the wall.
const TURRET: Anchor = Anchor::Item {
    suffix: "h_turret_ap",
    nth: 0,
};
/// V: along the wall from the gate to the water.
const WALL_TO_WATER: [f32; 3] = [0.0, 8.0, 0.0];
const RADIUS: f32 = 4.0;
const WIDE: f32 = 6.0;
const NONE: [f32; 3] = [0.0; 3];

/// Firing positions in a base zone are capped at four on this map, which
/// is short of tag headroom (132 KiB available).
const BASE_POSITIONS: usize = 4;

const fn drop(
    name: &'static str,
    anchor: Anchor,
    offset: [f32; 3],
    radius: f32,
    roles: Roles,
) -> DropSpec {
    DropSpec {
        name,
        anchor,
        offset,
        radius,
        roles,
        sides: Sides::Both,
    }
}

pub static LAYOUT: Layout = Layout {
    map: "zanzibar",
    home: HOME,
    two_teams: false,
    base: BaseSpec {
        anchor: STAND,
        positions: BASE_POSITIONS,
    },
    drops: &[
        drop(
            "beach_landing",
            LANDING,
            NONE,
            WIDE,
            Roles::LINE.with(Roles::HEAVY),
        ),
        drop(
            "sea_wall_gate",
            Anchor::Between(&LANDING, &WHEEL, 0.4),
            NONE,
            WIDE,
            Roles::LINE,
        ),
        drop(
            "sea_wall_broken_end",
            Anchor::Between(&LANDING, &WHEEL, 0.4),
            WALL_TO_WATER,
            WIDE,
            Roles::FLANK,
        ),
        drop("wheel", WHEEL, NONE, RADIUS, Roles::LINE),
        drop("seaside_hut", HUT_PAD, NONE, RADIUS, Roles::FLANK),
        drop("stone_ruin", RUIN_PAD, NONE, RADIUS, Roles::FLANK),
        drop("sea_wall_turret", TURRET, NONE, RADIUS, Roles::LINE),
    ],
    // The balcony wall, front windows and courtyard blocks are the kit's
    // own sampling.
    cover: &[],
};
