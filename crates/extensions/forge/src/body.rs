//! The player's Forge body: which of the kit's bodies D-pad Up swaps them
//! into (docs/game/forge-design-slices.md, "The monitor body"). Kept per
//! profile as `[profiles."<name>".forge] body`, a choice of
//! `forge_doc::bodies::Body`; the picker offers every body and greys the
//! ones this map's kit left out, with the reason.
use std::collections::BTreeMap;

use forge_doc::bodies::{self, Body, Refused};
use settings::{Apply, Choice, Def, Kind, Scope, Store, Tier, Value};

pub const KEY: &str = "forge.body";

const CHOICES: &[Choice] = &[
    Choice {
        value: 0,
        name: "monitor",
        label: "Monitor",
    },
    Choice {
        value: 1,
        name: "sentinel",
        label: "Sentinel",
    },
    Choice {
        value: 2,
        name: "prophet",
        label: "Prophet in chair",
    },
];

/// The profile's Forge body. The unit is made in the game for every
/// machine, so it is the simulation's.
pub static DEF: Def = Def {
    key: KEY,
    label: "Forge body",
    help: "The body D-pad Up swaps you into in a Forge session: the Monitor, a Sentinel or a \
           Prophet in his chair. Kept per profile under [profiles.\"<name>\".forge].",
    kind: Kind::Choice(CHOICES),
    default: Value::Int(0),
    env: None,
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Sim,
    tier: Tier::User,
    menu: None,
};

/// `profile`'s body, and a line when the saved value is not one (it is
/// left in the file, and the Monitor is used).
pub fn load(store: &Store, profile: &str) -> (Body, Option<String>) {
    match store.profile_get(profile, KEY) {
        None => (Body::Monitor, None),
        Some(v) => match v.as_int().and_then(|i| u8::try_from(i).ok()).and_then(Body::from_code) {
            Some(body) => (body, None),
            None => (
                Body::Monitor,
                Some(format!("{KEY} is not a Forge body; the Monitor is used")),
            ),
        },
    }
}

/// Saves `body` under `profile`. `Ok(Some(why))` when it holds for this run
/// but the file was not written.
pub fn save(store: &Store, profile: &str, body: Body) -> Result<Option<String>, String> {
    store
        .profile_set(profile, KEY, Value::Int(i64::from(body.code())))
        .map_err(|e| e.to_string())
}

/// What the kit of the loaded map gives each body: its unit tag's datum, or
/// why it has none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kit {
    /// The bodies whose unit tag the loaded map holds, with its datum.
    pub carried: BTreeMap<Body, u32>,
    /// The Forge record's reasons for the bodies the build left out.
    pub refused: BTreeMap<Body, Refused>,
}

impl Kit {
    /// The unit tag of `body`, or the sentence that refuses it by name.
    pub fn unit(&self, body: Body) -> Result<u32, String> {
        self.carried
            .get(&body)
            .copied()
            .ok_or_else(|| bodies::missing(body, self.refused.get(&body).copied()))
    }
}

/// One row of the body picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub body: Body,
    pub label: &'static str,
    /// Why it cannot be chosen on this map; the picker shows it greyed with
    /// the sentence.
    pub refused: Option<String>,
}

/// The picker's rows, every body in order: a body the kit lacks stays,
/// greyed with its reason, so nothing is swapped for another.
pub fn rows(kit: &Kit) -> Vec<Row> {
    Body::ALL
        .into_iter()
        .map(|body| Row {
            body,
            label: body.name(),
            refused: kit.unit(body).err(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choices_are_the_bodies_in_order() {
        assert_eq!(CHOICES.len(), Body::ALL.len());
        for (c, b) in CHOICES.iter().zip(Body::ALL) {
            assert_eq!(c.value, i64::from(b.code()));
            assert_eq!(c.name, b.key());
            assert_eq!(c.label, b.name());
        }
    }

    #[test]
    fn a_body_the_kit_lacks_is_greyed_with_its_reason() {
        let kit = Kit {
            carried: BTreeMap::from([(Body::Monitor, 0xe100_0001)]),
            refused: BTreeMap::from([(Body::Sentinel, Refused::CannotCopy)]),
        };
        assert_eq!(kit.unit(Body::Monitor), Ok(0xe100_0001));
        let rows = rows(&kit);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].refused, None);
        assert_eq!(
            rows[1].refused.as_deref(),
            Some("Sentinel is not in this map's kit: its tags cannot be copied yet.")
        );
        assert!(
            rows[2]
                .refused
                .as_deref()
                .is_some_and(|s| s.contains("not built with the Forge kit"))
        );
        // A kit with no Monitor refuses Forge by name.
        assert!(Kit::default().unit(Body::Monitor).unwrap_err().starts_with("Forge needs the Monitor"));
    }
}
