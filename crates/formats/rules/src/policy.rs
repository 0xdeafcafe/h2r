//! Which scripts a kind of game takes, by the capabilities their headers
//! declare (docs/game/rules-engine.md, "Roles, powers and capabilities").
//!
//! One rule for every party that reads a header: Live when a hopper entry
//! or a ranked session names a script, Live's publish routes when they
//! record where a variant may play, and the host at the game's start.
//!
//! - **Custom** games take any capability this build knows.
//! - **Hopper** games take any but the hopper's deny list; `forge.*` in
//!   the list denies the family.
//! - **Ranked** games never take a `forge.*` capability: Live's allow-list
//!   never lists a script that lets players edit the match.
//! - **Theater** replays what a film recorded, so it refuses no script;
//!   it takes no edit ([`edits_allowed`]).
//!
//! A capability this build does not know is refused everywhere.

use crate::caps;

/// A kind of game, as the policy reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context<'a> {
    Custom,
    /// A matchmaking hopper that does not count for ranks, with its deny
    /// list of capability names.
    Hopper { deny: &'a [String] },
    Ranked,
    /// A film played back.
    Theater,
}

impl Context<'_> {
    /// The context's name, as eligibility lists and refusals spell it.
    pub fn name(&self) -> &'static str {
        match self {
            Context::Custom => "custom",
            Context::Hopper { .. } => "hopper",
            Context::Ranked => "ranked",
            Context::Theater => "theater",
        }
    }
}

/// The contexts [`eligible`] answers for, in order. A hopper here has no
/// deny list: a hopper keeps none yet.
pub const PLAYED: [Context<'static>; 3] = [Context::Custom, Context::Hopper { deny: &[] }, Context::Ranked];

/// Why one declared capability is refused in a context.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("capability {capability} is not allowed in a {context} game: {reason}")]
pub struct Denial {
    pub capability: String,
    pub context: &'static str,
    pub reason: &'static str,
}

/// Whether a deny-list entry names `capability`: the name itself, or a
/// family (`forge.*`) it belongs to.
pub fn denies(entry: &str, capability: &str) -> bool {
    match entry.strip_suffix(".*") {
        Some(family) => capability
            .strip_prefix(family)
            .is_some_and(|rest| rest.starts_with('.')),
        None => entry == capability,
    }
}

/// Why `capability` is refused in `context` under API version `api`, or
/// `None` when it is allowed.
pub fn refusal(context: Context<'_>, capability: &str, api: u16) -> Option<&'static str> {
    let Some(cap) = caps::capability(capability, api) else {
        return Some("this build does not know the capability");
    };
    match context {
        Context::Custom | Context::Theater => None,
        Context::Hopper { deny } => deny
            .iter()
            .any(|d| denies(d, cap.name))
            .then_some("the hopper's deny list names it"),
        Context::Ranked => cap
            .grantable
            .then_some("ranked games never take edits, so Live's allow-list never lists a script that grants them"),
    }
}

/// Every declared capability `context` refuses, in declaration order, each
/// with its reason. Empty: the context takes the script.
pub fn check(context: Context<'_>, requires: &[String], api: u16) -> Vec<Denial> {
    requires
        .iter()
        .filter_map(|name| {
            refusal(context, name, api).map(|reason| Denial {
                capability: name.clone(),
                context: context.name(),
                reason,
            })
        })
        .collect()
}

/// The contexts of [`PLAYED`] that take a script declaring `requires`, by
/// name: what a published variant records as where it may play.
pub fn eligible(requires: &[String], api: u16) -> Vec<&'static str> {
    PLAYED
        .iter()
        .filter(|c| check(**c, requires, api).is_empty())
        .map(|c| c.name())
        .collect()
}

/// Whether a granted power may be used: in custom and hopper games, never
/// in ranked ones or in Theater.
pub fn edits_allowed(context: Context<'_>) -> bool {
    matches!(context, Context::Custom | Context::Hopper { .. })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::API_VERSION;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn custom_takes_every_known_capability() {
        let all: Vec<String> = caps::CAPABILITIES.iter().map(|c| c.name.to_string()).collect();
        assert!(check(Context::Custom, &all, API_VERSION).is_empty());
        assert!(check(Context::Theater, &all, API_VERSION).is_empty());
    }

    #[test]
    fn ranked_refuses_each_forge_capability_by_name() {
        let req = names(&["hud.message", "forge.move", "player.team", "forge.place"]);
        let d = check(Context::Ranked, &req, API_VERSION);
        let refused: Vec<&str> = d.iter().map(|d| d.capability.as_str()).collect();
        assert_eq!(refused, ["forge.move", "forge.place"]);
        assert!(d.iter().all(|d| d.context == "ranked"));
        assert!(d[0].to_string().starts_with("capability forge.move is not allowed in a ranked game"));
    }

    #[test]
    fn a_hopper_refuses_its_deny_list_and_families() {
        let deny = names(&["forge.*", "ai.squads"]);
        let req = names(&["forge.scale", "ai.squads", "objects.create"]);
        let d = check(Context::Hopper { deny: &deny }, &req, API_VERSION);
        let refused: Vec<&str> = d.iter().map(|d| d.capability.as_str()).collect();
        assert_eq!(refused, ["forge.scale", "ai.squads"]);
        assert!(check(Context::Hopper { deny: &[] }, &req, API_VERSION).is_empty());
        assert!(denies("forge.*", "forge.move"));
        assert!(!denies("forge.*", "forgery.move"));
        assert!(!denies("forge.move", "forge.place"));
    }

    #[test]
    fn an_unknown_capability_is_refused_everywhere() {
        let req = names(&["net.socket"]);
        for c in [Context::Custom, Context::Hopper { deny: &[] }, Context::Ranked, Context::Theater] {
            assert_eq!(check(c, &req, API_VERSION).len(), 1, "{}", c.name());
        }
        assert!(eligible(&req, API_VERSION).is_empty());
    }

    #[test]
    fn eligibility_lists_where_a_script_may_play() {
        assert_eq!(eligible(&[], API_VERSION), ["custom", "hopper", "ranked"]);
        assert_eq!(eligible(&names(&["forge.delete"]), API_VERSION), ["custom", "hopper"]);
        assert_eq!(eligible(&names(&["player.team"]), API_VERSION), ["custom", "hopper", "ranked"]);
    }

    #[test]
    fn edits_run_in_custom_and_hopper_games_only() {
        assert!(edits_allowed(Context::Custom));
        assert!(edits_allowed(Context::Hopper { deny: &[] }));
        assert!(!edits_allowed(Context::Ranked));
        assert!(!edits_allowed(Context::Theater));
    }
}
