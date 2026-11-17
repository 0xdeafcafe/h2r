//! Halo 1 builtins and value types mapped to Halo 2's.
//!
//! Halo 2's descriptors carry no names, so the Halo 2 side ([`Target`]) is
//! built from an `hs::Catalog` that has observed the Halo 2 maps: only the
//! builtins some retail Halo 2 script calls have names. A Halo 1 builtin
//! whose name Halo 2 never calls is reported missing even if Halo 2 has it
//! under an index nobody calls; see `docs/formats/h1-scripts.md`.

use std::collections::{BTreeMap, HashMap};

use crate::catalog::Catalog;

/// Halo 1 builtins Halo 2 has under another name, with the same arguments
/// (or ones Halo 2 casts). Each target must be one a retail Halo 2 script
/// calls, or the entry is reported missing.
pub const RENAMES: &[(&str, &str)] = &[
    ("ai_magically_see_encounter", "ai_magically_see"),
    ("game_save_totally_unsafe", "game_save_immediate"),
    ("ai_attach", "ai_attach_units"),
    ("ai_exit_vehicle", "ai_vehicle_exit"),
    ("object_pvs_set_object", "pvs_set_object"),
    ("object_pvs_clear", "pvs_clear"),
    // Halo 2 has no living fraction its scripts call; its strength is the
    // squad's remaining vitality, 0 when all are dead (a30's waits on
    // `(> 0.3 (ai_living_fraction x))` and `(= 0.0 ...)`).
    ("ai_living_fraction", "ai_strength"),
    // The ticks left of a playing impulse sound (0 once it ends), as Halo
    // 1's. Halo 2's evaluator 0x2a9fe0 finds the sound among the 16 script
    // impulses and returns its permutation's length less the time played,
    // times 30 (0x18a240). Halo 1's scripts wait on it after nearly every
    // line of dialogue; stubbed to 0, those waits ended at once, and a
    // script that replays a sound each time the wait ends replayed it every
    // tick. `docs/formats/h1-scripts.md`.
    ("sound_impulse_time", "sound_impulse_language_time"),
];

/// Halo 1 builtins whose Halo 2 counterpart takes different arguments, and
/// the rewrite that bridges them.
pub const ADAPTERS: &[(&str, &str, Adapter)] = &[
    // Halo 2 adds an optional count. Retail Halo 2 scripts leave it out in
    // 1309 of 2100 calls and never pass 0 (`ai_place_counts` in
    // h1campaign's survey), so the call keeps Halo 1's one argument.
    ("ai_place", "ai_place", Adapter::OmitOptional),
    // Halo 2 adds an exit mode; every retail Halo 2 call passes 1.
    (
        "unit_exit_vehicle",
        "unit_exit_vehicle",
        Adapter::AppendShort(1),
    ),
    // Halo 2 first takes the device that changes the group.
    ("device_group_set", "device_group_set", Adapter::PrependNone),
    // Halo 2's scripts never call `!=`: write `(not (= a b))`.
    ("!=", "=", Adapter::Negate),
];

/// A rewrite from a Halo 1 call to a Halo 2 one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adapter {
    /// Same arity; argument nodes take Halo 2's parameter types and Halo 2's
    /// interpreter casts the values, as it casts any argument. Literals whose
    /// type changes (a marker string becoming a `string_id`) keep their
    /// token and become hooks.
    Coerce,
    /// Append a `short` literal argument. The literal is a hook too, so the
    /// converter can choose another value.
    AppendShort(i16),
    /// Insert `none` as the first argument.
    PrependNone,
    /// Halo 2's last parameter is optional and Halo 1 has no counterpart:
    /// the call keeps Halo 1's arguments, one fewer than Halo 2's table.
    OmitOptional,
    /// Call the target (a comparison) inside `not`.
    Negate,
}

/// One Halo 2 builtin as the translation needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H2Builtin {
    /// Index in Halo 2's builtin table.
    pub index: u16,
    /// Return type.
    pub return_type: u16,
    /// Parameter types (empty for special forms).
    pub parameters: Vec<u16>,
    /// A special form: custom argument parsing (descriptor flag 2).
    pub special: bool,
}

/// Halo 2's builtins, types and engine globals, by name.
#[derive(Debug, Clone, Default)]
pub struct Target {
    /// Named builtins.
    pub builtins: HashMap<String, H2Builtin>,
    /// Value type names, by index.
    pub types: Vec<String>,
    /// Engine (external) globals seen in Halo 2 scripts: name to index and
    /// type.
    pub externals: HashMap<String, (u16, u16)>,
}

impl Target {
    /// The named builtins and types of an observed Halo 2 catalog.
    pub fn from_catalog(c: &hs::Catalog) -> Self {
        let mut builtins: HashMap<String, H2Builtin> = c
            .named_functions()
            .into_iter()
            .filter_map(|(i, name)| {
                let f = c.function(i).ok()?;
                Some((
                    name.to_owned(),
                    H2Builtin {
                        index: i,
                        return_type: f.return_type(),
                        parameters: f.parameters().to_vec(),
                        special: f.flags() & 2 != 0,
                    },
                ))
            })
            .collect();
        crate::identified::name_unnamed(&mut builtins, c);
        Self {
            builtins,
            types: c.types().to_vec(),
            externals: HashMap::new(),
        }
    }

    /// Records the engine globals a Halo 2 scenario's scripts name.
    pub fn observe_externals(&mut self, s: &hs::Scenario) {
        for n in s.nodes().iter().filter(|n| n.live() && n.global()) {
            let index = n.data() as u16;
            if index & 0x8000 != 0
                && let Ok(name) = s.string(n.string_offset())
            {
                self.externals
                    .insert(name.to_owned(), (index & 0x7fff, n.value_type()));
            }
        }
    }

    /// The type called `name`.
    pub fn type_index(&self, name: &str) -> Option<u16> {
        self.types.iter().position(|t| t == name).map(|i| i as u16)
    }
}

/// How one Halo 1 builtin carries over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// Same name; the same signature once types are mapped, or both are
    /// special forms.
    Direct,
    /// Another name, same signature ([`RENAMES`]).
    Renamed,
    /// Through an [`Adapter`]: a cast to Halo 2's parameter types, or one of
    /// [`ADAPTERS`].
    Adapter,
    /// Halo 2 has no builtin by this name (or none its scripts call), or one
    /// with a different arity, or a parameter type Halo 2 lacks. A candidate
    /// for the Rust HS VM.
    Missing,
}

/// One Halo 1 builtin's translation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Translation {
    /// Halo 1 name.
    pub name: String,
    /// How it carries over.
    pub kind: Kind,
    /// The Halo 2 builtin and its name, unless missing.
    pub target: Option<(String, H2Builtin)>,
    /// The rewrite, for [`Kind::Adapter`].
    pub adapter: Option<Adapter>,
    /// Why it is missing, when it is.
    pub reason: Option<&'static str>,
}

/// Every Halo 1 builtin and value type, translated.
#[derive(Debug, Clone)]
pub struct Table {
    /// By Halo 1 builtin index.
    pub builtins: Vec<Translation>,
    /// Halo 2 type for each Halo 1 type index, matched by name.
    pub types: Vec<Option<u16>>,
}

impl Table {
    /// Translates every builtin of `h1` against `h2`.
    pub fn build(h1: &Catalog, h2: &Target) -> Self {
        let types: Vec<Option<u16>> = h1.types().iter().map(|t| h2.type_index(t)).collect();
        let map_ty = |t: u16| types.get(t as usize).copied().flatten();
        let renames: HashMap<&str, &str> = RENAMES.iter().copied().collect();
        let adapters: HashMap<&str, (&str, Adapter)> =
            ADAPTERS.iter().map(|&(a, b, k)| (a, (b, k))).collect();
        let builtins = h1
            .functions()
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let missing = |reason| Translation {
                    name: f.name.clone(),
                    kind: Kind::Missing,
                    target: None,
                    adapter: None,
                    reason: Some(reason),
                };
                if let Some(&(h2_name, adapter)) = adapters.get(f.name.as_str()) {
                    let Some(b) = h2.builtins.get(h2_name) else {
                        return missing("adapter target not called by Halo 2 scripts");
                    };
                    return Translation {
                        name: f.name.clone(),
                        kind: Kind::Adapter,
                        target: Some((h2_name.to_owned(), b.clone())),
                        adapter: Some(adapter),
                        reason: None,
                    };
                }
                let renamed = renames.get(f.name.as_str()).copied();
                let h2_name = renamed.unwrap_or(&f.name);
                let Some(b) = h2.builtins.get(h2_name) else {
                    return missing("no Halo 2 builtin by this name");
                };
                let same = if renamed.is_some() {
                    Kind::Renamed
                } else {
                    Kind::Direct
                };
                let special = !h1.fixed_signature(i as u16);
                let kind = if special || b.special {
                    if special != b.special {
                        return missing("special form on one side only");
                    }
                    (same, None)
                } else if f.parameters.len() != b.parameters.len() {
                    return missing("different arity");
                } else {
                    let mapped: Option<Vec<u16>> =
                        f.parameters.iter().map(|&t| map_ty(t)).collect();
                    let Some(mapped) = mapped else {
                        return missing("a parameter type Halo 2 lacks");
                    };
                    if mapped == b.parameters && map_ty(f.return_type) == Some(b.return_type) {
                        (same, None)
                    } else {
                        (Kind::Adapter, Some(Adapter::Coerce))
                    }
                };
                Translation {
                    name: f.name.clone(),
                    kind: kind.0,
                    target: Some((h2_name.to_owned(), b.clone())),
                    adapter: kind.1,
                    reason: None,
                }
            })
            .collect();
        Self { builtins, types }
    }

    /// Halo 1 builtin `index`'s translation.
    pub fn builtin(&self, index: u16) -> Option<&Translation> {
        self.builtins.get(index as usize)
    }

    /// Halo 2's type for Halo 1 type `ty`.
    pub fn ty(&self, ty: u16) -> Option<u16> {
        self.types.get(ty as usize).copied().flatten()
    }

    /// Calls by kind, from per-builtin use counts.
    pub fn coverage(&self, uses: &BTreeMap<u16, usize>) -> Coverage {
        let mut c = Coverage::default();
        for (&i, &n) in uses {
            let kind = self.builtin(i).map_or(Kind::Missing, |t| t.kind);
            *c.calls.entry(kind).or_default() += n;
            if kind == Kind::Missing {
                *c.missing.entry(i).or_default() += n;
            }
        }
        c
    }
}

/// Calls by how they translate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Calls per kind.
    pub calls: BTreeMap<Kind, usize>,
    /// Calls to each missing builtin, by Halo 1 index.
    pub missing: BTreeMap<u16, usize>,
}

impl Coverage {
    /// All calls.
    pub fn total(&self) -> usize {
        self.calls.values().sum()
    }
    /// Calls of `kind`.
    pub fn of(&self, kind: Kind) -> usize {
        self.calls.get(&kind).copied().unwrap_or(0)
    }
    /// Adds another map's coverage.
    pub fn add(&mut self, other: &Coverage) {
        for (&k, &n) in &other.calls {
            *self.calls.entry(k).or_default() += n;
        }
        for (&k, &n) in &other.missing {
            *self.missing.entry(k).or_default() += n;
        }
    }
}
