//! The playlists a server offers with no hopper file published: a classic
//! Halo 2 lineup built from nothing but the format ([`super::layout`]) and
//! numbers set here. No bytes come from a disc or a real hopper file.
//!
//! What the game itself does is the model. Its compiled-in defaults
//! (`0x001940d0`, used when no file was ever downloaded) build seven
//! hoppers the same way: a descriptor per hopper (`0x00193c70`), the
//! start-wait table from a base wait and the player range
//! ([`super::layout::start_waits`]), the skill ceiling
//! ([`super::layout::default_skill_ceiling`]), status 0, and entries whose
//! variants are the stock record of a game type plus a few changes
//! (`0x00212f40`), with settings 12..14 cleared (`0x00193010`). Here each
//! variant is the record of a script ([`variants`]): the same stock record
//! and changes, written as the script's settings, so every default entry is
//! mode 2 (docs/live/live-matchmaking.md, "6.8 The default set"). The
//! timings here are the game's (30 s base wait and 150 s search timeout
//! for small games, 45 s and 300 s for big team games); the playlists,
//! their sizes, maps and weights are this project's choice.
//!
//! After the seven human playlists come nine clanker playlists
//! (docs/game/bots.md §8, §9.4 item 1): Clankermatch hoppers, where every
//! other player is a bot the host runs. Their descriptors are ordinary ones;
//! what makes them clanker hoppers is [`Playlist::clankers`], which the
//! server keeps beside the file and serves as
//! [`super::clankers::ClankerHoppers`]. The mark also says which clanker
//! playlists rank their games.
//!
//! The default rotation uses maps from the base disc and Multiplayer Map
//! Pack. Live filters the published rotation for consoles that cannot load
//! the pack (docs/live/live-matchmaking.md, "Map Pack rotations").

use super::clankers::Clankers;
use super::layout::{Descriptor, Entry, PLAYLIST_IDS, Rules, default_skill_ceiling, start_waits};
use super::variant::VariantRecord;
use super::{
    BLOCK, BLOCKS_AT, DESCRIPTOR, HOPPERS, HopperError, HopperFile, MAX_DESCRIPTION, MAX_TITLE,
};

/// The base disc's multiplayer levels by map id: the ids the game's level
/// list (the globals tag, `matg +0x178`, element 0, block `+0x10`, stride
/// `0xc64`) gives each map, which is what an entry's `+0x04` must match.
pub const RETAIL_MAPS: [(u32, &str); 12] = [
    (10, "Ivory Tower"),
    (20, "Midship"),
    (30, "Zanzibar"),
    (40, "Waterworks"),
    (50, "Lockout"),
    (60, "Burial Mounds"),
    (70, "Colossus"),
    (80, "Ascension"),
    (100, "Beaver Creek"),
    (110, "Coagulation"),
    (120, "Foundation"),
    (800, "Headlong"),
];

/// The Multiplayer Map Pack's levels by map id, in the same level list.
pub const MAP_PACK_MAPS: [(u32, &str); 9] = [
    (1000, "Containment"),
    (1010, "Warlock"),
    (1020, "Sanctuary"),
    (1030, "Turf"),
    (1040, "Backwash"),
    (1050, "Elongation"),
    (1060, "Relic"),
    (1070, "Terminal"),
    (1080, "Gemini"),
];

pub const IVORY_TOWER: u32 = 10;
pub const MIDSHIP: u32 = 20;
pub const ZANZIBAR: u32 = 30;
pub const WATERWORKS: u32 = 40;
pub const LOCKOUT: u32 = 50;
pub const BURIAL_MOUNDS: u32 = 60;
pub const COLOSSUS: u32 = 70;
pub const ASCENSION: u32 = 80;
pub const BEAVER_CREEK: u32 = 100;
pub const COAGULATION: u32 = 110;
pub const FOUNDATION: u32 = 120;
pub const HEADLONG: u32 = 800;
pub const CONTAINMENT: u32 = 1000;
pub const WARLOCK: u32 = 1010;
pub const SANCTUARY: u32 = 1020;
pub const TURF: u32 = 1030;
pub const BACKWASH: u32 = 1040;
pub const ELONGATION: u32 = 1050;
pub const RELIC: u32 = 1060;
pub const TERMINAL: u32 = 1070;
pub const GEMINI: u32 = 1080;

/// The base-disc map ids, for a rotation without the Map Pack.
pub fn base_map_ids() -> Vec<u32> {
    RETAIL_MAPS.iter().map(|(id, _)| *id).collect()
}

/// Every retail map id, base disc and Map Pack, for [`HopperFile::problems`].
pub fn retail_map_ids() -> Vec<u32> {
    RETAIL_MAPS
        .iter()
        .chain(&MAP_PACK_MAPS)
        .map(|(id, _)| *id)
        .collect()
}

/// The game's base wait and search timeout for small games, ms.
const SMALL_WAIT: u32 = 30_000;
const SMALL_TIMEOUT: u32 = 150_000;
/// And for big team games.
const BIG_WAIT: u32 = 45_000;
const BIG_TIMEOUT: u32 = 300_000;

/// The matchmaking variants the default playlists use. Each is the record of
/// its engine's built-in script (`rules::builtin::MATCHMAKING`,
/// `variants::portable::record_for`): the stock record of the type, the
/// script's settings (the hopper defaults writer's, `0x00193010`, and the
/// variant's own) and the script reference, so every one is mode 2 and a
/// hopper plays scripted variants only. Built once.
pub mod variants {
    use std::sync::OnceLock;

    use super::VariantRecord;

    /// The built-in matchmaking scripts' records, in
    /// `rules::builtin::MATCHMAKING` order, with their keys.
    pub(super) fn all() -> &'static [(&'static str, VariantRecord)] {
        static ALL: OnceLock<Vec<(&'static str, VariantRecord)>> = OnceLock::new();
        ALL.get_or_init(|| {
            ::rules::builtin::MATCHMAKING
                .iter()
                .map(|m| (m.key, of_script(m.key, m.source)))
                .collect()
        })
    }

    /// The record of a script's own variant (`record_for`). The built-in
    /// scripts compile and make a record the game keeps (a test holds
    /// them to it), so a failure here is a build with a broken script.
    fn of_script(key: &str, source: &str) -> VariantRecord {
        let ir = ::rules::compile(source)
            .unwrap_or_else(|e| panic!("the built-in matchmaking script {key} does not compile: {e}"));
        let record = ::variants::portable::record_for(&ir, &[])
            .unwrap_or_else(|e| panic!("the built-in matchmaking script {key} makes no record: {e}"));
        VariantRecord::from_bytes(&record.to_bytes()).expect("a record is 0x130 bytes")
    }

    fn by_key(key: &str) -> VariantRecord {
        all()
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("no built-in matchmaking variant {key}"))
    }

    /// Free for all Slayer, a six-minute limit.
    pub fn rumble_slayer() -> VariantRecord {
        by_key("rumble_slayer")
    }

    pub fn team_slayer() -> VariantRecord {
        by_key("team_slayer")
    }

    /// Team Slayer with sniper rifles: no vehicles or turrets, no motion
    /// sensor, a sniper loadout (the item numbers the game's own Snipers
    /// variant uses).
    pub fn team_snipers() -> VariantRecord {
        by_key("team_snipers")
    }

    /// Team Slayer with rocket launchers and rockets on the map.
    pub fn team_heavies() -> VariantRecord {
        by_key("team_heavies")
    }

    pub fn oddball() -> VariantRecord {
        by_key("oddball")
    }

    pub fn king_of_the_hill() -> VariantRecord {
        by_key("king_of_the_hill")
    }

    /// CTF with the type's two extra flags on, as the game's Classic CTF.
    pub fn classic_ctf() -> VariantRecord {
        by_key("classic_ctf")
    }

    /// Two rounds, one score each, three minutes a round.
    pub fn assault() -> VariantRecord {
        by_key("assault")
    }

    pub fn territories() -> VariantRecord {
        by_key("territories")
    }

    pub fn team_king() -> VariantRecord {
        by_key("team_king")
    }

    pub fn teamball() -> VariantRecord {
        by_key("teamball")
    }
}

/// The built-in matchmaking variants by key: what an entry can be given
/// without copying another entry's 0x130 bytes (the playlist builders'
/// variant picker, `variant` in the admin PUT body). Each is the record of
/// a script in `rules::builtin::MATCHMAKING` ([`variants`]), so each is
/// mode 2; none comes from a disc.
pub fn builtin_variants() -> Vec<(&'static str, VariantRecord)> {
    variants::all().to_vec()
}

/// The built-in variant named `key` ([`builtin_variants`]).
pub fn builtin_variant(key: &str) -> Option<VariantRecord> {
    builtin_variants()
        .into_iter()
        .find_map(|(k, v)| (k == key).then_some(v))
}

/// The key of a built-in variant with exactly these bytes, if any.
pub fn builtin_key(v: &VariantRecord) -> Option<&'static str> {
    builtin_variants()
        .into_iter()
        .find_map(|(k, b)| (b == *v).then_some(k))
}

/// The default lineup's human playlists, offered as templates for a new
/// playlist: each gives the descriptor (type, team and party sizes, waits,
/// timeout, skill ceiling) a playlist of its kind needs to pass the game's
/// validator, which the editors' raw fields cannot build. The clanker
/// playlists are left out: their mark is kept by playlist id and index
/// ([`super::clankers`]), which a copy elsewhere would not have.
pub fn templates() -> Vec<Playlist> {
    playlists()
        .into_iter()
        .filter(|p| p.clankers.is_none())
        .collect()
}

/// The template whose default playlist id is `playlist_id`.
pub fn template(playlist_id: u32) -> Option<Playlist> {
    templates()
        .into_iter()
        .find(|p| p.playlist_id == playlist_id)
}

/// One entry of a default playlist: `(variant, map id, weight)`.
pub type DefaultEntry = (fn() -> VariantRecord, u32, u32);

/// One default playlist, as written into the file.
pub struct Playlist {
    pub playlist_id: u32,
    pub title: &'static str,
    pub description: &'static str,
    pub rules: Rules,
    pub base_wait_ms: u32,
    pub search_timeout_ms: u32,
    pub entries: Vec<DefaultEntry>,
    /// A clanker hopper's mark (docs/game/bots.md §9.4 item 1): not a game
    /// field, kept beside the descriptor and served on
    /// `GET /v1/hoppers/clankers`. `None` for a human hopper.
    pub clankers: Option<Clankers>,
}

impl Playlist {
    /// The descriptor this playlist writes.
    pub fn descriptor(&self) -> Descriptor {
        let mut d = Descriptor {
            playlist_id: self.playlist_id,
            skill_ceiling: default_skill_ceiling(),
            search_timeout_ms: self.search_timeout_ms,
            ..Descriptor::default()
        };
        d.set_rules(&self.rules);
        d.set_text(self.title, self.description);
        let (teams, size) = match self.rules {
            Rules::FreeForAll { players, .. } => (1, players),
            Rules::UnrankedTeams {
                teams, team_size, ..
            }
            | Rules::RankedTeams {
                teams, team_size, ..
            }
            | Rules::Clan {
                teams, team_size, ..
            } => (teams, team_size),
        };
        d.start_wait_ms = start_waits(self.base_wait_ms, teams, size);
        d
    }

    pub fn entries(&self) -> Vec<Entry> {
        self.entries
            .iter()
            .map(|(variant, map_id, weight)| Entry {
                map_id: *map_id,
                weight: *weight,
                map_name: String::new(),
                variant: variant(),
            })
            .collect()
    }
}

fn team(
    teams: u32,
    size: (u32, u32),
    party: (u32, u32),
    party_as_team: Option<(u32, u32)>,
) -> Rules {
    Rules::RankedTeams {
        teams,
        team_size: size,
        max_difference: 0,
        party,
        party_as_team,
    }
}

/// The human playlist left out to fit seven human and nine clanker
/// playlists in the 16 slots. Change this one id to drop a different one.
const DROPPED_HUMAN_PLAYLIST: u32 = 8;

/// The default lineup, in hopper order. Playlist ids are 1.. in order.
pub fn playlists() -> Vec<Playlist> {
    use variants::*;
    let small_ffa = [
        LOCKOUT,
        MIDSHIP,
        IVORY_TOWER,
        BEAVER_CREEK,
        ASCENSION,
        FOUNDATION,
        WARLOCK,
        SANCTUARY,
        ELONGATION,
        GEMINI,
    ];
    let small_team = [
        LOCKOUT,
        MIDSHIP,
        IVORY_TOWER,
        BEAVER_CREEK,
        ASCENSION,
        COLOSSUS,
        FOUNDATION,
        WARLOCK,
        SANCTUARY,
        TURF,
        ELONGATION,
        BACKWASH,
        RELIC,
    ];
    let big = [
        COAGULATION,
        HEADLONG,
        ZANZIBAR,
        WATERWORKS,
        BURIAL_MOUNDS,
        CONTAINMENT,
        TERMINAL,
        RELIC,
        GEMINI,
    ];
    let on = |v: fn() -> VariantRecord, maps: &[u32], w: u32| {
        maps.iter().map(|m| (v, *m, w)).collect::<Vec<_>>()
    };
    let rumble_entries = || {
        [
            on(rumble_slayer, &small_ffa, 10),
            on(oddball, &[LOCKOUT, MIDSHIP, WARLOCK], 3),
            on(king_of_the_hill, &[IVORY_TOWER, ASCENSION, SANCTUARY], 3),
        ]
        .concat()
    };
    let free_for_all_entries = || {
        [
            on(rumble_slayer, &small_ffa, 10),
            on(oddball, &[MIDSHIP, WARLOCK], 3),
            on(king_of_the_hill, &[ASCENSION, SANCTUARY], 3),
        ]
        .concat()
    };
    let slayer_entries = || {
        [
            on(team_slayer, &small_team, 10),
            on(team_slayer, &[ZANZIBAR], 4),
        ]
        .concat()
    };
    let skirmish_entries = || {
        [
            on(
                team_slayer,
                &[
                    LOCKOUT,
                    MIDSHIP,
                    IVORY_TOWER,
                    BEAVER_CREEK,
                    FOUNDATION,
                    WARLOCK,
                    SANCTUARY,
                ],
                10,
            ),
            on(teamball, &[MIDSHIP, LOCKOUT, WARLOCK], 3),
        ]
        .concat()
    };
    let snipers_entries = || {
        on(
            team_snipers,
            &[
                ZANZIBAR,
                COAGULATION,
                BURIAL_MOUNDS,
                ASCENSION,
                COLOSSUS,
                LOCKOUT,
                HEADLONG,
                CONTAINMENT,
                TERMINAL,
                RELIC,
                SANCTUARY,
            ],
            10,
        )
    };
    let big_battle_entries = || {
        [
            on(team_slayer, &big, 10),
            on(
                classic_ctf,
                &[
                    COAGULATION,
                    ZANZIBAR,
                    HEADLONG,
                    WATERWORKS,
                    CONTAINMENT,
                    TERMINAL,
                    RELIC,
                ],
                6,
            ),
            on(
                assault,
                &[
                    ZANZIBAR,
                    COAGULATION,
                    BURIAL_MOUNDS,
                    CONTAINMENT,
                    TERMINAL,
                ],
                4,
            ),
            on(
                territories,
                &[HEADLONG, WATERWORKS, COAGULATION, CONTAINMENT, RELIC],
                4,
            ),
        ]
        .concat()
    };
    let training_entries = || {
        [
            on(team_slayer, &small_team, 10),
            on(classic_ctf, &[BEAVER_CREEK, MIDSHIP, SANCTUARY], 4),
            on(team_king, &[IVORY_TOWER, WARLOCK], 3),
            on(teamball, &[LOCKOUT, SANCTUARY], 3),
        ]
        .concat()
    };
    let human = vec![
        Playlist {
            playlist_id: 1,
            title: "Rumble Pit",
            description: "Every player for themselves, up to 8. Slayer, with some Oddball and King of the Hill.",
            rules: Rules::FreeForAll {
                ranked: true,
                players: (4, 8),
            },
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: rumble_entries(),
            clankers: None,
        },
        Playlist {
            playlist_id: 2,
            title: "Team Slayer",
            description: "Four against four Slayer on the small and medium maps.",
            rules: team(2, (3, 4), (1, 4), Some((3, 4))),
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: slayer_entries(),
            clankers: None,
        },
        Playlist {
            playlist_id: 3,
            title: "Team Skirmish",
            description: "Two against two. Bring a partner or find one here.",
            rules: team(2, (2, 2), (1, 2), Some((2, 2))),
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: skirmish_entries(),
            clankers: None,
        },
        Playlist {
            playlist_id: 4,
            title: "Team Snipers",
            description: "Four against four with sniper rifles and no vehicles.",
            rules: team(2, (3, 4), (1, 4), None),
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: snipers_entries(),
            clankers: None,
        },
        Playlist {
            playlist_id: 5,
            title: "Big Team Battle",
            description: "Up to eight a side on the big maps: Slayer, CTF, Assault and Territories.",
            rules: team(2, (5, 8), (1, 5), None),
            base_wait_ms: BIG_WAIT,
            search_timeout_ms: BIG_TIMEOUT,
            entries: big_battle_entries(),
            clankers: None,
        },
        Playlist {
            playlist_id: 6,
            title: "Team Objective",
            description: "Four against four: Capture the Flag, Assault, Territories, King and Oddball.",
            rules: team(2, (3, 4), (1, 4), Some((3, 4))),
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: [
                on(
                    classic_ctf,
                    &[BEAVER_CREEK, MIDSHIP, COLOSSUS, ASCENSION, SANCTUARY],
                    6,
                ),
                on(assault, &[MIDSHIP, BEAVER_CREEK, TURF], 4),
                on(territories, &[LOCKOUT, ASCENSION, COLOSSUS, WARLOCK], 4),
                on(team_king, &[IVORY_TOWER, MIDSHIP, WARLOCK], 3),
                on(teamball, &[LOCKOUT, BEAVER_CREEK, SANCTUARY], 3),
            ]
            .concat(),
            clankers: None,
        },
        Playlist {
            playlist_id: 7,
            title: "Free for All",
            description: "Unranked free for all, 2 to 8 players. Parties welcome.",
            rules: Rules::FreeForAll {
                ranked: false,
                players: (2, 8),
            },
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: free_for_all_entries(),
            clankers: None,
        },
        Playlist {
            playlist_id: 8,
            title: "Unranked Teams",
            description: "Unranked team games for parties of any size, 2 to 4 a side.",
            rules: Rules::UnrankedTeams {
                teams: 2,
                team_size: (2, 4),
                uneven: true,
            },
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: training_entries(),
            clankers: None,
        },
    ];
    // Clankermatch (docs/game/bots.md §8): one human (or a party) and bots
    // up to the most players. Every descriptor is an unranked type: the
    // game's own search and lobby read it, and a ranked type's party and
    // team-size rules are made for filling a game with humans. An unranked
    // type lets a lone human host, with uneven teams while the bots join.
    // `Clankers::ranked` independently tells the server which finished
    // games rank. Every ranked playlist keeps its own standing
    // (`ranks_as: None`). The Clankermatch screen supplies the Clanker
    // context, so the canonical titles fit the descriptor's 15-byte limit.
    let clanker = vec![
        Playlist {
            playlist_id: 8,
            title: "Rumble Pit",
            description: "Ranked Clanker free for all, up to 8. Bots fill every seat your party does not.",
            rules: Rules::FreeForAll {
                ranked: false,
                players: (2, 8),
            },
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: rumble_entries(),
            clankers: Some(Clankers::RANKED),
        },
        Playlist {
            playlist_id: 9,
            title: "Team Slayer",
            description: "Ranked Clanker Team Slayer, four against four. Bots fill every seat your party does not.",
            rules: Rules::UnrankedTeams {
                teams: 2,
                team_size: (1, 4),
                uneven: true,
            },
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: slayer_entries(),
            clankers: Some(Clankers::RANKED),
        },
        Playlist {
            playlist_id: 10,
            title: "Team Skirmish",
            description: "Ranked Clanker Team Skirmish, two against two. Bots fill every seat your party does not.",
            rules: Rules::UnrankedTeams {
                teams: 2,
                team_size: (1, 2),
                uneven: true,
            },
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: skirmish_entries(),
            clankers: Some(Clankers::RANKED),
        },
        Playlist {
            playlist_id: 11,
            title: "Team Snipers",
            description: "Ranked Clanker Team Snipers, four against four. Bots fill every seat your party does not.",
            rules: Rules::UnrankedTeams {
                teams: 2,
                team_size: (1, 4),
                uneven: true,
            },
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: snipers_entries(),
            clankers: Some(Clankers::RANKED),
        },
        Playlist {
            playlist_id: 12,
            title: "Big Team Battle",
            description: "Ranked Clanker Big Team Battle on big maps. Bots fill every seat your party does not.",
            rules: Rules::UnrankedTeams {
                teams: 2,
                team_size: (1, 8),
                uneven: true,
            },
            base_wait_ms: BIG_WAIT,
            search_timeout_ms: BIG_TIMEOUT,
            entries: big_battle_entries(),
            clankers: Some(Clankers::RANKED),
        },
        Playlist {
            playlist_id: 13,
            title: "Big Team Objective",
            description: "Ranked Clanker CTF, Assault and Territories on big maps. Bots fill the other seats.",
            rules: Rules::UnrankedTeams {
                teams: 2,
                team_size: (1, 8),
                uneven: true,
            },
            base_wait_ms: BIG_WAIT,
            search_timeout_ms: BIG_TIMEOUT,
            entries: [
                on(classic_ctf, &big, 8),
                on(
                    assault,
                    &[
                        COAGULATION,
                        ZANZIBAR,
                        BURIAL_MOUNDS,
                        CONTAINMENT,
                        TERMINAL,
                        RELIC,
                    ],
                    5,
                ),
                on(
                    territories,
                    &[
                        COAGULATION,
                        HEADLONG,
                        WATERWORKS,
                        BURIAL_MOUNDS,
                        CONTAINMENT,
                        TERMINAL,
                    ],
                    5,
                ),
            ]
            .concat(),
            clankers: Some(Clankers::RANKED),
        },
        Playlist {
            playlist_id: 14,
            title: "Big Team Heavies",
            description: "Ranked Clanker Team Heavies on big maps. Bots fill every seat your party does not.",
            rules: Rules::UnrankedTeams {
                teams: 2,
                team_size: (1, 8),
                uneven: true,
            },
            base_wait_ms: BIG_WAIT,
            search_timeout_ms: BIG_TIMEOUT,
            entries: on(team_heavies, &big, 10),
            clankers: Some(Clankers::RANKED),
        },
        Playlist {
            playlist_id: 15,
            title: "Solo Training",
            description: "Unranked Clanker free for all training, up to 8. Bots fill every seat your party does not.",
            rules: Rules::FreeForAll {
                ranked: false,
                players: (2, 8),
            },
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: free_for_all_entries(),
            clankers: Some(Clankers::UNRANKED),
        },
        Playlist {
            playlist_id: 16,
            title: "Team Training",
            description: "Unranked Clanker team training, four against four. Bots fill every seat your party does not.",
            rules: Rules::UnrankedTeams {
                teams: 2,
                team_size: (1, 4),
                uneven: true,
            },
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: training_entries(),
            clankers: Some(Clankers::UNRANKED),
        },
    ];
    human
        .into_iter()
        .filter(|p| p.playlist_id != DROPPED_HUMAN_PLAYLIST)
        .chain(clanker)
        .collect()
}

impl HopperFile {
    /// Whether hopper `i` is an unused slot: type 0, which the game reads
    /// as no hopper (every reader goes through the validator, which refuses
    /// it). The default set fills every slot.
    pub fn is_unused(&self, i: usize) -> Result<bool, HopperError> {
        Ok(self.descriptor(i)?.hopper_type == 0)
    }

    /// The lowest playlist id (1..=100) no other used hopper has; the id is
    /// the base of the playlist's stats ids, so it must be unique.
    pub fn free_playlist_id(&self, i: usize) -> Option<u32> {
        let taken: Vec<u32> = (0..HOPPERS)
            .filter(|&j| j != i)
            .filter_map(|j| self.descriptor(j).ok())
            .filter(|d| d.hopper_type != 0)
            .map(|d| d.playlist_id)
            .collect();
        PLAYLIST_IDS.into_iter().find(|id| !taken.contains(id))
    }

    /// Makes hopper `i` a playlist of `template`'s kind: its whole
    /// descriptor (title and description included), status 0, with a
    /// playlist id no other hopper uses. The entries are left as they are:
    /// write at least one before publishing.
    pub fn create_from(&mut self, i: usize, template: &Playlist) -> Result<(), HopperError> {
        let mut d = template.descriptor();
        d.playlist_id = self.free_playlist_id(i).ok_or(HopperError::NoPlaylistId)?;
        self.set_descriptor(i, &d)
    }

    /// Makes hopper `i` an unused slot again: its descriptor and data block
    /// all zero, as [`HopperFile::default_set`] leaves the slots it does not
    /// fill.
    pub fn clear_hopper(&mut self, i: usize) -> Result<(), HopperError> {
        if i >= HOPPERS {
            return Err(HopperError::Hopper(i));
        }
        let image = self.image_mut();
        image[i * DESCRIPTOR..(i + 1) * DESCRIPTOR].fill(0);
        let block = BLOCKS_AT + i * BLOCK;
        image[block..block + BLOCK].fill(0);
        Ok(())
    }

    /// A complete file built from scratch: [`playlists`] in hoppers
    /// 0.., every other byte zero (unused hopper slots are all zero, which
    /// the game reads as no hopper).
    pub fn default_set() -> HopperFile {
        let mut file = HopperFile::blank();
        for (i, p) in playlists().iter().enumerate() {
            debug_assert!(p.title.len() <= MAX_TITLE && p.description.len() <= MAX_DESCRIPTION);
            file.set_descriptor(i, &p.descriptor())
                .expect("the default descriptors fit");
            file.set_entries(i, &p.entries())
                .expect("the default entries fit");
        }
        file
    }
}

#[cfg(test)]
mod tests {
    use super::super::layout::{HopperType, check_descriptor};
    use super::super::variant as v;
    use super::super::{BLOCK, BLOCKS_AT, DESCRIPTOR, HOPPERS, MAX_FILE};
    use super::*;

    #[test]
    fn the_default_set_passes_every_check_the_game_makes() {
        let file = HopperFile::default_set();
        assert_eq!(file.problems(&retail_map_ids()), vec![]);
        let lineup = playlists();
        let mut ids = Vec::new();
        for (i, p) in lineup.iter().enumerate() {
            let d = file.descriptor(i).unwrap();
            assert_eq!(check_descriptor(&d), vec![], "{}", p.title);
            assert_eq!(d.status, 0, "the game checks the maps itself");
            assert_eq!(d.rules(), Some(p.rules));
            assert_eq!(d.titles.iter().filter(|t| *t == p.title).count(), 9);
            assert!(p.title.len() <= MAX_TITLE && p.description.len() <= MAX_DESCRIPTION);
            ids.push(d.playlist_id);
            // Unused fields are zero.
            assert_eq!((d.base_wait_ms, d.min_rank, d.list_any_rank), (0, 0, false));
            let entries = file.entries(i).unwrap();
            assert_eq!(entries, p.entries());
            assert!((1..=100).contains(&entries.len()));
            for e in &entries {
                assert!(e.map_name.is_empty());
                assert_eq!(e.variant.problems(), vec![]);
                let teams = d.kind().unwrap().teams();
                assert_eq!(e.variant.team(), teams, "{} on {}", p.title, e.map_id);
            }
            // The old view agrees.
            let h = file.hopper(i).unwrap();
            assert_eq!(h.record_count as usize, entries.len());
            assert_eq!(h.name, "");
            assert_eq!(file.text(i).unwrap().title, p.title);
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), lineup.len(), "playlist ids are unique");
        // The rest of the image is zero.
        let img = file.image();
        assert!(
            img[lineup.len() * DESCRIPTOR..BLOCKS_AT]
                .iter()
                .all(|b| *b == 0)
        );
        assert!(
            img[BLOCKS_AT + lineup.len() * BLOCK..]
                .iter()
                .all(|b| *b == 0)
        );
        for i in 0..lineup.len() {
            let block = BLOCKS_AT + i * BLOCK;
            assert!(img[block..block + 0x44].iter().all(|b| *b == 0));
        }
        assert!(lineup.len() <= HOPPERS);
    }

    #[test]
    fn the_lineup_has_seven_human_and_nine_clanker_playlists() {
        let lineup = playlists();
        let titles: Vec<&str> = lineup.iter().map(|p| p.title).collect();
        assert_eq!(
            titles,
            [
                "Rumble Pit",
                "Team Slayer",
                "Team Skirmish",
                "Team Snipers",
                "Big Team Battle",
                "Team Objective",
                "Free for All",
                "Rumble Pit",
                "Team Slayer",
                "Team Skirmish",
                "Team Snipers",
                "Big Team Battle",
                "Big Team Objective",
                "Big Team Heavies",
                "Solo Training",
                "Team Training",
            ]
        );
        let kinds: Vec<HopperType> = lineup.iter().map(|p| p.rules.hopper_type()).collect();
        assert!(kinds[..6].iter().all(|k| k.ranked()));
        assert!(kinds[6..].iter().all(|k| !k.ranked()));
        // The human playlists first, then the clanker ones. Seven clanker
        // playlists rank in their own ladders; the two training ones do not.
        let marked: Vec<bool> = lineup.iter().map(|p| p.clankers.is_some()).collect();
        assert_eq!(marked, (0..16).map(|i| i >= 7).collect::<Vec<bool>>());
        let ranked: Vec<bool> = lineup
            .iter()
            .map(|p| p.clankers.is_some_and(|c| c.ranked))
            .collect();
        assert_eq!(
            ranked,
            (0..16).map(|i| (7..14).contains(&i)).collect::<Vec<bool>>()
        );
        assert!(lineup.iter().all(|p| p.clankers.is_none_or(|c| c.ranks_as.is_none())));
        let ids: Vec<u32> = lineup.iter().map(|p| p.playlist_id).collect();
        assert_eq!(ids, (1..=16).collect::<Vec<u32>>());
        assert_eq!(DROPPED_HUMAN_PLAYLIST, 8, "the human Unranked Teams playlist");
        let file = HopperFile::default_set();
        let d = file.descriptor(2).unwrap();
        assert_eq!(
            (d.min_players(), d.max_players()),
            (Some(4), Some(4)),
            "2v2"
        );
        let d = file.descriptor(4).unwrap();
        assert_eq!((d.min_players(), d.max_players()), (Some(10), Some(16)));
        assert_eq!(d.search_timeout_ms, 300_000);
        assert_eq!(d.start_wait(10), Some(45_000));
        let d = file.descriptor(11).unwrap();
        assert_eq!((d.min_players(), d.max_players()), (Some(2), Some(16)));
        assert_eq!(d.max_party(), Some(16), "a clanker party can be any size");
        let d = file.descriptor(0).unwrap();
        assert_eq!(d.max_party(), Some(1), "ranked free for all is solo");
        // Every map is retail; every Map Pack map is in a rotation, and every
        // game type the lineup names shows.
        let maps = retail_map_ids();
        let mut engines: Vec<u32> = Vec::new();
        let mut used_maps = Vec::new();
        for p in &lineup {
            for e in p.entries() {
                assert!(maps.contains(&e.map_id));
                engines.push(e.variant.engine());
                used_maps.push(e.map_id);
            }
        }
        engines.sort();
        engines.dedup();
        assert_eq!(engines, [1, 2, 3, 4, 8, 9]);
        for (id, name) in MAP_PACK_MAPS {
            assert!(used_maps.contains(&id), "{name} is not in a rotation");
        }
        // The compatibility file keeps a base-disc entry in every playlist.
        let mut disc = HopperFile::default_set();
        assert!(disc.retain_base_maps().unwrap());
        assert_eq!(disc.problems(&base_map_ids()), vec![]);
        for i in 0..HOPPERS {
            for e in disc.entries(i).unwrap() {
                assert!(super::super::is_base_map(e.map_id));
            }
        }
    }

    #[test]
    fn the_default_file_round_trips_byte_for_byte_and_fits_the_buffer() {
        let file = HopperFile::default_set();
        let built = file.build_checked().unwrap();
        assert!(built.len() <= MAX_FILE, "{} bytes", built.len());
        let back = HopperFile::parse(&built).unwrap();
        assert!(back.image() == file.image());
        assert_eq!(back.build(), built);
        assert_eq!(HopperFile::default_set().build(), built, "deterministic");
        // Rewriting every decoded field from what was read changes nothing.
        let mut again = back.clone();
        for i in 0..HOPPERS {
            again
                .set_descriptor(i, &back.descriptor(i).unwrap())
                .unwrap();
            again.set_entries(i, &back.entries(i).unwrap()).unwrap();
        }
        assert!(again.image() == file.image());
    }

    /// The mode of a record's script reference (`+0x11f`, the first byte of
    /// the `hx` block's reserved bytes: 0 none, 1 Firefight, 2 a rules
    /// script) and its hash prefix.
    fn script_of(r: &VariantRecord) -> (u8, [u8; 8]) {
        let v = ::variants::record::GameVariant::from_bytes(r.bytes()).unwrap();
        let s = v.ext().script;
        (s.mode.to_u8(), s.prefix)
    }

    #[test]
    fn the_default_lineup_is_all_mode_2_and_names_its_built_in_script() {
        let mut named = Vec::new();
        for p in playlists() {
            assert!(!p.entries.is_empty(), "{}", p.title);
            for e in p.entries() {
                let (mode, prefix) = script_of(&e.variant);
                assert_eq!(mode, 2, "{}: {}", p.title, e.variant.name());
                named.push(prefix);
            }
        }
        assert!(playlists().iter().any(|p| p.clankers.is_some()), "the clanker playlists are in");
        // Each prefix is the hash of a script in `MATCHMAKING`.
        let hashes: Vec<[u8; 8]> = ::rules::builtin::MATCHMAKING
            .iter()
            .map(|m| ::rules::compile(m.source).unwrap().id().0[..8].try_into().unwrap())
            .collect();
        for prefix in named {
            assert!(hashes.contains(&prefix));
        }
        for (k, r) in builtin_variants() {
            assert_eq!(script_of(&r).0, 2, "{k}");
        }
        assert_eq!(builtin_variants().len(), ::rules::builtin::MATCHMAKING.len());
    }

    #[test]
    fn the_variants_are_the_stock_records_with_their_changes() {
        use super::variants::*;
        let ts = team_snipers();
        assert_eq!(ts.name(), "Team Snipers");
        assert!(ts.team());
        assert_eq!(ts.flags() & v::flag::MOTION_SENSOR, 0);
        assert_eq!(ts.u8_at(v::STARTING_WEAPON), 10);
        let th = team_heavies();
        assert_eq!(th.name(), "Team Heavies");
        assert!(th.team());
        assert_eq!(th.u8_at(v::STARTING_WEAPON), 8);
        assert_eq!(th.u8_at(v::SECONDARY_WEAPON), 4);
        assert_eq!(th.u8_at(v::WEAPONS_ON_MAP), 2);
        let rs = rumble_slayer();
        assert!(!rs.team());
        assert_eq!(
            (rs.i32_at(v::SCORE_TO_WIN), rs.i32_at(v::TIME_LIMIT)),
            (25, 360)
        );
        let a = assault();
        assert_eq!((a.i32_at(v::ROUNDS), a.i32_at(v::SCORE_TO_WIN)), (2, 1));
        assert_eq!(classic_ctf().u32_at(v::TYPE_AREA), 0xe);
        for r in [
            team_slayer(),
            oddball(),
            king_of_the_hill(),
            territories(),
            team_king(),
            teamball(),
        ] {
            assert_eq!(r.problems(), vec![], "{}", r.name());
            assert_eq!(r.i32_at(v::SETTING_14), 0);
        }
    }

    #[test]
    fn the_built_in_variants_are_keyed_clean_and_cover_the_lineup() {
        let all = builtin_variants();
        for (k, v) in &all {
            assert_eq!(v.problems(), vec![], "{k}");
            assert_eq!(builtin_variant(k).as_ref(), Some(v));
            assert_eq!(builtin_key(v), Some(*k));
        }
        let mut keys: Vec<&str> = all.iter().map(|(k, _)| *k).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), all.len(), "keys are unique");
        assert_eq!(builtin_variant("no_such_variant"), None);
        // Every default entry carries one of them, so a template's entries
        // can be named by key.
        for p in playlists() {
            for e in p.entries() {
                assert!(builtin_key(&e.variant).is_some(), "{}", p.title);
            }
        }
    }

    #[test]
    fn a_template_fills_an_unused_slot_the_game_takes_and_clearing_empties_it() {
        let mut file = HopperFile::default_set();
        let n = HOPPERS - 1;
        assert!(!file.is_unused(0).unwrap());
        file.clear_hopper(n).unwrap();
        let cleared = file.image().to_vec();
        assert!(file.is_unused(n).unwrap());
        let t = template(2).unwrap();
        assert_eq!(t.title, "Team Slayer");
        file.create_from(n, &t).unwrap();
        file.set_entries(n, &t.entries()[..1]).unwrap();
        let d = file.descriptor(n).unwrap();
        assert_eq!(check_descriptor(&d), vec![]);
        assert_eq!(d.playlist_id, 17, "the lowest id no other playlist has");
        assert_eq!(d.kind(), Some(t.rules.hopper_type()));
        assert_eq!(file.problems(&retail_map_ids()), vec![]);
        assert!(!file.is_unused(n).unwrap());
        // Clearing makes it the all-zero slot it was.
        file.clear_hopper(n).unwrap();
        assert_eq!(file.image(), cleared);
        assert_eq!(
            file.clear_hopper(HOPPERS),
            Err(HopperError::Hopper(HOPPERS))
        );
        // Only the seven human playlists are templates.
        assert!(template(8).is_none(), "clanker playlists are not templates");
        assert_eq!(templates().len(), 7);
    }
}
