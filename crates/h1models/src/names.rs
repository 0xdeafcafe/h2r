//! Halo 1 animation names -> Halo 2 mode / class / type / action.
//!
//! Halo 1 names are space-separated labels, `<unit> <weapon> [<type>]
//! <action>` (for example `stand rifle move-front`, `crouch pistol idle`,
//! `W-driver enter`). Halo 2 looks animations up through a tree of string IDs:
//! mode, weapon class, weapon type, action. This module splits a Halo 1 name
//! into that tree and renames the labels whose Halo 2 counterpart differs.
//! The table is in `docs/formats/h1-models.md`, "Animation names"; entries
//! marked there as unverified need checking against retail `jmad` names.

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

/// Unit (mode) labels whose Halo 2 name differs.
pub const MODES: &[(&str, &str)] = &[
    ("stand", "combat"),
    ("crouch", "crouch"),
    ("alert", "combat"),
    ("W-driver", "warthog_d"),
    ("W-gunner", "warthog_g"),
    ("W-passenger", "warthog_p"),
    ("G-driver", "ghost_d"),
    ("B-driver", "banshee_d"),
    ("scorpion-driver", "scorpion_d"),
];

/// Weapon class labels that exist under the same name in Halo 2; others are
/// kept verbatim.
pub const CLASSES: &[&str] = &["unarmed", "pistol", "rifle", "missile", "fixed"];

/// Action labels whose Halo 2 name differs beyond `-` -> `_`.
pub const ACTIONS: &[(&str, &str)] = &[
    ("reload", "reload_1"),
    ("melee", "melee_strike_1"),
    ("throw-grenade", "throw_grenade"),
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
    fn splits_and_renames() {
        assert_eq!(
            map_name("stand rifle move-front").full(),
            "combat:rifle:move_front"
        );
        assert_eq!(map_name("crouch pistol idle").full(), "crouch:pistol:idle");
        assert_eq!(map_name("W-driver enter").full(), "warthog_d:enter");
        assert_eq!(
            map_name("stand rifle ar reload").full(),
            "combat:rifle:ar:reload_1"
        );
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
    }
}
