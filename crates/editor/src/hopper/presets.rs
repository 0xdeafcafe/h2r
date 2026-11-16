//! Preset playlists: official Halo 2 Xbox Live lineups across the game's
//! online life (2004-2010), the web builder's (`web/src/admin/presets.ts`),
//! written from public knowledge as descriptive references only (playlist
//! names, game types, built-in variant names, map display names). No game
//! data: a preset resolves against the user's own disc (map names to ids)
//! and their own file (a published entry to copy each game variant from),
//! and whatever does not resolve is listed as unavailable.
//!
//! A preset sets the title and description, the range fields it names, and
//! its entries (a map and a weight each, the game variant copied whole from
//! a published entry of the same game type). It cannot set the hopper type,
//! the matchmaking timeouts, or a variant's rules (docs/live/live-admin.md,
//! "Presets").

use live_proto::hopper::{MAX_DESCRIPTION, MAX_RECORDS, MAX_TITLE};

use super::catalog::{Catalog, engine_of, normalise};
use super::model::{EditHopper, EditRecord, SlotRef, clip};

pub struct PresetGame {
    /// `slayer`, `ctf`, `oddball`, `king_of_the_hill`, `juggernaut`,
    /// `territories`, `assault`.
    pub game_type: &'static str,
    /// A built-in variant name to prefer as the source.
    pub variant: Option<&'static str>,
    /// Prefer a source with this team play.
    pub team: Option<bool>,
    /// Map display names; one entry per map.
    pub maps: &'static [&'static str],
    /// Relative weight (1..1000).
    pub weight: u32,
}

pub struct Preset {
    pub key: &'static str,
    pub label: &'static str,
    pub era: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// Range fields to set; `None` keeps the slot's.
    pub ranges: [Option<u32>; 3],
    pub games: &'static [PresetGame],
    /// Why parts of it are from memory rather than a record.
    pub uncertain: Option<&'static str>,
}

const LAUNCH_SMALL: &[&str] = &[
    "Lockout",
    "Midship",
    "Ivory Tower",
    "Beaver Creek",
    "Ascension",
];
const LAUNCH_BIG: &[&str] = &[
    "Coagulation",
    "Headlong",
    "Zanzibar",
    "Waterworks",
    "Burial Mounds",
];
const SMALL_AND_PACK: &[&str] = &[
    "Lockout",
    "Midship",
    "Ivory Tower",
    "Beaver Creek",
    "Ascension",
    "Warlock",
    "Sanctuary",
    "Turf",
    "Elongation",
    "Backwash",
    "Relic",
];
const BIG_AND_PACK: &[&str] = &[
    "Coagulation",
    "Headlong",
    "Zanzibar",
    "Waterworks",
    "Burial Mounds",
    "Containment",
    "Terminal",
    "Relic",
    "Gemini",
];

const fn game(
    game_type: &'static str,
    variant: &'static str,
    team: Option<bool>,
    maps: &'static [&'static str],
    weight: u32,
) -> PresetGame {
    PresetGame {
        game_type,
        variant: Some(variant),
        team,
        maps,
        weight,
    }
}

const T: Option<bool> = Some(true);
const ANY: Option<bool> = None;
const NONE3: [Option<u32>; 3] = [None, None, None];

pub const PRESETS: &[Preset] = &[
    Preset {
        key: "rumble-pit-2004",
        label: "Rumble Pit (launch)",
        era: "2004 launch",
        title: "Rumble Pit",
        description: "Free for all, up to 8 players. Slayer with some objective games.",
        ranges: [Some(4), Some(8), None],
        games: &[
            game("slayer", "Slayer", ANY, LAUNCH_SMALL, 3),
            game(
                "oddball",
                "Oddball",
                ANY,
                &["Lockout", "Midship", "Ivory Tower"],
                1,
            ),
            game(
                "king_of_the_hill",
                "Crazy King",
                ANY,
                &["Midship", "Ivory Tower", "Ascension"],
                1,
            ),
            game(
                "juggernaut",
                "Ninjanaut",
                ANY,
                &["Lockout", "Beaver Creek"],
                1,
            ),
        ],
        uncertain: Some("map rotation and weights are approximate"),
    },
    Preset {
        key: "team-skirmish-2004",
        label: "Team Skirmish (launch)",
        era: "2004 launch",
        title: "Team Skirmish",
        description: "Two teams of up to 4. Mostly CTF and other objective games.",
        ranges: NONE3,
        games: &[
            game(
                "ctf",
                "CTF Classic",
                T,
                &["Midship", "Beaver Creek", "Zanzibar", "Ivory Tower"],
                1,
            ),
            game("assault", "Single Bomb", T, &["Midship", "Zanzibar"], 1),
            game("slayer", "Team Slayer", T, LAUNCH_SMALL, 1),
            game(
                "king_of_the_hill",
                "Team King",
                T,
                &["Midship", "Ascension"],
                1,
            ),
            game(
                "territories",
                "3 Plots",
                T,
                &["Ascension", "Beaver Creek"],
                1,
            ),
        ],
        uncertain: Some("map rotation is approximate"),
    },
    Preset {
        key: "head-to-head-2004",
        label: "Head to Head (launch)",
        era: "2004 launch",
        title: "Head to Head",
        description: "One on one Slayer.",
        ranges: NONE3,
        games: &[game("slayer", "Slayer", ANY, LAUNCH_SMALL, 1)],
        uncertain: None,
    },
    Preset {
        key: "big-team-battle-2004",
        label: "Big Team Battle (launch)",
        era: "2004 launch",
        title: "Big Team Battle",
        description: "Two teams of up to 8. Mostly objective games, with vehicles.",
        ranges: NONE3,
        games: &[
            game("ctf", "Multi Flag CTF", T, LAUNCH_BIG, 2),
            game(
                "assault",
                "Multi Bomb",
                T,
                &["Coagulation", "Headlong", "Waterworks"],
                1,
            ),
            game("slayer", "Team Slayer", T, LAUNCH_BIG, 1),
            game(
                "territories",
                "Land Grab",
                T,
                &["Coagulation", "Burial Mounds"],
                1,
            ),
        ],
        uncertain: Some("map rotation and weights are approximate"),
    },
    Preset {
        key: "minor-clanmatch-2004",
        label: "Minor Clanmatch (launch)",
        era: "2004 launch",
        title: "Minor Clanmatch",
        description: "Clan parties of 3-4. Plays the Team Skirmish games.",
        ranges: [Some(2), Some(3), Some(4)],
        games: &[
            game(
                "ctf",
                "CTF Classic",
                T,
                &["Midship", "Beaver Creek", "Zanzibar"],
                1,
            ),
            game("slayer", "Team Slayer", T, LAUNCH_SMALL, 1),
        ],
        uncertain: Some(
            "ranges follow the compiled-in default for this playlist; their meaning is unconfirmed",
        ),
    },
    Preset {
        key: "major-clanmatch-2004",
        label: "Major Clanmatch (launch)",
        era: "2004 launch",
        title: "Major Clanmatch",
        description: "Clan parties of 6-8. Plays the Big Team Battle games.",
        ranges: [None, Some(6), Some(8)],
        games: &[
            game("ctf", "Multi Flag CTF", T, LAUNCH_BIG, 1),
            game("slayer", "Team Slayer", T, LAUNCH_BIG, 1),
        ],
        uncertain: Some(
            "ranges follow the compiled-in default for this playlist; their meaning is unconfirmed",
        ),
    },
    Preset {
        key: "training-ground-2004",
        label: "Training Ground (launch)",
        era: "2004 launch",
        title: "Training Ground",
        description: "Unranked team games; guests allowed.",
        ranges: NONE3,
        games: &[
            game("slayer", "Team Slayer", T, LAUNCH_SMALL, 1),
            game("ctf", "CTF Classic", T, &["Midship", "Beaver Creek"], 1),
        ],
        uncertain: None,
    },
    Preset {
        key: "team-slayer",
        label: "Team Slayer",
        era: "2005 onward",
        title: "Team Slayer",
        description: "Two teams of 4. Team Slayer only.",
        ranges: NONE3,
        games: &[game("slayer", "Team Slayer", T, SMALL_AND_PACK, 1)],
        uncertain: Some("added in a 2005 playlist update; exact map list varied"),
    },
    Preset {
        key: "double-team",
        label: "Double Team",
        era: "2005 onward",
        title: "Double Team",
        description: "Two teams of 2. Team Slayer, some objective games.",
        ranges: NONE3,
        games: &[
            game(
                "slayer",
                "Team Slayer",
                T,
                &[
                    "Lockout",
                    "Midship",
                    "Beaver Creek",
                    "Ivory Tower",
                    "Warlock",
                    "Sanctuary",
                ],
                3,
            ),
            game("ctf", "1 Flag CTF", T, &["Midship", "Beaver Creek"], 1),
        ],
        uncertain: Some("added in 2005; objective share is approximate"),
    },
    Preset {
        key: "team-snipers",
        label: "Team Snipers",
        era: "2005 onward",
        title: "Team Snipers",
        description: "Sniper rifles only, teams of 4.",
        ranges: NONE3,
        games: &[game(
            "slayer",
            "Snipers",
            T,
            &[
                "Zanzibar",
                "Headlong",
                "Burial Mounds",
                "Ascension",
                "Coagulation",
                "Lockout",
            ],
            1,
        )],
        uncertain: Some(
            "the source variant's settings decide the weapons; a team Snipers entry must already be in the file",
        ),
    },
    Preset {
        key: "team-hardcore",
        label: "Team Hardcore",
        era: "2006 onward",
        title: "Team Hardcore",
        description: "Competitive 4 on 4: Slayer and CTF on the tightest maps.",
        ranges: NONE3,
        games: &[
            game(
                "slayer",
                "Team Slayer",
                T,
                &[
                    "Lockout",
                    "Midship",
                    "Ivory Tower",
                    "Beaver Creek",
                    "Sanctuary",
                    "Warlock",
                ],
                2,
            ),
            game(
                "ctf",
                "CTF Classic",
                T,
                &["Midship", "Beaver Creek", "Sanctuary"],
                1,
            ),
            game(
                "king_of_the_hill",
                "Team King",
                T,
                &["Midship", "Lockout"],
                1,
            ),
        ],
        uncertain: Some(
            "hardcore rules lived in custom variants; the builder copies whichever source variant matches",
        ),
    },
    Preset {
        key: "big-team-2006",
        label: "Big Team Battle (with the map packs)",
        era: "2005-2007",
        title: "Big Team Battle",
        description: "Two teams of up to 8, map pack maps included.",
        ranges: NONE3,
        games: &[
            game("ctf", "Multi Flag CTF", T, BIG_AND_PACK, 2),
            game(
                "assault",
                "Multi Bomb",
                T,
                &["Coagulation", "Containment", "Terminal"],
                1,
            ),
            game("slayer", "Team Slayer", T, BIG_AND_PACK, 1),
        ],
        uncertain: Some("map pack maps resolve only when the disc lists them"),
    },
    Preset {
        key: "shotty-snipers",
        label: "Shotty Snipers",
        era: "2006-2007",
        title: "Shotty Snipers",
        description: "Shotguns and sniper rifles, teams of 4.",
        ranges: NONE3,
        games: &[game(
            "slayer",
            "Snipers",
            T,
            &["Lockout", "Zanzibar", "Ascension", "Midship", "Sanctuary"],
            1,
        )],
        uncertain: Some(
            "dates and maps from memory; the shotgun loadout needs a source variant that has it",
        ),
    },
    Preset {
        key: "rumble-pit-later",
        label: "Rumble Pit (later)",
        era: "2006-2010",
        title: "Rumble Pit",
        description: "Free for all, up to 8 players, map pack maps included.",
        ranges: [Some(4), Some(8), None],
        games: &[
            game(
                "slayer",
                "Slayer",
                ANY,
                &[
                    "Lockout",
                    "Midship",
                    "Ivory Tower",
                    "Beaver Creek",
                    "Ascension",
                    "Warlock",
                    "Sanctuary",
                    "Elongation",
                    "Gemini",
                ],
                3,
            ),
            game("oddball", "Oddball", ANY, &["Lockout", "Warlock"], 1),
            game(
                "king_of_the_hill",
                "Crazy King",
                ANY,
                &["Midship", "Sanctuary"],
                1,
            ),
        ],
        uncertain: Some("map rotation and weights are approximate"),
    },
];

/// One preset entry as it resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGame {
    pub label: String,
    pub record: Option<EditRecord>,
    /// Why it is unavailable.
    pub missing: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub title: String,
    pub description: String,
    pub ranges: [Option<u32>; 3],
    pub games: Vec<ResolvedGame>,
    /// The entries that will be written (at most 100).
    pub records: Vec<EditRecord>,
}

/// The published entry to copy for a game: same engine; then a matching
/// variant name (+2); then matching team play (+1); the first best wins.
fn source<'a>(published: &'a [EditHopper], g: &PresetGame) -> Option<(SlotRef, &'a EditRecord)> {
    let engine = engine_of(g.game_type)?;
    let score = |r: &EditRecord| {
        let v = r.variant.as_ref();
        let name = g
            .variant
            .is_some_and(|want| v.is_some_and(|v| normalise(&v.name) == normalise(want)));
        let team = g.team.is_some_and(|t| v.is_some_and(|v| v.team == t));
        2 * u32::from(name) + u32::from(team)
    };
    let mut best: Option<(SlotRef, &EditRecord, u32)> = None;
    for h in published {
        for (slot, r) in h.records.iter().enumerate() {
            if r.variant.as_ref().is_none_or(|v| v.engine != engine) {
                continue;
            }
            let s = score(r);
            if best.as_ref().is_none_or(|(.., b)| s > *b) {
                best = Some((
                    SlotRef {
                        hopper: h.index,
                        slot,
                    },
                    r,
                    s,
                ));
            }
        }
    }
    best.map(|(at, r, _)| (at, r))
}

/// Resolves a preset against the catalog and the published file (the
/// hoppers as loaded, not as edited: copies name loaded slots).
pub fn resolve(preset: &Preset, catalog: Option<&Catalog>, published: &[EditHopper]) -> Resolution {
    let mut games = Vec::new();
    for g in preset.games {
        let src = source(published, g);
        let type_name = g.variant.unwrap_or(g.game_type);
        for name in g.maps {
            let label = format!("{type_name} on {name}");
            let missing = |why: String| ResolvedGame {
                label: label.clone(),
                record: None,
                missing: Some(why),
            };
            let Some(catalog) = catalog else {
                games.push(missing("no map list (open the disc)".into()));
                continue;
            };
            let Some(map) = catalog.find_map(name) else {
                games.push(missing(format!("{name} is not in this game's maps")));
                continue;
            };
            let Some((at, r)) = src else {
                games.push(missing(format!(
                    "no published entry plays {} to copy",
                    g.game_type.replace('_', " ")
                )));
                continue;
            };
            games.push(ResolvedGame {
                label: label.clone(),
                record: Some(EditRecord {
                    id: map.id,
                    string: r.string.clone(),
                    flags: r.flags,
                    dwords: [g.weight.clamp(1, 1000), r.dwords[1]],
                    variant: r.variant.clone(),
                    src: Some(at),
                    picked: r.picked.clone(),
                }),
                missing: None,
            });
        }
    }
    let records = games
        .iter()
        .filter_map(|g| g.record.clone())
        .take(MAX_RECORDS)
        .collect();
    Resolution {
        title: preset.title.into(),
        description: preset.description.into(),
        ranges: preset.ranges,
        games,
        records,
    }
}

/// The hopper with a resolution applied. Keeps its entries when nothing
/// resolved (a hopper needs at least one), and its type, status and the
/// ranges the preset leaves alone. Text longer than the slots is cut.
pub fn apply(h: &EditHopper, r: &Resolution) -> EditHopper {
    let mut out = h.clone();
    for (slot, v) in out.ranges.iter_mut().zip(r.ranges) {
        if let Some(v) = v {
            *slot = v;
        }
    }
    out.title = clip(&r.title, MAX_TITLE);
    out.description = clip(&r.description, MAX_DESCRIPTION);
    if !r.records.is_empty() {
        out.records = r.records.clone();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::catalog::CatalogMap;
    use super::super::model::{self, tests::put_variant};
    use super::*;

    fn catalog() -> Catalog {
        let maps = [
            "Lockout",
            "Midship",
            "Ivory Tower",
            "Beaver Creek",
            "Ascension",
        ]
        .iter()
        .enumerate()
        .map(|(i, n)| CatalogMap {
            id: 100 + i as u32,
            name: n.to_string(),
            description: String::new(),
            key: None,
        })
        .collect();
        Catalog { maps }
    }

    #[test]
    fn presets_are_well_formed() {
        let mut keys = std::collections::HashSet::new();
        for p in PRESETS {
            assert!(keys.insert(p.key), "duplicate {}", p.key);
            assert!(p.title.len() <= MAX_TITLE, "{}", p.key);
            assert!(p.description.len() <= MAX_DESCRIPTION, "{}", p.key);
            for g in p.games {
                assert!(engine_of(g.game_type).is_some(), "{}", g.game_type);
                assert!((1..=1000).contains(&g.weight));
            }
        }
        assert_eq!(PRESETS.len(), 14);
    }

    #[test]
    fn a_preset_resolves_against_maps_and_published_variants() {
        let mut f = model::tests::sample();
        // Published: hopper 0 slot 0 a team Slayer named "Team Slayer",
        // slot 1 a free-for-all Slayer; hopper 1 slot 1 a CTF entry.
        put_variant(&mut f, 0, 0, "Team Slayer", 2, true);
        put_variant(&mut f, 0, 1, "Slayer", 2, false);
        put_variant(&mut f, 1, 0, "Juggernaut", 7, false);
        put_variant(&mut f, 1, 1, "CTF Classic", 1, true);
        put_variant(&mut f, 1, 2, "Odd", 3, false);
        put_variant(&mut f, 2, 0, "Slayer", 2, false);
        put_variant(&mut f, 2, 1, "Ninjanaut", 7, false);
        put_variant(&mut f, 2, 2, "Crazy King", 4, false);
        put_variant(&mut f, 2, 3, "Oddball", 3, false);
        let published = model::load(&f);
        let p = PRESETS.iter().find(|p| p.key == "rumble-pit-2004").unwrap();
        let r = resolve(p, Some(&catalog()), &published);
        assert_eq!(r.title, "Rumble Pit");
        // 5 Slayer + 3 Oddball + 3 Crazy King + 2 Ninjanaut, all on maps
        // the catalog has.
        assert_eq!(r.games.len(), 13);
        assert_eq!(r.records.len(), 13);
        let first = &r.records[0];
        assert_eq!(first.id, 100);
        assert_eq!(first.weight(), 3);
        // "Slayer" by name beats the earlier team Slayer.
        assert_eq!(first.src, Some(SlotRef { hopper: 0, slot: 1 }));
        // Oddball: "Oddball" by name (hopper 2) over "Odd" (hopper 1).
        assert_eq!(r.records[5].src, Some(SlotRef { hopper: 2, slot: 3 }));
        assert_eq!(r.records[5].weight(), 1);

        // Applied and written, each entry carries its source slot whole
        // (save for the map id and weight).
        let mut m = published.clone();
        m[3] = apply(&m[3], &r);
        assert_eq!(m[3].ranges, [4, 8, 4]);
        assert_eq!(m[3].title, "Rumble Pit");
        let out = model::apply(&f, &m).unwrap();
        let back = model::load(&out);
        assert_eq!(back[3].records.len(), 13);
        assert_eq!(back[3].records[5].variant.as_ref().unwrap().name, "Oddball");
        let want = f.slot(2, 3).unwrap();
        let got = out.slot(3, 5).unwrap();
        assert_eq!(&got[8..0x248], &want[8..0x248]);
        assert_eq!(&got[0x24c..], &want[0x24c..]);
    }

    #[test]
    fn what_does_not_resolve_is_listed_and_written_nowhere() {
        let f = model::tests::sample();
        let published = model::load(&f);
        let p = PRESETS
            .iter()
            .find(|p| p.key == "big-team-battle-2004")
            .unwrap();
        let none = resolve(p, None, &published);
        assert!(none.records.is_empty());
        assert!(
            none.games
                .iter()
                .all(|g| g.missing.as_deref() == Some("no map list (open the disc)"))
        );
        let r = resolve(p, Some(&catalog()), &published);
        assert!(r.records.is_empty(), "none of its maps are in the catalog");
        assert!(
            r.games[0]
                .missing
                .as_ref()
                .unwrap()
                .contains("Coagulation is not in")
        );
        // Nothing resolved: the hopper keeps its entries.
        let h = apply(&published[0], &r);
        assert_eq!(h.records, published[0].records);
        assert_eq!(h.title, "Big Team Battle");

        // A game type nothing published plays.
        let p = PRESETS
            .iter()
            .find(|p| p.key == "team-skirmish-2004")
            .unwrap();
        let r = resolve(p, Some(&catalog()), &published);
        let assault = r
            .games
            .iter()
            .find(|g| g.label == "Single Bomb on Midship")
            .unwrap();
        assert_eq!(
            assault.missing.as_deref(),
            Some("no published entry plays assault to copy")
        );
    }
}
