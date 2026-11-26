//! Coagulation: red's base and blue's face each other across the canyon.
//! The map is symmetric, so both teams hunt (Crossfire is offered). Every
//! constant marked `V` is a position the census run settles
//! (`h2 firefight kit coagulation --layout-report`).
use super::*;

const RED: Option<Team> = Some(Team::Red);
const BLUE: Option<Team> = Some(Team::Blue);

const RED_STAND: Anchor = Anchor::Flag {
    kind: FlagKind::CtfStand,
    team: RED,
    nth: 0,
};
const BLUE_STAND: Anchor = Anchor::Flag {
    kind: FlagKind::CtfStand,
    team: BLUE,
    nth: 0,
};
/// The centre hill, else the point halfway between the two flag stands.
const HILL: Anchor = Anchor::Or(
    &Anchor::Flag {
        kind: FlagKind::Hill,
        team: None,
        nth: 0,
    },
    &Anchor::Between(&RED_STAND, &BLUE_STAND, 0.5),
);

/// V: the weapon pad in the cave mouth, and the two at the ends of the
/// cliff ledge (a tag-name suffix and the placement's number among those).
const CAVE_PAD: Anchor = Anchor::Item {
    suffix: "sniper_rifle",
    nth: 0,
};
const LEDGE_PAD_NEAR: Anchor = Anchor::Item {
    suffix: "rocket_launcher",
    nth: 0,
};
const LEDGE_PAD_FAR: Anchor = Anchor::Item {
    suffix: "rocket_launcher",
    nth: 1,
};

/// V: toward the north and south walls from the hill.
const MID_NORTH: [f32; 3] = [0.0, 10.0, 0.0];
const MID_SOUTH: [f32; 3] = [0.0, -10.0, 0.0];
/// V: out of each base's forecourt toward the middle, past the kit's base
/// exclusion (more than 8 units).
const BLUE_FORECOURT: [f32; 3] = [-12.0, 0.0, 0.0];
const RED_FORECOURT: [f32; 3] = [12.0, 0.0, 0.0];
/// V: how far a drop may snap to find a usable sector.
const RADIUS: f32 = 4.0;
const WIDE: f32 = 6.0;

const NONE: [f32; 3] = [0.0; 3];

const fn drop(
    name: &'static str,
    anchor: Anchor,
    offset: [f32; 3],
    radius: f32,
    roles: Roles,
    sides: Sides,
) -> DropSpec {
    DropSpec {
        name,
        anchor,
        offset,
        radius,
        roles,
        sides,
    }
}

pub static LAYOUT: Layout = Layout {
    map: "coagulation",
    home: Team::Red,
    two_teams: true,
    base: BaseSpec {
        anchor: RED_STAND,
        positions: 8,
    },
    drops: &[
        drop("centre_hill", HILL, NONE, RADIUS, Roles::LINE, Sides::Both),
        drop(
            "mid_rocks_north",
            HILL,
            MID_NORTH,
            RADIUS,
            Roles::LINE,
            Sides::Both,
        ),
        drop(
            "mid_rocks_south",
            HILL,
            MID_SOUTH,
            RADIUS,
            Roles::LINE,
            Sides::Both,
        ),
        drop("cave_mouth", CAVE_PAD, NONE, WIDE, Roles::FLANK, Sides::Both),
        drop(
            "cliff_ledge",
            Anchor::Between(&LEDGE_PAD_NEAR, &LEDGE_PAD_FAR, 0.5),
            NONE,
            WIDE,
            Roles::FLANK,
            Sides::Both,
        ),
        drop(
            "red_teleporter_exit",
            Anchor::Flag {
                kind: FlagKind::TeleporterExit,
                team: RED,
                nth: 0,
            },
            NONE,
            RADIUS,
            Roles::FLANK,
            Sides::Only(Team::Blue),
        ),
        drop(
            "blue_teleporter_exit",
            Anchor::Flag {
                kind: FlagKind::TeleporterExit,
                team: BLUE,
                nth: 0,
            },
            NONE,
            RADIUS,
            Roles::FLANK,
            Sides::Only(Team::Red),
        ),
        drop(
            "blue_forecourt",
            BLUE_STAND,
            BLUE_FORECOURT,
            WIDE,
            Roles::HEAVY,
            Sides::Only(Team::Red),
        ),
        drop(
            "red_forecourt",
            RED_STAND,
            RED_FORECOURT,
            WIDE,
            Roles::HEAVY,
            Sides::Only(Team::Blue),
        ),
    ],
    // The roof parapet, door frames and rocks on the forecourt are the
    // kit's own sampling, wall toward the middle.
    cover: &[],
};
