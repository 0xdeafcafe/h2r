//! Resting poses, locomotion and the dialogue overlay are indexed by slot
//! in an H1 unit graph. A seated marine can borrow another class's clip,
//! and the first idle can itself have a permutation suffix.
use super::*;

const LOCOMOTION: &[(usize, &[&str])] = &[
    (0, &["idle"]),
    (2, &["turn_left"]),
    (3, &["turn_right"]),
    (8, &["move_front", "walk_front"]),
    (9, &["move_back", "walk_back"]),
    (10, &["move_left", "walk_left"]),
    (11, &["move_right", "walk_right"]),
];

fn slot(g: &Graph, slots: &[i16], k: usize, kind: AnimType) -> Result<Option<usize>> {
    let value = slots.get(k).copied().unwrap_or(-1);
    if value == -1 {
        return Ok(None);
    }
    let i = value as usize;
    match g.animations.get(i) {
        Some(a) if a.kind == kind => Ok(Some(i)),
        _ => Err(Error::Invalid(format!(
            "biped slot {k} references invalid {kind:?} clip {value}"
        ))),
    }
}

fn put(entries: &mut Vec<(String, usize)>, key: &str, clip: usize) {
    entries.retain(|(name, _)| name != key);
    entries.push((key.into(), clip));
}

fn actions<'a>(tree: &'a mut Actions, mode: &str, class: &str) -> &'a mut Vec<(String, usize)> {
    typed_actions(tree, mode, class, "any")
}

fn typed_actions<'a>(
    tree: &'a mut Actions,
    mode: &str,
    class: &str,
    kind: &str,
) -> &'a mut Vec<(String, usize)> {
    tree.entry(mode.into())
        .or_default()
        .entry(class.into())
        .or_default()
        .entry(kind.into())
        .or_default()
}

pub(super) fn bindings(
    g: &Graph,
    tree: &mut Actions,
    overlays: &mut Overlays,
    weapon_classes: &[(String, String)],
) -> Result<()> {
    let mut standing = None;
    let mut standing_talk = None;
    for u in &g.units {
        let mode = crate::names::unit_mode(&u.label);
        // A bare-handed unit requests the wildcard class. Prefer the
        // unarmed pose, then the first authored class with a resting pose
        // (e.g. the Elite's pistol or the cinematic Chief's rifle).
        let default = u
            .classes
            .iter()
            .enumerate()
            .filter(|(_, c)| c.slots.first().is_some_and(|&i| i >= 0))
            .min_by_key(|(_, c)| c.label != "unarmed")
            .map(|(index, _)| index);
        for (index, c) in u.classes.iter().enumerate() {
            let is_default = Some(index) == default;
            // The rebuilt weapon carries the cyborg's class. The same
            // Needler uses a rifle pose on the marine and a pistol pose on
            // the cyborg. Bind the exact type before H2 tries class/any.
            let aliases: Vec<_> = c
                .types
                .iter()
                .filter_map(|(kind, _)| {
                    weapon_classes
                        .iter()
                        .find(|(ty, class)| ty == kind && class != &c.label)
                })
                .collect();
            for &(k, names) in LOCOMOTION {
                let Some(i) = slot(g, &c.slots, k, AnimType::Base)? else {
                    continue;
                };
                for name in names {
                    put(actions(tree, &mode, &c.label), name, i);
                    for (kind, held_class) in &aliases {
                        put(typed_actions(tree, &mode, held_class, kind), name, i);
                    }
                    if is_default {
                        put(actions(tree, &mode, "any"), name, i);
                        if u.label == "stand" {
                            put(actions(tree, "any", "any"), name, i);
                        }
                    }
                }
                if is_default && u.label == "stand" && k == 0 {
                    standing = Some(i);
                }
            }
        }
        if let Some(i) = slot(g, &u.slots, 10, AnimType::Overlay)? {
            actions(tree, &mode, "any").retain(|(key, _)| key != "talk");
            put(
                overlays
                    .entry((mode, "any".into(), "any".into()))
                    .or_default(),
                "talk",
                i,
            );
            if u.label == "stand" {
                standing_talk = Some(i);
            }
        }
    }
    // The x30 pilot has no filled idle slot. Its explicit standalone
    // base "idle" is also the pose to use while no cinematic is playing.
    let idle = match standing {
        Some(i) => i,
        None => {
            let mut clips = g
                .animations
                .iter()
                .enumerate()
                .filter(|(_, a)| a.kind == AnimType::Base && a.name == "idle");
            match (clips.next(), clips.next()) {
                (Some((i, _)), None) => i,
                _ => {
                    return Err(Error::Invalid(
                        "biped has no unambiguous authored standing idle".into(),
                    ));
                }
            }
        }
    };
    for mode in ["combat", "patrol", "any"] {
        let entries = actions(tree, mode, "any");
        if !entries.iter().any(|(key, _)| key == "idle") {
            put(entries, "idle", idle);
        }
    }
    // Script-only actors may have no directional cycle. Keep their own
    // resting pose on the base channel if the engine asks them to move.
    let global = actions(tree, "any", "any");
    for &(_, keys) in LOCOMOTION {
        for &key in keys {
            if !global.iter().any(|(name, _)| name == key) {
                put(global, key, idle);
            }
        }
    }
    if let Some(i) = standing_talk {
        put(
            overlays
                .entry(("any".into(), "any".into(), "any".into()))
                .or_default(),
            "talk",
            i,
        );
    }
    Ok(())
}
