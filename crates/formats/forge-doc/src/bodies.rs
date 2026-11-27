//! The Forge bodies: the units a player flies as in a Forge session
//! (docs/game/forge-design-slices.md, "The monitor body").
//!
//! A Forge kit carries each body as a unit tag transplanted from the
//! player's own campaign maps (`forge_build`'s `bodies`). D-pad Up makes
//! the chosen body the player's unit; the player's biped is kept and comes
//! back where the body is. The table names each body's tag and the
//! campaign maps that hold it. **V**: the paths and donors come from the
//! campaign's tag names, not a census of the disc; the kit build tries
//! each donor in order and refuses a body none of them gives, by name.

use serde::{Deserialize, Serialize};

/// One of the bodies a Forge kit carries, in the order the kit fits them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Body {
    /// 343 Guilty Spark's monitor. Forge does not open on a map whose kit
    /// lacks it.
    Monitor,
    Sentinel,
    /// A Prophet in his gravity throne.
    Prophet,
}

/// A tag as the kit copies it: its group and its path in the donor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagPath {
    pub group: [u8; 4],
    pub path: &'static str,
}

/// What the kit copies for one body and from where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    pub body: Body,
    /// The unit the player becomes.
    pub unit: TagPath,
    /// The vehicle the unit sits in, for a body that has one.
    pub chair: Option<TagPath>,
    /// Campaign maps that hold the tags, tried in order.
    pub donors: &'static [&'static str],
}

const BIPD: [u8; 4] = *b"bipd";
const VEHI: [u8; 4] = *b"vehi";

/// Every body, in [`Body`] order.
pub const SPECS: [Spec; 3] = [
    Spec {
        body: Body::Monitor,
        unit: TagPath {
            group: BIPD,
            path: r"objects\characters\monitor\monitor",
        },
        chair: None,
        donors: &["06b_floodzone", "08b_deltacontrol", "06a_sentinelwalls"],
    },
    Spec {
        body: Body::Sentinel,
        unit: TagPath {
            group: BIPD,
            path: r"objects\characters\sentinel_aggressor\sentinel_aggressor",
        },
        chair: None,
        donors: &["06a_sentinelwalls", "06b_floodzone", "08b_deltacontrol"],
    },
    Spec {
        body: Body::Prophet,
        unit: TagPath {
            group: BIPD,
            path: r"objects\characters\prophet\prophet_regret",
        },
        chair: Some(TagPath {
            group: VEHI,
            path: r"objects\vehicles\gravity_throne\gravity_throne",
        }),
        donors: &["05b_deltatowers", "07a_highcharity"],
    },
];

impl Body {
    pub const ALL: [Body; 3] = [Body::Monitor, Body::Sentinel, Body::Prophet];

    /// The key in the profile, the agent API and the network.
    pub fn key(self) -> &'static str {
        match self {
            Body::Monitor => "monitor",
            Body::Sentinel => "sentinel",
            Body::Prophet => "prophet",
        }
    }

    /// The name the picker and the refusals show.
    pub fn name(self) -> &'static str {
        match self {
            Body::Monitor => "Monitor",
            Body::Sentinel => "Sentinel",
            Body::Prophet => "Prophet in chair",
        }
    }

    pub fn of_key(key: &str) -> Option<Body> {
        Body::ALL.into_iter().find(|b| b.key() == key)
    }

    /// Its number in the profile's choice (`forge.body`).
    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Body> {
        Body::ALL.get(usize::from(code)).copied()
    }

    pub fn spec(self) -> &'static Spec {
        &SPECS[self as usize]
    }

    /// Forge cannot open without it.
    pub fn required(self) -> bool {
        self == Body::Monitor
    }
}

/// Why a kit build left a body out: the Forge record's row
/// (`record::BODY_REFUSED`), so the game can say it by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Refused {
    /// No campaign map this machine has holds its tags.
    NoDonor,
    /// The tag schema cannot copy one of its tags.
    CannotCopy,
    /// The map's tag memory has no room for it.
    NoRoom,
}

impl Refused {
    pub const ALL: [Refused; 3] = [Refused::NoDonor, Refused::CannotCopy, Refused::NoRoom];

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Refused> {
        Refused::ALL.get(usize::from(code)).copied()
    }

    /// The reason, as the refusals end it.
    pub fn reason(self) -> &'static str {
        match self {
            Refused::NoDonor => "no campaign map on this machine holds it",
            Refused::CannotCopy => "its tags cannot be copied yet",
            Refused::NoRoom => "the map has no room for it",
        }
    }
}

/// Why a loaded map has no `body`: the Forge record's reason, or, with no
/// row, a map not built with the Forge kit.
pub fn missing(body: Body, refused: Option<Refused>) -> String {
    let why = refused.map_or(
        "the map was not built with the Forge kit; start Forge from the lobby",
        Refused::reason,
    );
    if body.required() {
        no_monitor(why)
    } else {
        not_carried(body, why)
    }
}

/// Why the kit has no `body`, for the picker and the build's report.
pub fn not_carried(body: Body, why: &str) -> String {
    format!("{} is not in this map's kit: {why}.", body.name())
}

/// The sentence of a map whose kit carries no Monitor: Forge does not open.
pub fn no_monitor(why: &str) -> String {
    format!("Forge needs the Monitor, and this map's kit has none: {why}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_in_body_order_and_keys_round_trip() {
        for (i, b) in Body::ALL.into_iter().enumerate() {
            assert_eq!(SPECS[i].body, b);
            assert_eq!(b.spec().body, b);
            assert_eq!(Body::of_key(b.key()), Some(b));
            assert_eq!(Body::from_code(b.code()), Some(b));
            assert!(!b.spec().donors.is_empty(), "{} has a donor", b.name());
        }
        assert_eq!(Body::of_key("marker"), None);
        assert_eq!(Body::from_code(3), None);
        assert!(Body::Monitor.required());
        assert!(!Body::Sentinel.required() && !Body::Prophet.required());
        assert!(Body::Prophet.spec().chair.is_some());
    }

    #[test]
    fn refusals_name_the_body() {
        assert_eq!(
            not_carried(Body::Sentinel, "no room"),
            "Sentinel is not in this map's kit: no room."
        );
        assert!(no_monitor("x").starts_with("Forge needs the Monitor"));
        assert_eq!(
            missing(Body::Monitor, Some(Refused::NoRoom)),
            "Forge needs the Monitor, and this map's kit has none: the map has no room for it."
        );
        assert!(missing(Body::Prophet, None).contains("not built with the Forge kit"));
        for r in Refused::ALL {
            assert_eq!(Refused::from_code(r.code()), Some(r));
        }
        assert_eq!(Refused::from_code(3), None);
    }
}
