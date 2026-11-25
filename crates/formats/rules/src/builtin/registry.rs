//! The script registry: the built-in scripts as a set of packages gives
//! them (docs/editor/studio-redesign.md, "Each content path as a package").
//!
//! `core`'s `rules` items are the retail types' scripts; `modes`' `variant`
//! items are the built-in list entries, in manifest order, and its other
//! `rules` items the example and matchmaking scripts. [`Registry::baked`] is
//! what the first-party packages hold, which `super`'s constants are the
//! files of; a boot with a package off, or a checkout with one edited,
//! builds the registry from that set's sources instead
//! (`workspace::registry`).
use std::collections::BTreeSet;

use content::package::{Kind, Manifest};

use super::*;

/// What an item makes of its script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// A `rules` item of `core`: the script of a retail type, found by
    /// engine index.
    Retail,
    /// A `variant` item: a built-in list entry.
    Variant,
    /// Any other `rules` item: an example, or a matchmaking script when it
    /// `uses` a type's game.
    Rules,
}

/// One item's script, before it is compiled: the file's stem names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub key: String,
    pub role: Role,
    pub text: String,
}

/// One compiled script of the registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    pub key: String,
    pub role: Role,
    /// `header.name`.
    pub name: String,
    /// The engine index its record is on (`ir::Header::engine`).
    pub engine: u8,
    /// Whether it `uses` a type's built-in game: a settings script.
    pub settings: bool,
    pub source: String,
}

/// The keys `examples()` names.
pub const EXAMPLE_KEYS: [&str; 7] = [
    "infection",
    "gun_game",
    "vip",
    "siege",
    "race",
    "bounce_hunt",
    "superbounce",
];

/// The scripts of a set of packages, in the set's order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registry {
    scripts: Vec<Script>,
    /// A line for each item left out, and why.
    pub problems: Vec<String>,
}

impl Registry {
    /// Compiles each source; one that does not compile or does not fit its
    /// tick budget is left out and named in [`Registry::problems`]. A later
    /// script with a key and role an earlier one has replaces it, in the earlier's
    /// place; a second [`Role::Retail`] script for an engine is left out, as
    /// no package can replace a retail type's game.
    pub fn from_sources(sources: impl IntoIterator<Item = Source>) -> Registry {
        let mut out = Registry::default();
        for s in sources {
            let ir = match crate::lang::compile(&s.text) {
                Ok(ir) => ir,
                Err(e) => {
                    out.problems.push(format!("{}: {e}", s.key));
                    continue;
                }
            };
            match crate::verify::verify(&ir) {
                Ok(r) if r.fits() => {}
                Ok(r) => {
                    out.problems
                        .push(format!("{}: worst tick {}", s.key, r.worst_tick));
                    continue;
                }
                Err(e) => {
                    out.problems.push(format!("{}: {e}", s.key));
                    continue;
                }
            }
            let script = Script {
                key: s.key,
                role: s.role,
                name: ir.header.name.clone(),
                engine: ir.header.engine(),
                settings: ir.header.uses.is_some(),
                source: s.text,
            };
            if script.role == Role::Retail
                && (script.settings
                    || !ir.header.game_type.is_retail()
                    || out.retail(script.engine).is_some())
            {
                out.problems.push(format!(
                    "{}: not a retail type's game, or one already",
                    script.key
                ));
                continue;
            }
            match out
                .scripts
                .iter()
                .position(|o| o.key == script.key && o.role == script.role)
            {
                Some(at) => out.scripts[at] = script,
                None => out.scripts.push(script),
            }
        }
        out
    }

    /// What the first-party packages hold, in their order: the constants of
    /// this module.
    pub fn baked() -> Registry {
        let mut sources = Vec::new();
        for (key, b) in [
            ("slayer", SLAYER),
            ("oddball", ODDBALL),
            ("koth", KING),
            ("juggernaut", JUGGERNAUT),
            ("territories", TERRITORIES),
            ("ctf", CTF),
            ("assault", ASSAULT),
        ] {
            sources.push(Source {
                key: key.into(),
                role: Role::Retail,
                text: b.into(),
            });
        }
        for (key, text) in LISTED_FILES {
            sources.push(Source {
                key: (*key).into(),
                role: Role::Variant,
                text: (*text).into(),
            });
        }
        for (key, text) in examples() {
            if !LISTED_FILES.iter().any(|(k, _)| k == key) {
                sources.push(Source {
                    key: (*key).into(),
                    role: Role::Rules,
                    text: (*text).into(),
                });
            }
        }
        for m in MATCHMAKING {
            sources.push(Source {
                key: m.key.into(),
                role: Role::Rules,
                text: m.source.into(),
            });
        }
        Registry::from_sources(sources)
    }

    /// Every script, in the set's order.
    pub fn scripts(&self) -> &[Script] {
        &self.scripts
    }

    /// The script named by `key` (its file's stem), a retail type's apart:
    /// those are found by [`Registry::retail`], and a matchmaking script
    /// shares its stem with the retail game it settles.
    pub fn script(&self, key: &str) -> Option<&Script> {
        self.scripts
            .iter()
            .find(|s| s.key == key && s.role != Role::Retail)
    }

    /// The retail type's script on engine index `engine`.
    pub fn retail(&self, engine: u8) -> Option<&Script> {
        self.scripts
            .iter()
            .find(|s| s.role == Role::Retail && s.engine == engine)
    }

    /// The retail scripts, in the set's order.
    pub fn retail_scripts(&self) -> impl Iterator<Item = &Script> {
        self.scripts.iter().filter(|s| s.role == Role::Retail)
    }

    /// The built-in list entries, in the set's order: what the game's
    /// variant list gets at load.
    pub fn listed(&self) -> impl Iterator<Item = &Script> {
        self.scripts.iter().filter(|s| s.role == Role::Variant)
    }

    /// The settings scripts a default hopper's entries name by key, in the
    /// set's order.
    pub fn matchmaking(&self) -> impl Iterator<Item = &Script> {
        self.scripts
            .iter()
            .filter(|s| s.role == Role::Rules && s.settings)
    }

    /// The example scripts, in [`EXAMPLE_KEYS`]' order: those the set has.
    pub fn examples(&self) -> Vec<&Script> {
        EXAMPLE_KEYS.iter().filter_map(|k| self.script(k)).collect()
    }

    /// Whether the set has a script for every key (the keys a playlist
    /// names).
    pub fn missing(&self, keys: impl IntoIterator<Item = String>) -> BTreeSet<String> {
        keys.into_iter()
            .filter(|k| self.script(k).is_none())
            .collect()
    }
}

/// The scripts of a package's `rules` and `variant` items, in manifest
/// order, with a line for each that cannot be read. An item's text is its
/// source then its `inputs`, joined (CTF and Assault are a header over the
/// shared body); its key is the source's file name up to the first `.`. A
/// `rules` item of `core` is [`Role::Retail`].
pub fn package_sources(
    manifest: &Manifest,
    file: impl Fn(&str) -> Option<Vec<u8>>,
) -> (Vec<Source>, Vec<String>) {
    let (mut sources, mut problems) = (Vec::new(), Vec::new());
    for item in &manifest.items {
        // A Firefight wave set is a `rules` item no script reads
        // (`workspace::firefight_sets`).
        if item.kind == Kind::Rules
            && item
                .source
                .as_deref()
                .is_some_and(|s| s.to_ascii_lowercase().ends_with(".firefight.json"))
        {
            continue;
        }
        // A portable record names a script via its extension block.
        if item.kind == Kind::Variant
            && item.source.as_deref().is_some_and(|s| s.ends_with(".h2variant"))
        {
            continue;
        }
        let role = match item.kind {
            Kind::Rules if manifest.package.id == "core" => Role::Retail,
            Kind::Rules => Role::Rules,
            Kind::Variant => Role::Variant,
            _ => continue,
        };
        let Some(source) = item.source.as_deref() else {
            problems.push(format!(
                "{}: a {:?} item has no source",
                manifest.package.id, item.kind
            ));
            continue;
        };
        let mut text = String::new();
        let mut ok = true;
        for name in std::iter::once(source).chain(item.inputs.iter().map(String::as_str)) {
            match file(name).map(String::from_utf8) {
                Some(Ok(t)) => text.push_str(&t),
                Some(Err(_)) => {
                    problems.push(format!("{}: {name}: not text", manifest.package.id));
                    ok = false;
                }
                None => {
                    problems.push(format!(
                        "{}: {name}: not in the package",
                        manifest.package.id
                    ));
                    ok = false;
                }
            }
        }
        if ok {
            let stem = source.rsplit('/').next().unwrap_or(source);
            let key = stem.split('.').next().unwrap_or(stem).to_string();
            sources.push(Source { key, role, text });
        }
    }
    (sources, problems)
}

/// [`LISTED`]'s scripts by the stem of the file each is, in list order.
pub const LISTED_FILES: &[(&str, &str)] = &[
    ("zombies", ZOMBIES),
    ("hide_and_seek", HIDE_AND_SEEK),
    ("jousting", JOUSTING),
    ("swat", SWAT),
    ("duel", DUEL),
    ("clint_eastwood", CLINT_EASTWOOD),
    ("sun_king", SUN_KING),
    ("tower_of_power", TOWER_OF_POWER),
    ("wheeeeee", WHEEEEEE),
    ("cops_and_robbers", COPS_AND_ROBBERS),
    ("bounce_practice", BOUNCE_PRACTICE),
    ("glitch_hunt", GLITCH_HUNT),
    ("bumper_cars", BUMPER_CARS),
    ("superbounce", SUPERBOUNCE),
    ("siege", SIEGE),
    ("gun_game", GUN_GAME),
    ("race", RACE),
    ("vip", VIP),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_baked_registry_is_the_constants() {
        let r = Registry::baked();
        assert_eq!(r.problems, Vec::<String>::new());
        let retail: Vec<_> = r
            .retail_scripts()
            .map(|s| (s.name.as_str(), s.engine, s.source.as_str()))
            .collect();
        let all: Vec<_> = ALL.iter().map(|b| (b.name, b.engine, b.source)).collect();
        assert_eq!(retail.len(), all.len());
        for a in &all {
            assert!(retail.contains(a), "{}", a.0);
            assert_eq!(r.retail(a.1).map(|s| s.source.as_str()), builtin(a.1));
        }
        let listed: Vec<_> = r
            .listed()
            .map(|s| (s.name.as_str(), s.engine, s.source.as_str()))
            .collect();
        let want: Vec<_> = LISTED
            .iter()
            .map(|b| (b.name, b.engine, b.source))
            .collect();
        assert_eq!(listed, want);
        let keys: Vec<_> = r
            .matchmaking()
            .map(|s| (s.key.as_str(), s.engine, s.source.as_str()))
            .collect();
        let want: Vec<_> = MATCHMAKING
            .iter()
            .map(|m| (m.key, m.engine, m.source))
            .collect();
        assert_eq!(keys, want);
        let examples: Vec<_> = r
            .examples()
            .iter()
            .map(|s| (s.key.as_str(), s.source.as_str()))
            .collect();
        assert_eq!(examples, examples_of_consts());
    }

    fn examples_of_consts() -> Vec<(&'static str, &'static str)> {
        let all = examples();
        EXAMPLE_KEYS
            .iter()
            .map(|k| *all.iter().find(|(n, _)| n == k).unwrap())
            .collect()
    }

    fn src(key: &str, role: Role, text: &str) -> Source {
        Source {
            key: key.into(),
            role,
            text: text.into(),
        }
    }

    #[test]
    fn a_set_without_a_package_has_none_of_its_entries() {
        let all = Registry::baked();
        let without = Registry::from_sources(
            all.scripts()
                .iter()
                .filter(|s| s.role != Role::Variant)
                .map(|s| src(&s.key, s.role, &s.source)),
        );
        assert_eq!(without.listed().count(), 0);
        assert_eq!(
            without.retail_scripts().count(),
            all.retail_scripts().count()
        );
    }

    #[test]
    fn a_script_that_does_not_compile_is_named_and_left_out() {
        let r = Registry::from_sources([
            src("slayer", Role::Retail, SLAYER),
            src("broken", Role::Variant, "rules \"x\" 1\nnope"),
        ]);
        assert_eq!(r.scripts().len(), 1);
        assert_eq!(r.problems.len(), 1);
        assert!(r.problems[0].starts_with("broken: "), "{:?}", r.problems);
    }

    #[test]
    fn a_retail_type_has_one_script_and_a_later_key_replaces_in_place() {
        let r = Registry::from_sources([
            src("slayer", Role::Retail, SLAYER),
            src("also_slayer", Role::Retail, SLAYER),
            src("zombies", Role::Variant, ZOMBIES),
            src("swat", Role::Variant, SWAT),
            src("zombies", Role::Variant, DUEL),
        ]);
        assert_eq!(r.retail_scripts().count(), 1);
        assert_eq!(r.problems.len(), 1);
        let names: Vec<_> = r.listed().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Duel", "SWAT"]);
    }

    #[test]
    fn a_variant_that_is_not_a_retail_game_is_not_retail() {
        let r = Registry::from_sources([src("zombies", Role::Retail, ZOMBIES)]);
        assert_eq!(r.retail_scripts().count(), 0);
        assert_eq!(r.problems.len(), 1);
    }
}
