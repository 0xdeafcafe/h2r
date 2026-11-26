//! The wave set in a document: the format, the identity, the validator,
//! the tier, the zone order, the remap and the problems. Every set is
//! invented.
use std::collections::BTreeMap;

use firefight::{Caps, Count, DifficultyMultipliers, Drop, Lives, Round, Scoring, SquadSpec, Wave};

use super::*;
use crate::waves::{self, MAX_WAVES_BYTES, Problem, ProblemKind};

fn squad(class: &str, drop: Drop) -> SquadSpec {
    SquadSpec {
        class: class.into(),
        count: Count {
            per_player: 1,
            min: 1,
            max: 2,
        },
        drop,
        weapon_tier: 0,
    }
}

/// An invented set: `rounds` rounds of five waves, each wave the squads
/// `squads` gives.
fn set_of(name: &str, rounds: usize, squads: &[SquadSpec]) -> waves::Def {
    let mut by_class = BTreeMap::new();
    for s in squads {
        by_class.insert(s.class.clone(), 100);
    }
    waves::Def {
        format: firefight::FORMAT,
        name: name.into(),
        teams: 1,
        set: firefight::SetRules::finite(),
        rounds: (0..rounds)
            .map(|_| Round {
                waves: (0..5)
                    .map(|i| Wave {
                        squads: squads.to_vec(),
                        boss: i == 4,
                        gap_ticks: 300,
                    })
                    .collect(),
                skulls_on: vec![],
            })
            .collect(),
        lives: Lives {
            pool_start: 5,
            per_round_bonus: 1,
            shared: true,
        },
        caps: Caps {
            live_actors_by_players: vec![(4, 32)],
        },
        scoring: Scoring {
            by_class,
            player_kill: 0,
            difficulty_multiplier: DifficultyMultipliers {
                easy: 1000,
                normal: 1000,
                heroic: 1000,
                legendary: 1000,
            },
            skull_multipliers: BTreeMap::new(),
            assist: 500,
        },
    }
}

fn yard() -> waves::Def {
    set_of(
        "Yard",
        1,
        &[
            squad("elite", Drop::PreferZone(0)),
            squad("elite", Drop::Edge),
        ],
    )
}

fn with_waves(def: waves::Def) -> ForgeDoc {
    draft().with_waves(Some(def))
}

#[test]
fn a_document_with_waves_round_trips_at_the_waves_format() {
    let doc = with_waves(yard());
    assert_eq!(doc.format, FORMAT_WAVES);
    assert_eq!(FORMAT_WAVES, 5);
    assert_eq!(doc.required_format(), FORMAT_WAVES);
    let json = doc.canonical_json().unwrap();
    assert!(json.contains("\"waves\":{"), "{json}");
    let again = ForgeDoc::from_bytes(json.as_bytes(), &Limits::default()).unwrap();
    assert_eq!(again, doc);
    assert_eq!(again.id().unwrap(), doc.id().unwrap());
    assert_eq!(again.waves.unwrap().id().unwrap(), yard().id().unwrap());

    // The set is in the identity.
    let mut other = yard();
    other.name = "Other".into();
    assert_ne!(with_waves(other).id().unwrap(), doc.id().unwrap());

    // A document has one spelling: a set at another number, or the number
    // without a set, is refused.
    let mut wrong = doc.clone();
    wrong.format = FORMAT_GROUPS;
    assert_eq!(
        validate(&wrong, &Limits::default()),
        Err(Refusal::Format(FORMAT_GROUPS))
    );
    let mut bare = draft();
    bare.format = FORMAT_WAVES;
    assert_eq!(
        validate(&bare, &Limits::default()),
        Err(Refusal::Format(FORMAT_WAVES))
    );
}

#[test]
fn a_document_without_waves_keeps_its_identity() {
    let plain = draft();
    assert_eq!(plain.canonical_json().unwrap(), EMPTY);
    // The golden identity of `canonical_document_golden_vectors`.
    assert_eq!(
        plain.id().unwrap().to_string(),
        "77806693f7f1cea053e7a5fe7a3cd3fa28222cb01b8c282b6018a4abbb909001"
    );
    // Taking a set out gives back the document as it was, number and all.
    let back = with_waves(yard()).with_waves(None);
    assert_eq!(back, plain);
    assert_eq!(back.id().unwrap(), plain.id().unwrap());
    // A labelled document keeps its own number.
    let mut labelled = draft();
    labelled.ops = vec![
        add(1, BlockKind::NetgameFlags),
        Op {
            id: OpId(2),
            edit: Edit::Forge(Forge::Label {
                target: Endpoint::Added(OpId(1)),
                label: Some(Label::new("ff_drop", None).unwrap()),
            }),
        },
    ];
    labelled.format = labelled.required_format();
    let format = labelled.format;
    assert_eq!(
        labelled.with_waves(Some(yard())).with_waves(None).format,
        format
    );
    // Setting the same set again changes nothing, `expect` included.
    let mut doc = with_waves(yard());
    doc.expect = Some(Expected {
        map_id: MapId::forge(Sha256([1; 32]), Sha256([2; 32])),
        geometry: Sha256([3; 32]),
    });
    assert_eq!(doc.with_waves(Some(yard())), doc);
    assert_eq!(doc.with_waves(None).expect, None);
}

#[test]
fn a_set_naming_an_unknown_class_is_refused() {
    let ogres = set_of("Ogres", 1, &[squad("ogre", Drop::Edge)]);
    let error = validate(&with_waves(ogres.clone()), &Limits::default()).unwrap_err();
    assert_eq!(
        error,
        Refusal::Invalid {
            what: "waves".into(),
            reason: "the kit does not provide class ogre".into(),
        }
    );
    assert_eq!(waves::check(&ogres), Err(error));
    // Every class of the fixed list is admitted, built or not.
    for class in waves::CLASSES {
        waves::check(&set_of("Any", 1, &[squad(class, Drop::Edge)])).unwrap();
    }
    // The validator's own sentence: a wave over the enemy limit.
    let mut over = yard();
    over.caps.live_actors_by_players = vec![(4, 2)];
    assert_eq!(
        waves::check(&over).unwrap_err().to_string(),
        "waves: round 1, wave 1 needs 4 actors for 2 players (cap 2)"
    );
}

#[test]
fn an_oversize_set_is_refused_before_parsing() {
    // Twelve rounds of sixteen squads is over 64 KiB, and the set's own
    // format is one the validator would refuse first: the size is checked
    // before the validator reads it.
    let squads: Vec<SquadSpec> = (0..16)
        .map(|_| SquadSpec {
            count: Count {
                per_player: 0,
                min: 1,
                max: 1,
            },
            ..squad("elite", Drop::PreferZone(11))
        })
        .collect();
    let mut big = set_of("Big", 12, &squads);
    big.format = 99;
    let limit = Refusal::Limit {
        what: "waves bytes".into(),
        max: MAX_WAVES_BYTES,
    };
    assert_eq!(MAX_WAVES_BYTES, 64 * 1024);
    assert_eq!(waves::check(&big), Err(limit.clone()));

    // The same through a document read from bytes.
    let doc = with_waves(big);
    let json = serde_json::to_vec(&doc).unwrap();
    assert!(json.len() > MAX_WAVES_BYTES);
    assert_eq!(ForgeDoc::from_bytes(&json, &Limits::default()), Err(limit));
}

#[test]
fn a_variant_with_waves_is_a_map_variant() {
    let mut doc = with_waves(yard());
    doc.title = "Yard".into();
    doc.recipes = vec![
        GroundRecipe {
            kit_version: 3,
            home: 0,
            probe: false,
            classes: vec![],
        }
        .to_recipe(),
    ];
    doc.ops = vec![add(1, BlockKind::NetgameFlags)];
    doc.format = doc.required_format();
    validate(&doc, &Limits::default()).unwrap();
    assert_eq!(doc.tier(), Tier::Variant);
    doc.check_variant().unwrap();
    // The play form keeps the set.
    assert_eq!(play_form(&session_form(&doc)).waves, Some(yard()));
}

#[test]
fn the_zone_order_is_by_position_then_facing() {
    let drops = [
        ('c', [2.0, 0.0, 0.0], 0.0),
        ('a', [-1.0, 5.0, 0.0], 0.0),
        ('d', [2.0, 0.0, 0.0], 1.0),
        ('b', [-1.0, 5.0, 0.5], 0.0),
        ('e', [-0.0, 0.0, 0.0], 0.0),
        ('f', [0.0, 0.0, 0.0], 0.0),
    ];
    // total_cmp puts -0 before 0.
    assert_eq!(waves::zone_order(&drops), ['a', 'b', 'e', 'f', 'c', 'd']);
}

#[test]
fn remap_follows_each_marker_and_sends_a_lost_one_to_the_edge() {
    let def = set_of(
        "Map",
        2,
        &[
            squad("elite", Drop::PreferZone(0)),
            squad("elite", Drop::PreferZone(1)),
            squad("elite", Drop::PreferZone(2)),
            squad("elite", Drop::PreferZone(7)),
            squad("elite", Drop::Edge),
        ],
    );
    let out = waves::remap(&def, &['a', 'b', 'c'], &['c', 'a']);
    for round in &out.rounds {
        for wave in &round.waves {
            let drops: Vec<Drop> = wave.squads.iter().map(|s| s.drop).collect();
            assert_eq!(
                drops,
                [
                    Drop::PreferZone(1),
                    Drop::Edge,
                    Drop::PreferZone(0),
                    // No marker was zone 7: it is left for the problems.
                    Drop::PreferZone(7),
                    Drop::Edge,
                ]
            );
        }
    }
    assert_eq!(waves::remap(&def, &['a', 'b'], &['a', 'b']), def);
}

#[test]
fn problems_name_each_squad_the_ground_cannot_serve() {
    let def = set_of(
        "Mixed",
        1,
        &[
            squad("elite", Drop::PreferZone(1)),
            squad("grunt", Drop::Edge),
            squad("jackal", Drop::PreferZone(4)),
        ],
    );
    let built = ["elite".to_string()];
    let problems = waves::problems(&def, &built, 2);
    let at = |squad, what| Problem {
        round: 1,
        wave: 1,
        squad,
        what,
    };
    assert_eq!(
        problems[..3],
        [
            at(2, ProblemKind::ClassNotBuilt),
            at(3, ProblemKind::ClassNotBuilt),
            at(3, ProblemKind::ZoneNotPlaced),
        ]
    );
    assert_eq!(problems.len(), 15);
    assert_eq!(problems[14].wave, 5);
    assert!(waves::problems(&yard(), &built, 1).is_empty());
    assert_eq!(
        waves::problems(&yard(), &built, 0),
        (1..=5)
            .map(|wave| Problem {
                wave,
                ..at(1, ProblemKind::ZoneNotPlaced)
            })
            .collect::<Vec<_>>()
    );
}

#[test]
fn seeded_fuzz_of_documents_with_waves() {
    let limits = Limits::default();
    let mut rng = Rng(0x0057_a7e5_f1fe_f16e);
    let classes = ["elite", "grunt", "hunter", "ogre", "Elite"];
    let canonical = with_waves(yard()).canonical_json().unwrap();
    let (mut accepted, mut with_set, mut refused) = (0, 0, 0);
    for case in 0..3_000 {
        let bytes = match case % 3 {
            0 => {
                let mut bytes = canonical.as_bytes().to_vec();
                for _ in 0..rng.below(8) {
                    let pos = rng.below(bytes.len());
                    bytes[pos] = rng.next() as u8;
                }
                bytes
            }
            _ => {
                let squads: Vec<SquadSpec> = (0..rng.below(4) + 1)
                    .map(|_| SquadSpec {
                        class: classes[rng.below(classes.len())].into(),
                        count: Count {
                            per_player: rng.below(4) as u8,
                            min: rng.below(4) as u16,
                            max: rng.below(12) as u16,
                        },
                        drop: match rng.below(3) {
                            0 => Drop::Edge,
                            _ => Drop::PreferZone(rng.below(14) as u8),
                        },
                        weapon_tier: rng.below(5) as u8,
                    })
                    .collect();
                let mut def = set_of("Fuzz", rng.below(3), &squads);
                if rng.below(4) == 0 {
                    def.rounds.iter_mut().for_each(|r| {
                        r.waves.truncate(rng.below(6));
                    });
                }
                let mut doc = if rng.below(5) == 0 {
                    draft()
                } else {
                    with_waves(def)
                };
                doc.format = match rng.below(4) {
                    0 => rng.below(7) as u32,
                    _ => doc.required_format(),
                };
                serde_json::to_vec(&doc).unwrap()
            }
        };
        match ForgeDoc::from_bytes(&bytes, &limits) {
            Ok(doc) => {
                accepted += 1;
                assert_eq!(doc.format, doc.required_format(), "seeded case {case}");
                if let Some(set) = &doc.waves {
                    with_set += 1;
                    assert_eq!(doc.format, FORMAT_WAVES, "seeded case {case}");
                    waves::check(set).unwrap();
                }
                let json = doc.canonical_json().unwrap();
                let again = ForgeDoc::from_bytes(json.as_bytes(), &limits).unwrap();
                assert_eq!(again, doc, "seeded case {case}");
                assert_eq!(again.id().unwrap(), doc.id().unwrap(), "seeded case {case}");
            }
            Err(_) => refused += 1,
        }
    }
    assert!(
        with_set > 0 && refused > 0,
        "{accepted} {with_set} {refused}"
    );
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}
