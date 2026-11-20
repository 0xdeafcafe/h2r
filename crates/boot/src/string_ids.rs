//! Every family of string ids of ours, checked against every other: no id
//! of one family may be claimed by another's lookup. Two collisions got
//! into play-tests before this (the skull labels over the campaign
//! picker's, the co-op texts over the community variants' names).
//!
//! A family is the ids it serves and the predicate its lookup claims ids
//! with. Our shared string table (`layout_glue`'s `ui-strings` source)
//! claims a whole middle-byte family (`layout_glue::is_ours`) and answers
//! exact ids from one table, so its members (layouts, the ui model, the
//! Live menu rows, the skulls pages, the campaign picker) are also checked
//! for no id twice.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{
    campaign_menu, community_variants, coop_live, custom_games, layout_glue, skulls_menu,
    variant_options, video_screen,
};

struct Family {
    name: &'static str,
    ids: Vec<u32>,
    claims: fn(u32) -> bool,
}

/// The shared table's members, each with its ids.
fn table_members() -> Vec<(&'static str, Vec<u32>)> {
    let ids = |v: Vec<(u32, &str)>| v.into_iter().map(|(i, _)| i).collect::<Vec<_>>();
    let store = Arc::new(settings::Store::defaults(crate::setting_defs::ALL));
    let model = video_screen::model(store, video_screen::Switches::ALL).unwrap();
    vec![
        (
            "layouts and speaker",
            ids(layout_glue::served_strings(true)),
        ),
        (
            "ui model",
            model.strings().iter().map(|(i, _)| *i).collect(),
        ),
        ("live menu rows", ids(custom_games::strings())),
        ("skulls pages", ids(skulls_menu::strings())),
        (
            "variant page",
            crate::variant_page::strings()
                .into_iter()
                .map(|(i, _)| i)
                .collect(),
        ),
        (
            "campaign picker",
            (0..=campaign_menu::MAX_ROWS as u32)
                .map(|i| campaign_menu::ID_BASE + i)
                .collect(),
        ),
    ]
}

fn families() -> Vec<Family> {
    let community = (0..community_variants::VARIANTS.len())
        .flat_map(|i| {
            [
                community_variants::name_id(i),
                community_variants::description_id(i),
            ]
        })
        .collect();
    vec![
        Family {
            name: "our string table",
            ids: table_members().into_iter().flat_map(|(_, v)| v).collect(),
            claims: layout_glue::is_ours,
        },
        Family {
            name: "community variants",
            ids: community,
            claims: |id| community_variants::is_ours(id) || community_variants::text(id).is_some(),
        },
        Family {
            name: "variant options",
            ids: (0..variants::extra::OPTIONS.len())
                .map(variant_options::label_id)
                .collect(),
            claims: |id| variant_options::text(id).is_some(),
        },
        Family {
            name: "co-op texts",
            ids: (0..coop_live::TEXT_SLOTS)
                .map(coop_live::Texts::id)
                .collect(),
            claims: coop_live::Texts::owns,
        },
    ]
}

#[test]
fn no_family_claims_another_familys_ids() {
    let all = families();
    for f in &all {
        assert!(!f.ids.is_empty(), "{} has ids", f.name);
        for id in &f.ids {
            assert!((f.claims)(*id), "{} does not claim its own {id:#x}", f.name);
        }
        for other in all.iter().filter(|o| o.name != f.name) {
            for id in &f.ids {
                assert!(
                    !(other.claims)(*id),
                    "{id:#x} of the {} is claimed by the {}",
                    f.name,
                    other.name
                );
            }
        }
    }
}

#[test]
fn the_shared_table_has_no_id_twice() {
    let mut seen: BTreeMap<u32, &str> = BTreeMap::new();
    for (who, ids) in table_members() {
        for id in ids {
            if let Some(first) = seen.insert(id, who) {
                panic!("{id:#x} is both the {first}'s and the {who}'s");
            }
        }
    }
}

#[test]
fn the_old_co_op_base_is_no_longer_swallowed_by_the_community_variants() {
    // Their claim was on the low 24 bits: 0x20ffe000 read as "Zombies".
    for id in [0x20ff_e000, 0x21ff_e001, 0xffff_e07f] {
        assert!(!community_variants::is_ours(id), "{id:#x}");
        assert_eq!(community_variants::text(id), None, "{id:#x}");
    }
    assert!(community_variants::is_ours(0x00ff_e000));
}

/// Every id a module serves is inside the registry family named for it
/// (`crate::ui_kit::ids`), so the registry's compile-time check covers it.
#[test]
fn every_served_id_is_in_its_registered_family() {
    use crate::ui_kit::ids::family;
    let ids = |v: Vec<(u32, &str)>| v.into_iter().map(|(i, _)| i).collect::<Vec<_>>();
    let store = Arc::new(settings::Store::defaults(crate::setting_defs::ALL));
    let model = video_screen::model(store, video_screen::Switches::ALL).unwrap();
    let skulls = skulls_menu::strings()
        .into_iter()
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    let checks: Vec<(&str, Vec<u32>)> = vec![
        ("layouts", ids(layout_glue::strings())),
        (
            "speaker",
            vec![
                crate::speaker_row::SMALL_ON_ID,
                crate::speaker_row::SMALL_OFF_ID,
            ],
        ),
        ("live menu rows", ids(custom_games::strings())),
        ("ui model", model.strings().iter().map(|(i, _)| *i).collect()),
        (
            "campaign picker",
            (0..=campaign_menu::MAX_ROWS as u32)
                .map(|i| campaign_menu::ID_BASE + i)
                .collect(),
        ),
        (
            "co-op texts",
            (0..coop_live::TEXT_SLOTS)
                .map(coop_live::Texts::id)
                .collect(),
        ),
        (
            "community variants",
            (0..community_variants::VARIANTS.len())
                .flat_map(|i| {
                    [
                        community_variants::name_id(i),
                        community_variants::description_id(i),
                    ]
                })
                .collect(),
        ),
        (
            "signin",
            std::iter::once(crate::signin_screen::TITLE_ID)
                .chain(crate::signin_screen::TEXT_IDS)
                .collect(),
        ),
    ];
    for (name, ids) in checks {
        let f = family(name);
        for id in ids {
            assert!(f.owns(id), "{id:#x} is outside the {name} family");
        }
    }
    for id in skulls {
        assert!(
            family("skulls").owns(id) || family("skull row labels").owns(id),
            "{id:#x} of the skulls pages is outside both skulls families"
        );
    }
}
