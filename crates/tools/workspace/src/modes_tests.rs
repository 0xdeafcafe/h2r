//! PK8: the `core` and `modes` packages of the checkout give the scripts,
//! the list entries and the default hopper the Rust constants and
//! `live_proto::hopper::defaults` give.
use std::path::PathBuf;

use content::package::Kind;
use content::playlist::{Clankers, MapRef, Rules};
use live_proto::hopper::clankers::Clankers as LiveClankers;
use live_proto::hopper::defaults::{self, MAP_PACK_MAPS, RETAIL_MAPS};
use live_proto::hopper::layout::Rules as LiveRules;
use rules::builtin::{self, Registry, Role};

use crate::tests::pkg;
use crate::{Loaded, check_package, hopper_fragments as hopper, script_registry as registry};

fn content_dir(name: &str) -> Loaded {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../content")
        .join(name);
    Loaded::from_dir(&dir).unwrap_or_else(|e| panic!("{e}"))
}

fn set() -> Vec<Loaded> {
    vec![content_dir("core"), content_dir("modes")]
}

#[test]
fn the_checkouts_core_and_modes_check_clean() {
    for p in set() {
        let findings: Vec<_> = check_package(&p)
            .into_iter()
            .filter(|f| {
                let kind = f.item.map(|i| p.package.manifest.items[i].kind);
                matches!(kind, Some(Kind::Rules | Kind::Variant | Kind::Playlist))
            })
            .collect();
        assert_eq!(findings, vec![], "{}", p.package.manifest.package.id);
    }
}

#[test]
fn the_registry_from_the_set_is_the_baked_one() {
    let from_set = registry(&set());
    assert_eq!(from_set.problems, Vec::<String>::new());
    let baked = Registry::baked();
    assert_eq!(from_set.scripts(), baked.scripts());
    let listed: Vec<_> = from_set.listed().map(|s| s.name.as_str()).collect();
    let want: Vec<_> = builtin::LISTED.iter().map(|b| b.name).collect();
    assert_eq!(listed, want);
    for b in builtin::ALL {
        assert_eq!(
            from_set.retail(b.engine).map(|s| s.source.as_str()),
            Some(b.source)
        );
    }
}

#[test]
fn a_package_off_removes_its_list_entries() {
    let all = set();
    assert_eq!(registry(&all).listed().count(), builtin::LISTED.len());
    // `modes` off: no entries, no matchmaking scripts, the retail games
    // stay (core is always on).
    let only_core = registry(&all[..1]);
    assert_eq!(only_core.listed().count(), 0);
    assert_eq!(only_core.matchmaking().count(), 0);
    assert_eq!(only_core.retail_scripts().count(), builtin::ALL.len());
    // And the hopper's fragments go with the scripts they name.
    assert_eq!(hopper(&all).len(), 1);
    assert_eq!(hopper(&all[..1]).len(), 0);
}

fn one(kind: &str, source: &str, tag: Option<&str>, bytes: &[u8]) -> Loaded {
    pkg(
        "mine",
        &[],
        &[(kind, source, tag, "all")],
        &[(source, bytes)],
    )
}

#[test]
fn a_script_that_does_not_compile_is_refused_with_its_line() {
    let p = one(
        "rules",
        "m.rules",
        None,
        b"rules \"Mine\" 1\ntype slayer\nnope nope\n",
    );
    let findings = check_package(&p);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].line.is_some(), "{findings:?}");
    // An item that adds no tag is refused one.
    let tagged = one(
        "rules",
        "m.rules",
        Some("a\\b"),
        b"rules \"Mine\" 1\ntype slayer\n",
    );
    assert!(
        check_package(&tagged)[0].what.contains("no tag"),
        "{:?}",
        check_package(&tagged)
    );
    // The registry leaves the bad one out and says so.
    let r = registry(&[p]);
    assert_eq!(r.scripts().len(), 0);
    assert_eq!(r.problems.len(), 1);
}

#[test]
fn a_variant_adds_one_entry_and_a_rules_item_none() {
    let variant = one("variant", "duel.rules", None, builtin::DUEL.as_bytes());
    assert_eq!(check_package(&variant), vec![]);
    let r = registry(&[variant]);
    assert_eq!(r.problems, Vec::<String>::new());
    assert_eq!(
        r.listed().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["Duel"]
    );
    let plain = one("rules", "duel.rules", None, builtin::DUEL.as_bytes());
    let r = registry(&[plain]);
    assert_eq!(r.listed().count(), 0);
    assert_eq!(r.scripts()[0].role, Role::Rules);
    // A rules item outside `core` is never a retail game.
    let slayer = one("rules", "slayer.rules", None, builtin::SLAYER.as_bytes());
    assert!(registry(&[slayer]).retail(2).is_none());
}

#[test]
fn a_fragment_that_names_a_script_the_set_lacks_is_left_out() {
    let fragment = r#"
[[playlist]]
id = 1
title = "T"
description = "D"
rules = { kind = "free-for-all", ranked = false, players = [2, 8] }
base_wait_ms = 30000
search_timeout_ms = 150000

[[playlist.entry]]
variant = "rumble_slayer"
maps = ["Lockout"]
weight = 1
"#;
    let alone = one("playlist", "p.toml", None, fragment.as_bytes());
    assert_eq!(check_package(&alone), vec![]);
    assert!(hopper(&[alone]).is_empty());
    let mut all = set();
    all.push(one("playlist", "p.toml", None, fragment.as_bytes()));
    let got = hopper(&all);
    assert_eq!(got.len(), 2);
    assert_eq!(got[1].0, "mine");
    let bad = one("playlist", "p.toml", None, b"[[playlist]]\nid = 1\n");
    assert!(!check_package(&bad).is_empty());
}

fn map_id(m: &MapRef) -> u32 {
    match m {
        MapRef::Id(id) => *id,
        MapRef::Name(name) => {
            RETAIL_MAPS
                .iter()
                .chain(&MAP_PACK_MAPS)
                .find(|(_, n)| n == name)
                .unwrap_or_else(|| panic!("no map named {name}"))
                .0
        }
    }
}

fn live_rules(r: &Rules) -> LiveRules {
    let pair = |p: &[u32; 2]| (p[0], p[1]);
    match r {
        Rules::FreeForAll { ranked, players } => LiveRules::FreeForAll {
            ranked: *ranked,
            players: pair(players),
        },
        Rules::UnrankedTeams {
            teams,
            team_size,
            uneven,
        } => LiveRules::UnrankedTeams {
            teams: *teams,
            team_size: pair(team_size),
            uneven: *uneven,
        },
        Rules::RankedTeams {
            teams,
            team_size,
            max_difference,
            party,
            party_as_team,
        } => LiveRules::RankedTeams {
            teams: *teams,
            team_size: pair(team_size),
            max_difference: *max_difference,
            party: pair(party),
            party_as_team: party_as_team.as_ref().map(pair),
        },
        Rules::Clan {
            teams,
            team_size,
            max_difference,
        } => LiveRules::Clan {
            teams: *teams,
            team_size: pair(team_size),
            max_difference: *max_difference,
        },
    }
}

#[test]
fn the_hopper_from_the_fragment_equals_the_defaults() {
    let got = hopper(&set());
    assert_eq!(got.len(), 1);
    let (package, fragment) = &got[0];
    assert_eq!(package, "modes");
    let want = defaults::playlists();
    assert_eq!(fragment.playlists.len(), want.len());
    for (f, w) in fragment.playlists.iter().zip(&want) {
        let who = format!("{} ({})", w.title, w.playlist_id);
        assert_eq!(f.id, w.playlist_id, "{who}");
        assert_eq!(f.title, w.title, "{who}");
        assert_eq!(f.description, w.description, "{who}");
        assert_eq!(live_rules(&f.rules), w.rules, "{who}");
        assert_eq!(f.base_wait_ms, w.base_wait_ms, "{who}");
        assert_eq!(f.search_timeout_ms, w.search_timeout_ms, "{who}");
        let clankers = f
            .clankers
            .map(|Clankers { ranked, ranks_as }| LiveClankers { ranked, ranks_as });
        assert_eq!(clankers, w.clankers, "{who}");
        let rows: Vec<(String, u32, u32)> = fragment
            .rows(f)
            .unwrap()
            .iter()
            .map(|r| (r.variant.clone(), map_id(&r.map), r.weight))
            .collect();
        let entries: Vec<(String, u32, u32)> = w
            .entries
            .iter()
            .map(|(variant, map, weight)| {
                let key = defaults::builtin_key(&variant()).expect("a built-in variant");
                (key.to_string(), *map, *weight)
            })
            .collect();
        assert_eq!(rows, entries, "{who}");
    }
    // Every script a fragment names is in the set.
    let have: Vec<String> = registry(&set())
        .matchmaking()
        .map(|s| s.key.clone())
        .collect();
    for v in fragment.variants() {
        assert!(have.contains(&v), "{v}");
    }
}
