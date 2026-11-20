//! Choices of ours: values the game's record keeps and plays but its editor
//! never offers (docs/game/game-options.md, "The options"). Each is inside
//! the clean-up's limits (`record::check`), so it saves, loads and reaches
//! every player in a networked game like a retail value; an older build
//! plays it too. Studio shows them beside the disc's choices, and the game's
//! own editor gets them in its option lists (`boot::variant_options`).
//!
//! The labels are ours. Values the disc already offers are never added
//! twice: [`merge`] skips them.

use crate::record::SettingId;

/// One added choice: a setting, its value and our label for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extra {
    pub setting: u32,
    pub value: i32,
    pub label: &'static str,
}

const fn x(setting: u32, value: i32, label: &'static str) -> Extra {
    Extra {
        setting,
        value,
        label,
    }
}

/// Every added choice. The index of an entry is its label's string id in
/// the game (`boot::variant_options`), so only append.
pub const OPTIONS: &[Extra] = &[
    // Starting weapon "None": `0x19eda0` maps 1 to no weapon, as it does
    // for the secondary weapon, which the editor already offers it for.
    x(89, 1, "None"),
    // Respawn time: retail starts at 3 s. The death code never waits
    // less than one second (docs/game/game-options.md), so 1 s is as fast
    // as it goes; 0 is not offered (with cycling it divides by zero).
    x(17, 1, "1 Second"),
    x(17, 2, "2 Seconds"),
    x(17, 45, "45 Seconds"),
    x(17, 60, "1 Minute"),
    x(17, 120, "2 Minutes"),
    // Score to win, points: Slayer, CTF, Juggernaut, Assault.
    x(2, 500, "500"),
    x(2, 1000, "1000"),
    x(1, 25, "25"),
    x(1, 50, "50"),
    x(1, 100, "100"),
    x(7, 50, "50"),
    x(7, 100, "100"),
    x(7, 250, "250"),
    x(9, 25, "25"),
    x(9, 50, "50"),
    x(9, 100, "100"),
    // Score to win, seconds: Oddball, King, Territories.
    x(3, 900, "15 Minutes"),
    x(3, 1800, "30 Minutes"),
    x(4, 900, "15 Minutes"),
    x(4, 1800, "30 Minutes"),
    x(8, 900, "15 Minutes"),
    x(8, 1800, "30 Minutes"),
    // Time limit, seconds; the record stops at 0x7fff (9 h 6 min).
    x(10, 5400, "90 Minutes"),
    x(10, 7200, "2 Hours"),
    x(10, 10800, "3 Hours"),
    x(10, 14400, "4 Hours"),
    x(10, 21600, "6 Hours"),
    x(10, 32400, "9 Hours"),
    // Lives per round.
    x(16, 4, "4"),
    x(16, 5, "5"),
    x(16, 10, "10"),
    x(16, 25, "25"),
    x(16, 50, "50"),
    // Suicide and betrayal penalties, seconds.
    x(18, 20, "20 Seconds"),
    x(18, 30, "30 Seconds"),
    x(18, 60, "1 Minute"),
    x(26, 300, "5 Minutes"),
    x(26, 600, "10 Minutes"),
];

/// The added choices of one setting, in table order.
pub fn for_setting(id: SettingId) -> impl Iterator<Item = (usize, &'static Extra)> {
    OPTIONS
        .iter()
        .enumerate()
        .filter(move |(_, e)| e.setting == id.0)
}

/// Where each added choice of `id` goes among `retail`'s values: before the
/// first value larger than it, else at the end, so ascending lists stay
/// ascending and "None" lands after "Map Default". A value `retail` has is
/// left out. Returns the merged list as `Ok(retail index)` for a retail
/// choice and `Err(OPTIONS index)` for ours.
pub fn merge(id: SettingId, retail: &[i32]) -> Vec<Result<usize, usize>> {
    let mut out: Vec<(i32, Result<usize, usize>)> = retail
        .iter()
        .enumerate()
        .map(|(i, v)| (*v, Ok(i)))
        .collect();
    for (k, e) in for_setting(id) {
        if out.iter().any(|(v, _)| *v == e.value) {
            continue;
        }
        let at = out
            .iter()
            .position(|(v, _)| *v > e.value)
            .unwrap_or(out.len());
        out.insert(at, (e.value, Err(k)));
    }
    out.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{self, GameType, GameVariant};

    #[test]
    fn every_added_choice_is_one_the_game_keeps() {
        let mut seen = std::collections::HashSet::new();
        for e in OPTIONS {
            assert!(seen.insert((e.setting, e.value)), "{e:?} twice");
            assert!(!e.label.is_empty() && e.label.len() < 32);
            let ty = GameType::ALL
                .into_iter()
                .find(|t| crate::schema::applies(SettingId(e.setting), *t))
                .unwrap();
            let mut v = GameVariant::blank(ty);
            if matches!(ty, GameType::Ctf | GameType::Assault) {
                v.set(SettingId(91), 1).unwrap();
            }
            if ty == GameType::Territories {
                for s in [74, 98, 99] {
                    v.set(SettingId(s), 1).unwrap();
                }
            }
            v.set(SettingId(e.setting), e.value).unwrap();
            assert!(
                record::refused(&v).is_empty(),
                "{e:?}: {:?}",
                record::refused(&v)
            );
        }
        assert!(
            !OPTIONS.iter().any(|e| e.setting == 17 && e.value == 0),
            "respawn 0 is never offered"
        );
    }

    #[test]
    fn the_schema_shows_ours_beside_the_discs() {
        use crate::schema::{Choice, Issue, Schema, with_extras};
        let c = |value, label: &str| Choice {
            value,
            label: label.into(),
            default: value == 0,
            ours: false,
        };
        let merged = with_extras(SettingId(89), vec![c(0, "Map Default"), c(2, "Random")]);
        let got: Vec<(i32, &str, bool)> = merged
            .iter()
            .map(|c| (c.value, c.label.as_str(), c.ours))
            .collect();
        assert_eq!(
            got,
            [
                (0, "Map Default", false),
                (1, "None", true),
                (2, "Random", false)
            ]
        );
        // Nothing offered (the fallback): nothing added.
        assert!(with_extras(SettingId(89), Vec::new()).is_empty());

        // Our rule shows as an issue.
        let mut v = GameVariant::blank(GameType::Slayer);
        v.set(record::RESPAWN, 0).unwrap();
        v.set(record::RESPAWN_MODIFIER, record::CYCLING).unwrap();
        let issues = Schema::fallback().validate(&v);
        assert!(matches!(issues.as_slice(), [Issue::Ours(_)]), "{issues:?}");
    }

    #[test]
    fn added_choices_go_in_value_order_and_never_twice() {
        // A weapon list: map default, random, weapons in the game's order.
        let weapons = [0, 2, 3, 4, 18, 8];
        let m = merge(SettingId(89), &weapons);
        assert_eq!(m.len(), 7);
        assert_eq!(m[0], Ok(0));
        assert_eq!(m[1], Err(0), "None right after Map Default");
        assert_eq!(&m[2..], &[Ok(1), Ok(2), Ok(3), Ok(4), Ok(5)]);

        // Lives: ours slot in between, retail order kept.
        let lives = [0, 1, 2, 3, 7, 9, 99];
        let values: Vec<i32> = merge(SettingId(16), &lives)
            .into_iter()
            .map(|r| match r {
                Ok(i) => lives[i],
                Err(k) => OPTIONS[k].value,
            })
            .collect();
        assert_eq!(values, [0, 1, 2, 3, 4, 5, 7, 9, 10, 25, 50, 99]);

        // A value the disc already offers is not added again.
        let m = merge(SettingId(17), &[1, 3, 5]);
        assert_eq!(m.iter().filter(|r| r.is_err()).count(), 4);
        // A setting with nothing of ours is unchanged.
        assert_eq!(merge(SettingId(19), &[0, 1, 2]), vec![Ok(0), Ok(1), Ok(2)]);
    }
}
