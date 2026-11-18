//! Halo 1 animation names -> Halo 2 mode / class / type / action.
//!
//! Halo 1 names are space-separated labels, `<unit> <weapon> [<type>]
//! <action>` (for example `stand rifle move-front`, `crouch pistol idle`,
//! `W-driver enter`). Halo 2 looks animations up through a tree of string IDs:
//! mode, weapon class, weapon type, action. This module splits a Halo 1 name
//! into that tree and renames the labels whose Halo 2 counterpart differs.
//! The table is in `docs/halo1/models.md`, "Animation names"; every
//! target below was checked against the `jmad` trees of every map on the
//! owner's retail disc by the ignored test `retail_h2_animation_names`
//! (evidence V3 there).

/// A Halo 2 animation path.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct H2Name {
    pub mode: String,
    pub class: String,
    pub kind: String,
    pub action: String,
}

impl H2Name {
    /// The animation's own name, `mode:class:type:action` with `any`
    /// segments left out (`combat:rifle:move_front`).
    pub fn full(&self) -> String {
        // First-person weapon animations: tree mode `any`, record name
        // `first_person:<action>` (V3: every retail first-person graph).
        if self.mode == FIRST_PERSON_MODE && self.class == "any" && self.kind == "any" {
            return format!("first_person:{}", self.action);
        }
        let mut parts = vec![self.mode.as_str()];
        if self.class != "any" || self.kind != "any" {
            parts.push(&self.class);
        }
        if self.kind != "any" {
            parts.push(&self.kind);
        }
        parts.push(&self.action);
        parts.join(":")
    }
}

/// Unit (mode) labels whose Halo 2 name differs. Every target is a mode of
/// the retail masterchief and/or elite biped graphs (seat animations live in
/// the biped's own graph, not the vehicle's).
pub const MODES: &[(&str, &str)] = &[
    ("stand", "combat"),
    ("crouch", "crouch"),
    // No alert mode exists in retail; alert poses join stand's variants.
    ("alert", "combat"),
    ("W-driver", "warthog_d"),
    ("W-gunner", "warthog_g"),
    ("W-passenger", "warthog_p"),
    ("G-driver", "ghost_d"),
    ("B-driver", "banshee_d"),
    ("scorpion-driver", "scorpion_d"),
    // Seats verified against the same retail graphs. `c_turret_ap` is the
    // retail covenant turret (its jmad is on every multiplayer map);
    // `guntower_d` appears on the 03a elite; the pelican's retail passenger
    // modes are numbered per side (01 = front row assumed); the scorpion's
    // are `_p_lf`/`_p_lb`/`_p_rf`/`_p_rb`.
    ("CT-driver", "c_turret_ap_d"),
    ("GT-gunner", "guntower_d"),
    ("P-riderLF", "pelican_p_l01"),
    ("P-riderRF", "pelican_p_r01"),
    ("scorpionLF", "scorpion_p_lf"),
];

/// Halo 1's first-person label (`first-person fire-1`, the first word of
/// every animation of a weapon's first-person graph) and the tree mode it
/// becomes: retail first-person graphs (`characters\\masterchief\\fp\\...`)
/// hold one mode, `any`, with class `any` and type `any` (and `dual`
/// classes Halo 1 has no animations for).
pub const FIRST_PERSON: &str = "first-person";
pub const FIRST_PERSON_MODE: &str = "any";

/// First-person action labels whose retail name differs beyond `-` -> `_`:
/// retail first-person melee is `melee_strike_1` (`_2`, `_3` variants).
pub const FIRST_PERSON_ACTIONS: &[(&str, &str)] = &[("melee", "melee_strike_1")];

/// Weapon class labels that exist under the same name in Halo 2; others are
/// kept verbatim. All five are classes of the retail biped graphs
/// (`warthog_g:fixed:idle` covers `fixed`).
pub const CLASSES: &[&str] = &["unarmed", "pistol", "rifle", "missile", "fixed"];

/// Action labels whose Halo 2 name differs beyond `-` -> `_`.
pub const ACTIONS: &[(&str, &str)] = &[
    // Retail has no `reload_1`, and no third-person reload at all: the
    // reload actions (`reload_full`, `reload_empty`, ...) belong to the
    // first-person weapon graphs. `reload_full` is the full-magazine one.
    // Halo 1 spells the action both `reload` and `reload-1`.
    ("reload", "reload_full"),
    ("reload-1", "reload_full"),
    ("throw-grenade", "throw_grenade"),
    // `melee` maps to itself — retail `combat:rifle:any:melee`, on the
    // elite; `melee_strike_1..3` are first-person-only — so no rename is
    // listed for it.
];

fn action_name(s: &str) -> String {
    ACTIONS
        .iter()
        .find(|(h1, _)| *h1 == s)
        .map(|(_, h2)| (*h2).to_owned())
        .unwrap_or_else(|| s.replace('-', "_").to_ascii_lowercase())
}

/// Splits and renames a Halo 1 animation name. A `%n` suffix (a random
/// variant) becomes `:var<n>` on the action.
pub fn map_name(h1: &str) -> H2Name {
    let (body, variant) = match h1.rsplit_once('%') {
        Some((b, v)) if v.chars().all(|c| c.is_ascii_digit()) => (b, Some(v)),
        _ => (h1, None),
    };
    let words: Vec<&str> = body.split_whitespace().collect();
    let mode_word = words.first().copied().unwrap_or("");
    if mode_word == FIRST_PERSON && words.len() == 2 {
        let a = words[1];
        let action = FIRST_PERSON_ACTIONS
            .iter()
            .find(|(h1, _)| *h1 == a)
            .map_or_else(|| action_name(a), |(_, h2)| (*h2).to_owned());
        return H2Name {
            mode: FIRST_PERSON_MODE.into(),
            class: "any".into(),
            kind: "any".into(),
            action: match variant {
                Some(v) => format!("{action}:var{v}"),
                None => action,
            },
        };
    }
    let mode = MODES
        .iter()
        .find(|(a, _)| *a == mode_word)
        .map(|(_, b)| (*b).to_owned())
        .unwrap_or_else(|| mode_word.replace('-', "_").to_ascii_lowercase());
    let rest = &words[words.len().min(1)..];
    let (class, kind, action) = match rest {
        [] => ("any".to_owned(), "any".to_owned(), "idle".to_owned()),
        [a] => ("any".to_owned(), "any".to_owned(), action_name(a)),
        [c, a] if CLASSES.contains(c) => (c.to_string(), "any".to_owned(), action_name(a)),
        [c, t, a] if CLASSES.contains(c) => (c.to_string(), t.to_string(), action_name(a)),
        // Anything else (damage, vehicle overlays): one action of all words.
        words => (
            "any".to_owned(),
            "any".to_owned(),
            words
                .iter()
                .map(|w| action_name(w))
                .collect::<Vec<_>>()
                .join("_"),
        ),
    };
    let action = match variant {
        Some(v) => format!("{action}:var{v}"),
        None => action,
    };
    H2Name {
        mode,
        class,
        kind,
        action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_person_names_take_mode_any() {
        let n = map_name("first-person fire-1");
        assert_eq!(
            (
                n.mode.as_str(),
                n.class.as_str(),
                n.kind.as_str(),
                n.action.as_str()
            ),
            ("any", "any", "any", "fire_1")
        );
        assert_eq!(n.full(), "first_person:fire_1");
        assert_eq!(map_name("first-person melee").action, "melee_strike_1");
        assert_eq!(map_name("first-person reload-empty").action, "reload_empty");
        assert_eq!(
            map_name("first-person idle%1").full(),
            "first_person:idle:var1"
        );
    }

    #[test]
    fn splits_and_renames() {
        assert_eq!(
            map_name("stand rifle move-front").full(),
            "combat:rifle:move_front"
        );
        assert_eq!(map_name("crouch pistol idle").full(), "crouch:pistol:idle");
        assert_eq!(map_name("W-driver enter").full(), "warthog_d:enter");
        assert_eq!(
            map_name("stand rifle ar reload-1").full(),
            "combat:rifle:ar:reload_full"
        );
        assert_eq!(
            map_name("stand missile reload").full(),
            "combat:missile:reload_full"
        );
        // Third-person melee keeps its name in Halo 2.
        assert_eq!(map_name("stand rifle melee").full(), "combat:rifle:melee");
        assert_eq!(
            map_name("alert unarmed idle%2").full(),
            "combat:unarmed:idle:var2"
        );
        assert_eq!(map_name("h-kill back gut").full(), "h_kill:back_gut");
        assert_eq!(
            map_name("suspension left back").full(),
            "suspension:left_back"
        );
        assert_eq!(map_name("firing").full(), "firing:idle");
        // The banshee's doors already carry their retail names.
        assert_eq!(map_name("stand opening").full(), "combat:opening");
        // Seats mapped to their retail modes.
        assert_eq!(
            map_name("CT-driver unarmed idle").full(),
            "c_turret_ap_d:unarmed:idle"
        );
        assert_eq!(map_name("GT-gunner enter").full(), "guntower_d:enter");
        assert_eq!(map_name("P-riderLF enter").full(), "pelican_p_l01:enter");
        assert_eq!(map_name("P-riderRF exit").full(), "pelican_p_r01:exit");
        assert_eq!(
            map_name("scorpionLF rifle idle").full(),
            "scorpion_p_lf:rifle:idle"
        );
    }

    // ------------------------------------------------------------------
    // Retail verification (2026-11-17). Every map on the owner's disc was
    // scanned; these are the facts the tables above were checked against.
    // ------------------------------------------------------------------

    /// `mode:class:type:action` paths, per graph kind, that the mapping is
    /// asserted to reproduce. All are leaf paths of retail graphs: the
    /// masterchief, elite and dervish bipeds (their first-person weapon
    /// graphs included) and the vehicle graphs.
    const RETAIL_PATHS: &[(&str, &str)] = &[
        // Biped movement, idle and stance vocabulary.
        ("biped", "combat:rifle:any:idle"),
        ("biped", "combat:unarmed:any:idle"),
        ("biped", "combat:pistol:any:idle"),
        ("biped", "combat:missile:any:idle"),
        ("biped", "crouch:rifle:any:idle"),
        ("biped", "combat:rifle:any:move_front"),
        ("biped", "combat:rifle:any:turn_left"),
        ("biped", "crouch:rifle:any:move_front"),
        ("biped", "combat:rifle:any:melee"),
        ("biped", "combat:rifle:any:stunned_front"),
        ("biped", "combat:pistol:any:stunned_back"),
        ("biped", "combat:rifle:any:ready"),
        ("biped", "combat:any:any:airborne_dead"),
        ("biped", "combat:any:any:landing_dead"),
        ("biped", "combat:sword:any:throw_grenade"),
        // Actions the table renames to, present on the bipeds' first-person
        // weapon graphs (mode `any`, animation names `first_person:*`).
        ("biped", "any:any:any:reload_full"),
        ("biped", "any:any:any:melee_strike_1"),
        ("biped", "any:any:any:fire_1"),
        // Vehicle seats the table maps to.
        ("biped", "warthog_d:any:any:enter"),
        ("biped", "warthog_g:fixed:any:idle"),
        ("biped", "warthog_p:any:any:enter"),
        ("biped", "ghost_d:any:any:enter"),
        ("biped", "banshee_d:any:any:enter"),
        ("biped", "scorpion_d:any:any:enter"),
        ("biped", "scorpion_p_lf:any:any:enter"),
        ("biped", "pelican_p_l01:any:any:enter"),
        ("biped", "pelican_p_r01:any:any:enter"),
        ("biped", "c_turret_ap_d:any:any:enter"),
        ("biped", "guntower_d:any:any:enter"),
        // The only actions any retail vehicle graph carries.
        ("vehicle", "combat:any:any:opening"),
        ("vehicle", "combat:any:any:closing"),
    ];

    /// Names the mapping deliberately does not use, with the fact their
    /// absence establishes: `(kind, level, name)` where level is `mode` or
    /// `action`.
    const RETAIL_ABSENT: &[(&str, &str, &str)] = &[
        // Halo 1 reload animations must not become `reload_1`.
        ("biped", "action", "reload_1"),
        // Unit damage has no Halo 2 tree names at all.
        ("biped", "mode", "h_kill"),
        ("biped", "mode", "s_kill"),
        ("biped", "mode", "s_ping"),
        // Vehicle overlays are driven outside the jmad tree in Halo 2.
        ("vehicle", "mode", "suspension"),
        ("vehicle", "mode", "vehicle"),
        ("vehicle", "action", "steering"),
        ("vehicle", "action", "ground_speed"),
    ];

    /// `biped` for `objects\characters\...` graphs, `vehicle` for
    /// `objects\vehicles\...`, else `other`.
    fn kind_of(path: &str) -> &'static str {
        if path.contains("\\characters\\") {
            "biped"
        } else if path.contains("\\vehicles\\") {
            "vehicle"
        } else {
            "other"
        }
    }

    /// Checks every name the tables above produce against the `jmad` trees
    /// of every map on the retail disc: each `MODES` target must be a mode
    /// of some biped graph, each `CLASSES` target a class of one, each
    /// `ACTIONS` target an action of one, and every path in [`RETAIL_PATHS`]
    /// must exist for its kind — while [`RETAIL_ABSENT`] must not.
    ///
    /// `H2_DISC=disc/halo2.iso cargo test -p h1models --release -- --ignored retail_h2 --nocapture`
    #[test]
    #[ignore = "needs $H2_DISC"]
    fn retail_h2_animation_names() {
        use std::collections::BTreeSet;
        use std::io::BufReader;
        let Some(disc) = std::env::var_os("H2_DISC") else {
            return;
        };
        let mut iso =
            xiso::Xiso::open(BufReader::new(std::fs::File::open(&disc).unwrap())).unwrap();
        let maps: Vec<_> = iso
            .entries()
            .unwrap()
            .into_iter()
            .filter(|e| !e.is_dir && e.path.starts_with("/maps/") && e.path.ends_with(".map"))
            .collect();
        // "kind:mode", "kind:class", ... and full "kind:mode:class:type:action".
        let mut names: [BTreeSet<String>; 5] = Default::default();
        let mut graphs = 0usize;
        for e in &maps {
            let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
            let mut map = map::Map::from_region(
                BufReader::new(std::fs::File::open(&disc).unwrap()),
                base,
                u64::from(e.size),
            )
            .unwrap();
            let strings = map.strings().to_vec();
            let resolve = |id: tags::wire::StringId| {
                id.resolve(&strings)
                    .ok()
                    .flatten()
                    .map(str::to_owned)
                    .unwrap_or_default()
            };
            let jmads: Vec<_> = map
                .tags()
                .iter()
                .filter(|t| t.group.0 == *b"jmad" && t.location.is_some())
                .map(|t| (t.id, t.name.clone().unwrap_or_default()))
                .collect();
            for (id, path) in jmads {
                let Ok(g) = tags::anim::AnimationGraph::read(&mut map, id) else {
                    continue;
                };
                graphs += 1;
                let kind = kind_of(&path);
                for m in &g.modes {
                    for c in &m.classes {
                        for ty in &c.types {
                            for a in &ty.actions {
                                let parts = [
                                    resolve(m.name),
                                    resolve(c.name),
                                    resolve(ty.name),
                                    resolve(a.name),
                                ];
                                names[0].insert(format!("{kind}:{}", parts[0]));
                                names[1].insert(format!("{kind}:{}", parts[1]));
                                names[2].insert(format!("{kind}:{}", parts[2]));
                                names[3].insert(format!("{kind}:{}", parts[3]));
                                names[4].insert(format!("{kind}:{}", parts.join(":")));
                            }
                        }
                    }
                }
            }
        }
        assert!(graphs > 100, "no animation graphs found on the disc");
        for (_, target) in MODES {
            assert!(
                names[0].contains(&format!("biped:{target}")),
                "mode {target} is not a retail biped mode"
            );
        }
        for class in CLASSES {
            assert!(
                names[1].contains(&format!("biped:{class}")),
                "class {class} is not a retail biped class"
            );
        }
        for (_, target) in ACTIONS {
            assert!(
                names[3].contains(&format!("biped:{target}")),
                "action {target} is not a retail biped action"
            );
        }
        for (kind, path) in RETAIL_PATHS {
            assert!(
                names[4].contains(&format!("{kind}:{path}")),
                "{path} is not a retail {kind} path"
            );
        }
        for (kind, level, name) in RETAIL_ABSENT {
            let level = match *level {
                "mode" => &names[0],
                _ => &names[3],
            };
            assert!(
                !level.contains(&format!("{kind}:{name}")),
                "{name} unexpectedly exists as a retail {kind} name"
            );
        }
        println!(
            "{} maps, {graphs} graphs: every table target verified ({} modes, {} classes, {} actions, {} paths), {} names confirmed absent",
            maps.len(),
            MODES.len(),
            CLASSES.len(),
            ACTIONS.len(),
            RETAIL_PATHS.len(),
            RETAIL_ABSENT.len()
        );
    }
}
