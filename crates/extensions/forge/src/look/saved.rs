//! A look in the profile's settings: `[profiles."<name>".forge]` in
//! `settings.toml` (docs/game/forge-monitor-appearance.md, "Where it is
//! saved").
//!
//! Five of the eight keys are numbers and are [`settings::Def`]s, so the
//! store checks their bounds and reports a bad one by key. The model, its
//! source map and the variant are names; a `Def` holds only a bool or a
//! whole number, so they are read and written as text
//! ([`settings::Store::profile_text`]). Nothing here rewrites a key it did
//! not set: a model this install cannot draw stays in the file.
use settings::{Apply, Choice, Def, Kind, Scope, Store, Tier, Unit, Value};

use super::census::Entry;
use super::{KEY_MAX, MARKER, MonitorLook, PALETTE, VARIANT_MAX, name_ok};

pub const MODEL: &str = "forge.monitor_model";
pub const SOURCE: &str = "forge.monitor_source";
pub const VARIANT: &str = "forge.monitor_variant";
pub const PRIMARY: &str = "forge.monitor_primary";
pub const SECONDARY: &str = "forge.monitor_secondary";
pub const TINT: &str = "forge.monitor_tint";
pub const LIGHT: &str = "forge.monitor_light";
pub const LIGHT_COLOUR: &str = "forge.monitor_light_colour";

/// The three name keys, read and written as text.
pub const TEXT_KEYS: [&str; 3] = [MODEL, SOURCE, VARIANT];

const TINTS: &[Choice] = &[
    Choice {
        value: 0,
        name: "none",
        label: "None",
    },
    Choice {
        value: 1,
        name: "light",
        label: "Light",
    },
    Choice {
        value: 2,
        name: "medium",
        label: "Medium",
    },
    Choice {
        value: 3,
        name: "strong",
        label: "Strong",
    },
];
const LIGHTS: &[Choice] = &[
    Choice {
        value: 0,
        name: "off",
        label: "Off",
    },
    Choice {
        value: 1,
        name: "dim",
        label: "Dim",
    },
    Choice {
        value: 2,
        name: "bright",
        label: "Bright",
    },
];

const fn number(key: &'static str, label: &'static str, kind: Kind) -> Def {
    Def {
        key,
        label,
        help: "A part of this profile's Forge monitor look. Kept per profile under \
               [profiles.\"<name>\".forge]; presentation only.",
        kind,
        default: Value::Int(0),
        env: None,
        flag: None,
        aliases: &[],
        parse_env: None,
        apply: Apply::Live,
        scope: Scope::View,
        tier: Tier::User,
        menu: None,
    }
}

const fn colour(key: &'static str, label: &'static str) -> Def {
    number(
        key,
        label,
        Kind::Int {
            valid: 0..=(PALETTE as i64 - 1),
            steps: &[],
            unit: Unit::Plain,
        },
    )
}

/// The five numeric keys. A profile with none of them takes the seat's
/// colour for each colour and no tint or light ([`load`]).
pub static DEFS: [Def; 5] = [
    colour(PRIMARY, "Primary colour"),
    colour(SECONDARY, "Secondary colour"),
    number(TINT, "Tint", Kind::Choice(TINTS)),
    number(LIGHT, "Light", Kind::Choice(LIGHTS)),
    colour(LIGHT_COLOUR, "Light colour"),
];

/// A look and the source map it was chosen from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub look: MonitorLook,
    /// The source map's SHA-256 (lower-case hex) a disc model was chosen
    /// from; `None` for the marker.
    pub source: Option<String>,
}

impl Saved {
    pub fn marker(seat_colour: u8) -> Saved {
        Saved {
            look: MonitorLook::default_for(seat_colour),
            source: None,
        }
    }
}

/// What a profile holds, and what was wrong with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub saved: Saved,
    /// One line per bad value, by key. The value is left in the file.
    pub problems: Vec<String>,
}

fn is_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn int(store: &Store, profile: &str, key: &str) -> Option<u8> {
    store
        .profile_get(profile, key)
        .and_then(Value::as_int)
        .and_then(|i| u8::try_from(i).ok())
}

/// `profile`'s saved look. A missing key takes its default (the colours,
/// the seat's: `seat_colour`); a bad value is reported by key and takes the
/// default too, and is never rewritten.
pub fn load(store: &Store, profile: &str, seat_colour: u8) -> Loaded {
    let mut problems = Vec::new();
    let mut text = |key: &str, max: usize| -> Option<String> {
        match store.profile_text(profile, key) {
            Ok(Some(t)) if t.is_empty() => None,
            Ok(Some(t)) if (key == SOURCE && is_hex(&t)) || (key != SOURCE && name_ok(&t, max)) => {
                Some(t)
            }
            Ok(Some(t)) => {
                problems.push(format!("{key} {t:?} is not a valid name; ignored"));
                None
            }
            Ok(None) => None,
            Err(e) => {
                problems.push(format!("{e}; ignored"));
                None
            }
        }
    };
    let model = text(MODEL, KEY_MAX);
    let variant = text(VARIANT, VARIANT_MAX);
    let source = text(SOURCE, 0);
    let mut look = MonitorLook::default_for(seat_colour);
    look.primary = int(store, profile, PRIMARY).unwrap_or(seat_colour);
    look.secondary = int(store, profile, SECONDARY).unwrap_or(seat_colour);
    look.light_colour = int(store, profile, LIGHT_COLOUR).unwrap_or(seat_colour);
    look.tint = int(store, profile, TINT).unwrap_or(0);
    look.light = int(store, profile, LIGHT).unwrap_or(0);
    look.model = model.unwrap_or_else(|| MARKER.into());
    look.variant = variant;
    if let Err(r) = look.validate() {
        problems.push(format!(
            "the saved monitor look was not used: {}",
            r.sentence()
        ));
        return Loaded {
            saved: Saved::marker(seat_colour),
            problems,
        };
    }
    let source = if look.is_marker() { None } else { source };
    Loaded {
        saved: Saved { look, source },
        problems,
    }
}

/// Saves `saved` under `profile`. `Ok(Some(why))` when the look holds for
/// this run but the file was not written. The saved look is checked first.
pub fn save(store: &Store, profile: &str, saved: &Saved) -> Result<Option<String>, String> {
    saved.look.validate().map_err(|r| r.sentence())?;
    let l = &saved.look;
    let mut not_saved = None;
    let mut note = |r: Result<Option<String>, settings::SetError>| match r {
        Ok(Some(why)) => {
            not_saved.get_or_insert(why);
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(e) => Err(e.to_string()),
    };
    note(store.profile_text_set(profile, MODEL, &l.model))?;
    note(store.profile_text_set(profile, SOURCE, saved.source.as_deref().unwrap_or("")))?;
    note(store.profile_text_set(profile, VARIANT, l.variant.as_deref().unwrap_or("")))?;
    for (key, value) in [
        (PRIMARY, l.primary),
        (SECONDARY, l.secondary),
        (TINT, l.tint),
        (LIGHT, l.light),
        (LIGHT_COLOUR, l.light_colour),
    ] {
        note(store.profile_set(profile, key, Value::Int(i64::from(value))))?;
    }
    Ok(not_saved)
}

/// A saved look as this machine can use it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub look: MonitorLook,
    /// Why the look is not the saved one, said once to the player.
    pub notice: Option<String>,
}

/// The saved look against this machine's catalogue. A model the catalogue
/// lacks (or holds from a different source map) is not replaced in the
/// file: the session starts with the marker in the saved colours, and the
/// notice says why. A variant the model lacks is reported the same way.
pub fn resolve(saved: &Saved, catalogue: &[Entry]) -> Resolved {
    let look = &saved.look;
    if look.is_marker() {
        return Resolved {
            look: look.clone(),
            notice: None,
        };
    }
    let entry = catalogue.iter().find(|e| {
        e.key == look.model
            && saved
                .source
                .as_deref()
                .is_none_or(|s| s.eq_ignore_ascii_case(&e.sha256))
    });
    let marker = |notice: String| Resolved {
        look: MonitorLook {
            model: MARKER.into(),
            variant: None,
            tint: 0,
            light: 0,
            ..look.clone()
        },
        notice: Some(notice),
    };
    match entry {
        None => marker(format!(
            "{} needs a campaign map this install does not have",
            title(&look.model)
        )),
        Some(e) => match &look.variant {
            Some(v) if !e.variants.contains(v) => marker(format!(
                "{} has no variant {v:?} on this install",
                title(&look.model)
            )),
            _ => Resolved {
                look: look.clone(),
                notice: None,
            },
        },
    }
}

/// A key as a name for a sentence: `sentinel_enforcer` is "Sentinel enforcer".
pub fn title(key: &str) -> String {
    let mut s = key.replace('_', " ");
    if let Some(first) = s.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    s
}
