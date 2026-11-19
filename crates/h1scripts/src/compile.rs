//! Translated Halo 1 scripts as Halo 2 syntax nodes and scenario tables.
//!
//! The output has Halo 2's wire layout ([`hs::Node`], `hs::Script`,
//! `hs::Global`, `hs::Reference`) and goes into a Halo 2 scenario as
//! `map::build` blocks ([`Compiled::install`]). Values that name things in
//! the converted level (objects, AI, trigger volumes, cutscene points, tags,
//! enum values, string IDs) cannot be known here: their atoms keep the Halo 1
//! token in the string data, hold a placeholder value, and are listed as
//! [`Hook`]s the converter fills in.

use std::collections::HashMap;

use hs::{Datum, Node};
use map::build::Block;

use crate::catalog::{Catalog, types};
use crate::layout::Tables;
use crate::translate::{Adapter, Kind, Table, Target};
use crate::{Result, invalid};

mod conversations;

/// Halo 2 scenario fields the tables go in (docs/formats/haloscript.md).
pub mod field {
    /// Script string data (byte count, address).
    pub const STRINGS: u32 = 0x1b0;
    /// Scripts block.
    pub const SCRIPTS: u32 = 0x1b8;
    /// Globals block.
    pub const GLOBALS: u32 = 0x1c0;
    /// Script references block.
    pub const REFERENCES: u32 = 0x1c8;
    /// Syntax nodes block.
    pub const NODES: u32 = 0x238;
    /// Bytes of scenario these fields need.
    pub const MIN_LEN: usize = 0x240;
}

/// Halo 2 value types the compiler writes by number (from Halo 2's type
/// name table: `crates/hs` reads it from the XBE).
mod h2t {
    pub const FUNCTION_NAME: u16 = 2;
    pub const PASSTHROUGH: u16 = 3;
    pub const VOID: u16 = 4;
    pub const BOOLEAN: u16 = 5;
    pub const REAL: u16 = 6;
    pub const SHORT: u16 = 7;
    pub const LONG: u16 = 8;
    pub const STRING: u16 = 9;
    pub const SCRIPT: u16 = 10;
    pub const STRING_ID: u16 = 11;
}

/// Node flags: atom, script call, global, retained (bit 3, on every node).
const CALL: u16 = 8;
const ATOM: u16 = 9;
const SCRIPT_CALL: u16 = 10;
const GLOBAL: u16 = 13;
/// Placeholder value of a hooked atom: Halo 2's `none`.
pub const NONE: u32 = u32::MAX;
/// Most nodes Halo 2's syntax pool holds (`0x9000`).
pub const MAX_NODES: usize = hs::MAX_NODES;
/// First opcode of [`Missing::Extension`] calls: one past Halo 2's table.
pub const EXTENSION_BASE: u16 = 0x8000;

/// What to do with a call to a builtin Halo 2 lacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missing {
    /// Replace the call with a literal of the type its parent expects
    /// (`false`, `0`, `none`, void). Retail Halo 2 runs the result.
    Stub,
    /// Keep the call, with opcode [`EXTENSION_BASE`] + the Halo 1 index and
    /// the Halo 1 name as its head token, for the Rust HS VM to bind. Retail
    /// Halo 2's interpreter cannot run it.
    Extension,
}

/// Halo 1 cutscenes and tutorials, which need what is not converted yet
/// (cutscene animation, recordings, command lists, HUD help text).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cutscenes {
    /// Compile them as written.
    Play,
    /// Take the path Halo 1 takes when the player skips: every call of the
    /// level's `cinematic_skip_start` script is `false`, so `(if
    /// (cinematic_skip_start) (x10))` never runs the cinematic and the
    /// script carries on as after a skip (its fade in, `cinematic_stop`,
    /// control back). A wake of a tutorial script with a short path
    /// ([`SHORT_PATHS`]) wakes the short path.
    #[default]
    Skip,
    /// Play the cinematics, but wake the short path of a tutorial that
    /// cannot finish ([`SHORT_PATHS`]; a10's cryo tutorial waits on look
    /// tests Halo 2 has no builtin for).
    PlayShortPaths,
}

/// The user script whose call guards every skippable Halo 1 cinematic.
pub const SKIP_GUARD: &str = "cinematic_skip_start";

/// Under [`Cutscenes::Skip`], `(wake from)` becomes `(wake to)` when the
/// level has both scripts. a10: the cryo tutorial (normal difficulty, single
/// player) disables input and waits on look and movement tests and command
/// lists Halo 2 lacks; the fast setup is the path Halo 1 takes in co-op
/// and on other difficulties, and gives control back.
pub const SHORT_PATHS: &[(&str, &str)] = &[("tutorial_setup", "fast_setup")];

/// What a stubbed status query returns under [`Missing::Stub`] instead of
/// zero, where zero would hold a `sleep_until` forever. Every use in the
/// ten levels' scripts reads the value as "done" (docs/halo1/scripts.md):
/// command list status 2 passes `(= 2 s)` and `(!= 3 s)` (the
/// four `(= 1 s)` waits still hold); the quiet and safe-to-speak tests
/// are true, and the HUD timer never runs out.
pub const DONE_VALUES: &[(&str, i32)] = &[
    ("ai_command_list_status", 2),
    ("game_all_quiet", 1),
    ("game_safe_to_speak", 1),
    // d40's escape timer: `(<= (hud_get_timer_ticks) 0)` ends the level
    // as failed; Halo 2 has no HUD timer, so it never runs out.
    ("hud_get_timer_ticks", 32767),
];

/// Halo 1's `ai_status` scale on Halo 2's combat status, highest first:
/// the Halo 2 engine global a status must reach, and the Halo 1 value.
/// Halo 1's XBE describes its scale as 0 inactive, 1 noncombat, 2
/// guarding, 3 search/suspicious, 4 definite enemy (heard or magic
/// awareness), 5 visible enemy, 6 engaging in combat; Halo 2's globals are
/// the ones its scripts compare with (`ai_combat_status_idle` to
/// `_dangerous`, ascending: every retail comparison reads them so). A
/// visible enemy counts as engaging, which is what Halo 1's waits on
/// `(= (ai_status x) 6)` ask for.
pub const STATUS_LADDER: &[(&str, i32)] = &[
    ("ai_combat_status_visible", 6),
    ("ai_combat_status_certain", 5),
    ("ai_combat_status_uninspected", 4),
    ("ai_combat_status_active", 3),
    ("ai_combat_status_alert", 2),
    ("ai_combat_status_idle", 1),
];

/// The `h1_type` of a hook on an animation name: its token is
/// `<graph tag name>|<Halo 1 animation name>`, and the converter writes the
/// string ID of that animation's name in the converted `jmad`
/// (`h1models::jmad::animation_names`).
pub const ANIMATION_NAME: &str = "animation_name";

/// Halo 2 value types whose value indexes a scenario block the engine
/// reads without a check: `none` there reads before the block (b40:
/// `object_teleport` read cutscene flag -1 at `0x002a003f`). A call with a
/// direct argument of one of these types that will not resolve is stubbed
/// ([`compile_with`]'s `resolves`), and listed in [`Compiled::unindexed`].
pub const INDEXED: &[&str] = &[
    "cutscene_flag",
    "cutscene_camera_point",
    "cutscene_title",
    "device_group",
];

/// Halo 2 builtins that hand their object to the engine without a `none`
/// check. `unit_open` (194, evaluator `0x002a32a0`) and `unit_close`
/// (195, `0x002a32e0`) send the unit message 0x17 or 0x18 through
/// `0x000e6900`, whose handlers (`0x000e9370`, `0x000e9500`) walk the
/// object and its children by index (`0x000e92e0`): `none` takes header
/// 0xffff, whose object pointer is null, and the read of its `+0x33e` at
/// `0x00113eae` faults. a10's `fast_setup` opens `cryotube_1` as its
/// cinematic ends; while the cryotube had no template the name had no
/// placement (it is built from its definition now, `h1objects::rebuild`,
/// and this stays the safety net for any other unplaced name). A call whose direct argument is an object name that will not
/// resolve to a placement ([`compile_with`]'s `resolves`, with a type from
/// [`OBJECT_NAME_TYPES`]) is stubbed and listed in
/// [`Compiled::unindexed`], as for [`INDEXED`].
///
/// Every other call is guarded, as Halo 1 makes these a no-op on `none`:
/// `(if (> (list_count x) 0) (unit_open x))`. A placed name is still `none`
/// when the object was never created or is gone (a10's rebuilt cryotube:
/// `vehicle_new` refuses a model with no physics model). The argument
/// casts to an object list through `0x0020a130` (a name) or `0x0020a170`
/// (an object), which give `none` for `none`, and `list_count`
/// (`0x002a1060`) counts a `none` list as 0. Not a bare `(if (list_count
/// x) ..)`: the retail short-to-boolean cast (`0x0020a020`) is `sete`,
/// true for 0. Of the 79 Halo 2 builtins with a unit or vehicle
/// parameter, only these two reach an object without a `none` check
/// (their evaluators and first callees read; `recording_time`, the other
/// candidate, only searches the recordings by handle).
pub const NONE_UNSAFE: &[&str] = &["unit_open", "unit_close"];

/// Halo 2 value types whose atoms are object names (an index into the
/// scenario's object names).
pub const OBJECT_NAME_TYPES: &[&str] = &[
    "object_name",
    "unit_name",
    "vehicle_name",
    "weapon_name",
    "device_name",
    "scenery_name",
    "object",
    "unit",
    "vehicle",
];

/// A value the converter must supply: write it into node `slot`'s data word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hook {
    /// The atom's slot in [`Compiled::nodes`].
    pub slot: u16,
    /// Halo 2 value type of the literal (its source type).
    pub h2_type: u16,
    /// Halo 2's name for that type.
    pub type_name: String,
    /// Halo 1's name for the literal's original type.
    pub h1_type: String,
    /// The token: an object, squad, flag or tag name, an enum value, or the
    /// text to intern as a string ID.
    pub token: String,
}

/// A call compiled under [`Missing`], or an engine global Halo 2 lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    /// The script or global it is in.
    pub definition: String,
    /// The Halo 1 builtin or engine global.
    pub name: String,
}

/// Halo 2 script tables, ready to install.
#[derive(Debug, Clone, Default)]
pub struct Compiled {
    /// Syntax nodes, every one live.
    pub nodes: Vec<Node>,
    /// String data.
    pub strings: Vec<u8>,
    /// Scripts, encoded (`hs::Script::SIZE` bytes each).
    pub scripts: Vec<[u8; 40]>,
    /// Globals, encoded.
    pub globals: Vec<[u8; 40]>,
    /// Halo 1's script references: group and tag name, for the converter to
    /// resolve to Halo 2 tags.
    pub references: Vec<(h1map::Group, String)>,
    /// Atoms the converter must fill in.
    pub hooks: Vec<Hook>,
    /// Calls to builtins Halo 2 lacks, as compiled under [`Missing`].
    pub missing: Vec<Unresolved>,
    /// Engine globals Halo 2 lacks (compiled as literals).
    pub missing_globals: Vec<Unresolved>,
    /// Calls stubbed like a missing builtin because an [`INDEXED`]
    /// argument, or a [`NONE_UNSAFE`] builtin's object name, would not
    /// resolve: the builtin, then the type and token.
    pub unindexed: Vec<Unresolved>,
    /// [`NONE_UNSAFE`] calls compiled inside a guard that skips them when
    /// their unit is `none`.
    pub guarded: Vec<Unresolved>,
    /// Halo 1 `(sleep <negative> [script])` calls compiled as Halo 2's
    /// `sleep_forever`: Halo 2 takes a negative duration as no sleep.
    pub forever: Vec<Unresolved>,
}

/// Compiles every global and script of `t`.
pub fn compile(
    t: &Tables,
    h1: &Catalog,
    table: &Table,
    h2: &Target,
    policy: Missing,
    reference_names: &[String],
) -> Result<Compiled> {
    compile_with(
        t,
        h1,
        table,
        h2,
        policy,
        Cutscenes::Play,
        &|_, _| true,
        reference_names,
    )
}

/// [`compile`], with Halo 1's cutscenes and tutorials played or skipped,
/// and `resolves(type, token)` saying whether a hook of an [`INDEXED`]
/// type (or an object name passed to a [`NONE_UNSAFE`] builtin) will get a
/// value other than `none`: a call with a direct argument that will not is
/// compiled as its type's zero, like a missing builtin under
/// [`Missing::Stub`].
#[allow(clippy::too_many_arguments)]
pub fn compile_with(
    t: &Tables,
    h1: &Catalog,
    table: &Table,
    h2: &Target,
    policy: Missing,
    cutscenes: Cutscenes,
    resolves: &dyn Fn(&str, &str) -> bool,
    reference_names: &[String],
) -> Result<Compiled> {
    compile_scenario(
        t,
        h1,
        table,
        h2,
        policy,
        cutscenes,
        resolves,
        reference_names,
        None,
    )
}

/// Compiles a level's scripts together with its scenario AI conversations.
#[allow(clippy::too_many_arguments)]
pub fn compile_with_conversations(
    t: &Tables,
    h1: &Catalog,
    table: &Table,
    h2: &Target,
    policy: Missing,
    cutscenes: Cutscenes,
    resolves: &dyn Fn(&str, &str) -> bool,
    reference_names: &[String],
    conversations: &[h1ai::conversations::Conversation],
    speaker_scopes: &h1ai::conversations::SpeakerScopes,
) -> Result<Compiled> {
    compile_scenario(
        t,
        h1,
        table,
        h2,
        policy,
        cutscenes,
        resolves,
        reference_names,
        Some((conversations, speaker_scopes)),
    )
}

#[allow(clippy::too_many_arguments)]
fn compile_scenario(
    t: &Tables,
    h1: &Catalog,
    table: &Table,
    h2: &Target,
    policy: Missing,
    cutscenes: Cutscenes,
    resolves: &dyn Fn(&str, &str) -> bool,
    reference_names: &[String],
    conversation_data: Option<(
        &[h1ai::conversations::Conversation],
        &h1ai::conversations::SpeakerScopes,
    )>,
) -> Result<Compiled> {
    let mut c = Compiler {
        t,
        h1,
        table,
        h2,
        policy,
        cutscenes,
        resolves,
        out: Compiled::default(),
        interned: HashMap::new(),
        definition: String::new(),
        conversations: None,
        conversation_heads: HashMap::new(),
    };
    c.out.scripts.resize(t.scripts.len(), [0; 40]);
    c.out.globals.resize(t.globals.len(), [0; 40]);
    if let Some((scenes, scopes)) = conversation_data {
        let lowering = conversations::build(&mut c, scenes, scopes)?;
        c.conversations = Some(lowering);
    }
    for (i, g) in t.globals.iter().enumerate() {
        c.definition = g.name.clone();
        let ty = c.ty(g.value_type)?;
        let root = c.expr(g.root, ty)?;
        c.out.globals[i] = definition(&g.name, ty, 0, root, i)?;
    }
    for (i, s) in t.scripts.iter().enumerate() {
        c.definition = s.name.clone();
        let ty = c.ty(s.return_type)?;
        let root = c.expr(s.root, ty)?;
        c.out.scripts[i] = definition(&s.name, s.kind, ty, root, i)?;
    }
    c.out.references = t
        .references
        .iter()
        .enumerate()
        .map(|(i, r)| (r.group, reference_names.get(i).cloned().unwrap_or_default()))
        .collect();
    if let Some((scenes, _)) = conversation_data {
        conversations::append_sound_references(&mut c.out, scenes);
    }
    if c.out.nodes.len() > MAX_NODES {
        return Err(invalid(format!(
            "{} nodes, Halo 2 holds {MAX_NODES}",
            c.out.nodes.len()
        )));
    }
    Ok(c.out)
}

/// Appends a startup script `name` that places each of `squads` with a
/// one-argument `ai_place`, each squad an `ai` atom hooked with its name
/// (the converter resolves it like any other). Halo 1 creates an encounter
/// at load unless it is flagged "not initially created"; Halo 2 places
/// squads only from scripts (no retail squad sets a placement flag), so
/// the converted level does it here.
pub fn add_startup_placements(
    out: &mut Compiled,
    h2: &Target,
    name: &str,
    squads: &[String],
) -> Result<()> {
    if squads.is_empty() {
        return Ok(());
    }
    let place = h2
        .builtins
        .get("ai_place")
        .ok_or_else(|| invalid("Halo 2 has no ai_place"))?
        .clone();
    let begin = h2
        .builtins
        .get("begin")
        .ok_or_else(|| invalid("Halo 2 has no begin"))?
        .clone();
    let ai = *place
        .parameters
        .first()
        .ok_or_else(|| invalid("ai_place takes no squad"))?;
    let type_name = h2.types.get(ai as usize).cloned().unwrap_or_default();
    let push = |out: &mut Compiled, opcode: u16, ty: u16, flags: u16, token: &[u8], data: u32| {
        let string = out.strings.len() as u32;
        out.strings.extend_from_slice(token);
        out.strings.push(0);
        let slot = out.nodes.len();
        let salt = 0x8000 | (slot as u16 & 0x7fff);
        let mut b = [0u8; Node::SIZE];
        b[0..2].copy_from_slice(&salt.to_le_bytes());
        b[2..4].copy_from_slice(&opcode.to_le_bytes());
        b[4..6].copy_from_slice(&ty.to_le_bytes());
        b[6..8].copy_from_slice(&flags.to_le_bytes());
        b[8..12].copy_from_slice(&Datum::NULL.0.to_le_bytes());
        b[12..16].copy_from_slice(&string.to_le_bytes());
        b[16..20].copy_from_slice(&data.to_le_bytes());
        out.nodes.push(Node::decode(&b).expect("20 bytes"));
        Datum(u32::from(salt) << 16 | slot as u32)
    };
    let link = |out: &mut Compiled, d: Datum, next: Datum| {
        let mut b = out.nodes[d.slot()].encode();
        b[8..12].copy_from_slice(&next.0.to_le_bytes());
        out.nodes[d.slot()] = Node::decode(&b).expect("20 bytes");
    };
    // A call: head atom, arguments chained after it, then the call node.
    let call =
        |out: &mut Compiled, b: &crate::translate::H2Builtin, token: &[u8], args: &[Datum]| {
            let head = push(out, b.index, h2t::FUNCTION_NAME, ATOM, token, 0);
            let mut prev = head;
            for &a in args {
                link(out, prev, a);
                prev = a;
            }
            // Both calls stand where a script expects void.
            push(out, b.index, h2t::VOID, CALL, b"", head.0)
        };
    let mut calls = Vec::new();
    for squad in squads {
        let atom = push(out, ai, ai, ATOM, squad.as_bytes(), NONE);
        out.hooks.push(Hook {
            slot: atom.slot() as u16,
            h2_type: ai,
            type_name: type_name.clone(),
            h1_type: "ai".into(),
            token: squad.clone(),
        });
        calls.push(call(out, &place, b"ai_place", &[atom]));
    }
    let body = call(out, &begin, b"begin", &calls);
    let i = out.scripts.len();
    out.scripts.push(definition(name, 0, h2t::VOID, body, i)?);
    if out.nodes.len() > MAX_NODES {
        return Err(invalid(format!(
            "{} nodes, Halo 2 holds {MAX_NODES}",
            out.nodes.len()
        )));
    }
    Ok(())
}

fn definition(name: &str, a: u16, b: u16, root: Datum, i: usize) -> Result<[u8; 40]> {
    if name.len() >= 32 {
        return Err(invalid(format!("definition {i}: name too long")));
    }
    let mut out = [0u8; 40];
    out[..name.len()].copy_from_slice(name.as_bytes());
    out[32..34].copy_from_slice(&a.to_le_bytes());
    out[34..36].copy_from_slice(&b.to_le_bytes());
    out[36..40].copy_from_slice(&root.0.to_le_bytes());
    Ok(out)
}

struct Compiler<'a> {
    t: &'a Tables,
    h1: &'a Catalog,
    table: &'a Table,
    h2: &'a Target,
    policy: Missing,
    cutscenes: Cutscenes,
    resolves: &'a dyn Fn(&str, &str) -> bool,
    out: Compiled,
    interned: HashMap<Vec<u8>, u32>,
    definition: String,
    conversations: Option<conversations::Lowering>,
    conversation_heads: HashMap<u16, Datum>,
}

/// A node before its slot is known to its parent: its fields.
struct New {
    opcode: u16,
    ty: u16,
    flags: u16,
    string: u32,
    data: u32,
}

impl Compiler<'_> {
    fn ty(&self, h1: u16) -> Result<u16> {
        self.table.ty(h1).ok_or_else(|| {
            invalid(format!(
                "{}: Halo 2 has no type {}",
                self.definition,
                self.h1.type_name(h1).unwrap_or("?")
            ))
        })
    }

    fn intern(&mut self, bytes: &[u8]) -> u32 {
        if let Some(&at) = self.interned.get(bytes) {
            return at;
        }
        let at = self.out.strings.len() as u32;
        self.out.strings.extend_from_slice(bytes);
        self.out.strings.push(0);
        self.interned.insert(bytes.to_vec(), at);
        at
    }

    /// Appends a node with no sibling; returns its datum.
    fn push(&mut self, n: New) -> Datum {
        let slot = self.out.nodes.len();
        let salt = 0x8000 | (slot as u16 & 0x7fff);
        let mut b = [0u8; Node::SIZE];
        b[0..2].copy_from_slice(&salt.to_le_bytes());
        b[2..4].copy_from_slice(&n.opcode.to_le_bytes());
        b[4..6].copy_from_slice(&n.ty.to_le_bytes());
        b[6..8].copy_from_slice(&n.flags.to_le_bytes());
        b[8..12].copy_from_slice(&Datum::NULL.0.to_le_bytes());
        b[12..16].copy_from_slice(&n.string.to_le_bytes());
        b[16..20].copy_from_slice(&n.data.to_le_bytes());
        self.out.nodes.push(Node::decode(&b).expect("20 bytes"));
        Datum(u32::from(salt) << 16 | slot as u32)
    }

    fn set_next(&mut self, d: Datum, next: Datum) {
        let mut b = self.out.nodes[d.slot()].encode();
        b[8..12].copy_from_slice(&next.0.to_le_bytes());
        self.out.nodes[d.slot()] = Node::decode(&b).expect("20 bytes");
    }

    fn set_type(&mut self, d: Datum, ty: u16) {
        let mut b = self.out.nodes[d.slot()].encode();
        b[4..6].copy_from_slice(&ty.to_le_bytes());
        self.out.nodes[d.slot()] = Node::decode(&b).expect("20 bytes");
    }

    /// A call: the call node, its head atom, then the arguments chained.
    fn call(&mut self, opcode: u16, flags: u16, ty: u16, name: &[u8], args: &[Datum]) -> Datum {
        let string = self.intern(name);
        let head = self.push(New {
            opcode,
            ty: h2t::FUNCTION_NAME,
            flags: ATOM,
            string,
            data: 0,
        });
        let mut prev = head;
        for &a in args {
            self.set_next(prev, a);
            prev = a;
        }
        self.push(New {
            opcode,
            ty,
            flags,
            string: 0,
            data: head.0,
        })
    }

    /// A literal of `ty`: the type's zero, or `none` (hooked when `token`).
    fn literal(&mut self, ty: u16, source: u16, bits: u32, token: &[u8]) -> Datum {
        let string = self.intern(token);
        self.push(New {
            opcode: source,
            ty,
            flags: ATOM,
            string,
            data: bits,
        })
    }

    fn default(&mut self, ty: u16) -> Datum {
        match ty {
            h2t::VOID => self.literal(ty, h2t::VOID, 0, b""),
            h2t::BOOLEAN => self.literal(ty, ty, 0, b"false"),
            h2t::REAL => self.literal(ty, ty, 0, b"0"),
            h2t::SHORT | h2t::LONG => self.literal(ty, ty, 0, b"0"),
            h2t::STRING => {
                let at = self.intern(b"");
                self.push(New {
                    opcode: ty,
                    ty,
                    flags: ATOM,
                    string: at,
                    data: at,
                })
            }
            _ => self.literal(ty, ty, NONE, b"none"),
        }
    }

    /// `v` as a literal of `ty` (boolean, real, short or long).
    fn number(&mut self, ty: u16, v: i32) -> Datum {
        let text = v.to_string();
        match ty {
            h2t::BOOLEAN => {
                let t: &[u8] = if v != 0 { b"true" } else { b"false" };
                self.literal(ty, ty, u32::from(v != 0), t)
            }
            h2t::REAL => self.literal(ty, ty, (v as f32).to_bits(), text.as_bytes()),
            h2t::SHORT => self.literal(ty, ty, u32::from(v as u16), text.as_bytes()),
            h2t::LONG => self.literal(ty, ty, v as u32, text.as_bytes()),
            _ => self.default(ty),
        }
    }

    fn hook(&mut self, d: Datum, h2_type: u16, h1_type: u16, token: &str) {
        self.out.hooks.push(Hook {
            slot: d.slot() as u16,
            h2_type,
            type_name: self
                .h2
                .types
                .get(h2_type as usize)
                .cloned()
                .unwrap_or_default(),
            h1_type: self.h1.type_name(h1_type).unwrap_or("?").to_owned(),
            token: token.to_owned(),
        });
    }

    /// The short path [`SHORT_PATHS`] gives for script `name`, and its
    /// index, when the level has it.
    fn short_path(&self, name: &[u8]) -> Option<(&'static str, u16)> {
        let &(_, to) = SHORT_PATHS.iter().find(|p| p.0.as_bytes() == name)?;
        let i = self.t.scripts.iter().position(|s| s.name == to)?;
        Some((to, u16::try_from(i).ok()?))
    }

    /// An animation name (a string literal right after an animation graph
    /// literal, as in `custom_animation`, `scenery_animation_start` and
    /// `camera_set_animation`) names an animation of that graph, which the
    /// converter renames. Its hook gets [`ANIMATION_NAME`] as its `h1_type`
    /// and `<graph>|<animation>` as its token. `compiled[k]` is `args[k]`
    /// compiled.
    fn mark_animation_names(&mut self, args: &[Datum], compiled: &[Datum]) -> Result<()> {
        for k in 1..args.len().min(compiled.len()) {
            let (g, a) = (*self.t.node(args[k - 1])?, *self.t.node(args[k])?);
            let is = |n: &Node, ty: &str| {
                n.atom() && !n.global() && self.h1.type_name(n.opcode()).ok() == Some(ty)
            };
            if !is(&g, "animation_graph") || !is(&a, "string") {
                continue;
            }
            let token = format!(
                "{}|{}",
                self.t.string(g.string_offset())?,
                self.t.string(a.string_offset())?
            );
            let slot = compiled[k].slot() as u16;
            if let Some(h) = self.out.hooks.iter_mut().find(|h| h.slot == slot) {
                h.h1_type = ANIMATION_NAME.to_owned();
                h.token = token;
            }
        }
        Ok(())
    }

    /// The first of `args` that is an atom of an [`INDEXED`] type (or, for
    /// a [`NONE_UNSAFE`] builtin, an object name) which will not resolve:
    /// its Halo 2 type name and token.
    fn unindexed_argument(
        &self,
        h2_name: &str,
        args: &[Datum],
    ) -> Result<Option<(String, String)>> {
        let none_unsafe = NONE_UNSAFE.contains(&h2_name);
        for &a in args {
            let n = self.t.node(a)?;
            if !n.atom() || n.global() {
                continue;
            }
            let Some(ty) = self
                .table
                .ty(n.opcode())
                .and_then(|t| self.h2.types.get(t as usize))
            else {
                continue;
            };
            if !INDEXED.contains(&ty.as_str())
                && !(none_unsafe && OBJECT_NAME_TYPES.contains(&ty.as_str()))
            {
                continue;
            }
            let token = self.t.string(n.string_offset())?;
            let captured = none_unsafe
                && OBJECT_NAME_TYPES.contains(&ty.as_str())
                && self
                    .conversations
                    .as_ref()
                    .is_some_and(|l| l.has_alias(token));
            if !captured && !(self.resolves)(ty, token) {
                return Ok(Some((ty.clone(), token.to_owned())));
            }
        }
        Ok(None)
    }

    /// Halo 1's `(ai_status x)` on Halo 2's `ai_combat_status`, as Halo 1's
    /// scale ([`STATUS_LADDER`]): `(if (>= (ai_combat_status x)
    /// ai_combat_status_visible) 6 (if ... 0))`. `None` when Halo 2's
    /// builtins or engine globals for it are not known.
    fn status_ladder(&mut self, args: &[Datum], expected: u16) -> Result<Option<Datum>> {
        let (Some(status), Some(ge), Some(iff), [arg]) = (
            self.h2.builtins.get("ai_combat_status").cloned(),
            self.h2.builtins.get(">=").cloned(),
            self.h2.builtins.get("if").cloned(),
            args,
        ) else {
            return Ok(None);
        };
        let mut globals = Vec::new();
        for &(name, v) in STATUS_LADDER {
            let Some(&(index, ty)) = self.h2.externals.get(name) else {
                return Ok(None);
            };
            globals.push((name, index, ty, v));
        }
        let ai = status.parameters[0];
        let mut out = self.number(expected, 0);
        for &(name, index, ty, v) in globals.iter().rev() {
            let x = self.expr(*arg, ai)?;
            let s = self.call(status.index, CALL, h2t::SHORT, b"ai_combat_status", &[x]);
            let string = self.intern(name.as_bytes());
            let g = self.push(New {
                opcode: ty,
                ty: h2t::SHORT,
                flags: GLOBAL,
                string,
                data: u32::from(index | 0x8000),
            });
            let cond = self.call(ge.index, CALL, h2t::BOOLEAN, b">=", &[s, g]);
            let then = self.number(expected, v);
            out = self.call(iff.index, CALL, expected, b"if", &[cond, then, out]);
        }
        Ok(Some(out))
    }

    /// Halo 1's `(op (ai_status x) k)` with `op` a comparison and `k` a
    /// number (every use in the ten levels), as one comparison of Halo 2's
    /// `ai_combat_status` with the [`STATUS_LADDER`] global for `k`: the
    /// same truth as [`Self::status_ladder`] at a tenth of its nodes (b40
    /// is at the tag memory bound). `None` when it does not have that shape.
    fn status_compare(&mut self, op: &str, args: &[Datum], expected: u16) -> Result<Option<Datum>> {
        let [a, b] = args else { return Ok(None) };
        let is_status = |c: &Self, d: Datum| -> Result<Option<Datum>> {
            let n = *c.t.node(d)?;
            if n.atom()
                || n.script_call()
                || c.h1
                    .function(n.opcode())
                    .map_or(true, |f| f.name != "ai_status")
            {
                return Ok(None);
            }
            let head = *c.t.node(Datum(n.data()))?;
            let arg = head.next();
            Ok((arg != Datum::NULL && c.t.node(arg)?.next() == Datum::NULL).then_some(arg))
        };
        let number = |c: &Self, d: Datum| -> Result<Option<i32>> {
            let n = *c.t.node(d)?;
            if !n.atom() || n.global() {
                return Ok(None);
            }
            // The value, not the token: a literal's string offset is not
            // always in the table.
            let Ok(text) = crate::decompile::literal(c.t, c.h1, &n) else {
                return Ok(None);
            };
            let v = text.trim().parse::<f32>().ok();
            Ok(v.filter(|v| v.fract() == 0.0).map(|v| v as i32))
        };
        let (arg, k, op) = match (is_status(self, *a)?, number(self, *b)?) {
            (Some(x), Some(k)) => (x, k, op),
            _ => match (number(self, *a)?, is_status(self, *b)?) {
                (Some(k), Some(x)) => (
                    x,
                    k,
                    match op {
                        "<" => ">",
                        ">" => "<",
                        "<=" => ">=",
                        ">=" => "<=",
                        o => o,
                    },
                ),
                _ => return Ok(None),
            },
        };
        // Halo 1's status is at least `k` exactly when Halo 2's reaches the
        // ladder's global for `k`.
        let (k, at_least) = match op {
            ">=" => (k, true),
            ">" => (k + 1, true),
            "<" => (k, false),
            "<=" => (k + 1, false),
            "=" => {
                let (Some(lo), Some(hi), Some(and)) = (
                    self.status_threshold(arg, k, true, h2t::BOOLEAN)?,
                    self.status_threshold(arg, k + 1, false, h2t::BOOLEAN)?,
                    self.h2.builtins.get("and").cloned(),
                ) else {
                    return Ok(None);
                };
                return Ok(Some(self.call(
                    and.index,
                    CALL,
                    expected,
                    b"and",
                    &[lo, hi],
                )));
            }
            _ => return Ok(None),
        };
        self.status_threshold(arg, k, at_least, expected)
    }

    /// `(>= (ai_combat_status x) G)` (or `<` when not `at_least`) for the
    /// [`STATUS_LADDER`] global of Halo 1 status `k`; a constant outside
    /// the scale.
    fn status_threshold(
        &mut self,
        arg: Datum,
        k: i32,
        at_least: bool,
        ty: u16,
    ) -> Result<Option<Datum>> {
        if k <= 0 || k > 6 {
            return Ok(Some(self.number(ty, i32::from((k <= 0) == at_least))));
        }
        let name = if at_least { ">=" } else { "<" };
        let (Some(status), Some(cmp), Some(&(global, _))) = (
            self.h2.builtins.get("ai_combat_status").cloned(),
            self.h2.builtins.get(name).cloned(),
            STATUS_LADDER.iter().find(|r| r.1 == k),
        ) else {
            return Ok(None);
        };
        let Some(&(index, gty)) = self.h2.externals.get(global) else {
            return Ok(None);
        };
        let x = self.expr(arg, status.parameters[0])?;
        let c = self.call(status.index, CALL, h2t::SHORT, b"ai_combat_status", &[x]);
        let string = self.intern(global.as_bytes());
        let g = self.push(New {
            opcode: gty,
            ty: h2t::SHORT,
            flags: GLOBAL,
            string,
            data: u32::from(index | 0x8000),
        });
        Ok(Some(self.call(
            cmp.index,
            CALL,
            ty,
            name.as_bytes(),
            &[c, g],
        )))
    }

    /// Compiles node `d` where its parent expects Halo 2 type `expected`.
    fn expr(&mut self, d: Datum, expected: u16) -> Result<Datum> {
        let n = *self.t.node(d)?;
        if n.atom() {
            return self.atom(&n, expected);
        }
        let head = *self.t.node(Datum(n.data()))?;
        let token = self.t.string(head.string_offset())?.to_owned();
        let mut args = Vec::new();
        let mut a = head.next();
        while a != Datum::NULL {
            args.push(a);
            a = self.t.node(a)?.next();
        }
        if n.script_call() {
            if self.cutscenes == Cutscenes::Skip && token == SKIP_GUARD {
                return Ok(self.literal(expected, h2t::BOOLEAN, 0, b"false"));
            }
            return Ok(self.call(n.opcode(), SCRIPT_CALL, expected, token.as_bytes(), &[]));
        }
        let index = n.opcode();
        let catalog = self.h1;
        let f = catalog.function(index)?;
        if let Some(d) = conversations::translate(self, &f.name, &args, expected)? {
            return Ok(d);
        }
        let tr = self
            .table
            .builtin(index)
            .ok_or_else(|| invalid("no translation"))?;
        if matches!(f.name.as_str(), "=" | "<" | ">" | "<=" | ">=")
            && let Some(d) = self.status_compare(&f.name, &args, expected)?
        {
            return Ok(d);
        }
        if f.name == "ai_status"
            && let Some(d) = self.status_ladder(&args, expected)?
        {
            return Ok(d);
        }
        if f.name == "sleep"
            && let Some(d) = self.sleep_forever(&args, expected)?
        {
            return Ok(d);
        }
        let Some((h2_name, b)) = tr.target.clone().filter(|_| tr.kind != Kind::Missing) else {
            self.out.missing.push(Unresolved {
                definition: self.definition.clone(),
                name: f.name.clone(),
            });
            return match self.policy {
                Missing::Stub => Ok(match DONE_VALUES.iter().find(|d| d.0 == f.name) {
                    Some(&(_, v)) => self.number(expected, v),
                    None => self.default(expected),
                }),
                Missing::Extension => {
                    let fixed = self.h1.fixed_signature(index);
                    let mut out = Vec::new();
                    for (k, &a) in args.iter().enumerate() {
                        let h1_ty = if fixed {
                            f.parameters[k]
                        } else {
                            self.t.node(a)?.value_type()
                        };
                        let ty = self.table.ty(h1_ty).unwrap_or(h2t::PASSTHROUGH);
                        out.push(self.expr(a, ty)?);
                    }
                    Ok(self.call(
                        EXTENSION_BASE + index,
                        CALL,
                        expected,
                        f.name.as_bytes(),
                        &out,
                    ))
                }
            };
        };
        if let Some((ty, token)) = self.unindexed_argument(&h2_name, &args)? {
            self.out.unindexed.push(Unresolved {
                definition: self.definition.clone(),
                name: format!("{h2_name} ({ty} {token})"),
            });
            return Ok(self.default(expected));
        }
        let mut out = Vec::new();
        match tr.adapter {
            Some(Adapter::Negate) => {
                for &a in &args {
                    let ty = self.ty(self.t.node(a)?.value_type())?;
                    out.push(self.expr(a, ty)?);
                }
                let not = self
                    .h2
                    .builtins
                    .get("not")
                    .ok_or_else(|| invalid("Halo 2 `not` unknown"))?
                    .clone();
                let inner = self.call(b.index, CALL, not.parameters[0], h2_name.as_bytes(), &out);
                return Ok(self.call(not.index, CALL, expected, b"not", &[inner]));
            }
            Some(Adapter::PrependNone) => {
                let first = self.literal(b.parameters[0], b.parameters[0], NONE, b"none");
                out.push(first);
            }
            _ => {}
        }
        let offset = out.len();
        for (k, &a) in args.iter().enumerate() {
            let ty = if b.special {
                self.ty(self.t.node(a)?.value_type())?
            } else {
                *b.parameters
                    .get(offset + k)
                    .ok_or_else(|| invalid(format!("{token}: too many arguments for {h2_name}")))?
            };
            out.push(self.expr(a, ty)?);
        }
        if let Some(Adapter::AppendShort(v)) = tr.adapter {
            let ty = *b
                .parameters
                .get(out.len())
                .ok_or_else(|| invalid("adapter arity"))?;
            let d = self.literal(ty, h2t::SHORT, v as u16 as u32, v.to_string().as_bytes());
            self.hook(d, h2t::SHORT, types::SHORT, &format!("{h2_name}:{v}"));
            out.push(d);
        }
        self.mark_animation_names(&args, &out[offset..])?;
        let optional = usize::from(tr.adapter == Some(Adapter::OmitOptional));
        if !b.special && out.len() + optional != b.parameters.len() {
            return Err(invalid(format!(
                "{token}: {} arguments for {h2_name}",
                out.len()
            )));
        }
        let call = self.call(b.index, CALL, expected, h2_name.as_bytes(), &out);
        if NONE_UNSAFE.contains(&h2_name.as_str())
            && let &[arg] = &args[..]
            && let Some(guard) = self.none_guard(arg)?
        {
            let iff = self
                .h2
                .builtins
                .get("if")
                .ok_or_else(|| invalid("Halo 2 `if` unknown"))?
                .clone();
            self.out.guarded.push(Unresolved {
                definition: self.definition.clone(),
                name: h2_name.clone(),
            });
            return Ok(self.call(iff.index, CALL, expected, b"if", &[guard, call]));
        }
        Ok(call)
    }

    /// Halo 1's `(sleep -1)` sleeps until a `wake` (S: Halo 1's scripting
    /// reference; a10's continuous `dialog_flavor_captain` is eight
    /// `(sleep -1)`-separated lines that `flavor_intercom` wakes one at a
    /// time, and a50's `kill_all_continuous` is `(sleep -1 <script>)` once
    /// per continuous script). Halo 2's `sleep` takes a negative duration
    /// as no sleep at all (E `0x002087e0`, docs/engine/hs_vm.md), so the
    /// call fell through: a continuous script restarted every tick and
    /// a10's captain started all eight lines thirty times a second, a
    /// loud whirr over the whole level and its cutscenes. Halo 2's
    /// `sleep_forever` (`0x00208750`: wake -2 on this thread, or on the
    /// script its optional argument names; `wake` resumes after it) is the
    /// same thing. `None` unless the duration is a negative literal and
    /// Halo 2 has `sleep_forever`.
    fn sleep_forever(&mut self, args: &[Datum], expected: u16) -> Result<Option<Datum>> {
        let Some((&duration, rest)) = args.split_first() else {
            return Ok(None);
        };
        let n = *self.t.node(duration)?;
        let negative = n.atom()
            && !n.global()
            && match self.h1.type_name(n.opcode()) {
                Ok("short") => (n.data() as u16 as i16) < 0,
                Ok("long") => (n.data() as i32) < 0,
                _ => false,
            };
        let Some(b) = self.h2.builtins.get("sleep_forever").cloned() else {
            return Ok(None);
        };
        if !negative || rest.len() > 1 {
            return Ok(None);
        }
        let mut out = Vec::new();
        for &a in rest {
            let ty = self.ty(self.t.node(a)?.value_type())?;
            out.push(self.expr(a, ty)?);
        }
        self.out.forever.push(Unresolved {
            definition: self.definition.clone(),
            name: "sleep".into(),
        });
        Ok(Some(self.call(
            b.index,
            CALL,
            expected,
            b"sleep_forever",
            &out,
        )))
    }

    /// `(> (list_count x) 0)`: true when Halo 1 argument `arg` is an
    /// object ([`NONE_UNSAFE`]). `None` when Halo 2's builtins for it are
    /// not known.
    fn none_guard(&mut self, arg: Datum) -> Result<Option<Datum>> {
        let (Some(count), Some(gt)) = (
            self.h2.builtins.get("list_count").cloned(),
            self.h2.builtins.get(">").cloned(),
        ) else {
            return Ok(None);
        };
        let Some(&list) = count.parameters.first() else {
            return Ok(None);
        };
        let x = self.expr(arg, list)?;
        let n = self.call(count.index, CALL, h2t::SHORT, b"list_count", &[x]);
        let zero = self.number(h2t::SHORT, 0);
        Ok(Some(self.call(
            gt.index,
            CALL,
            h2t::BOOLEAN,
            b">",
            &[n, zero],
        )))
    }

    fn atom(&mut self, n: &Node, expected: u16) -> Result<Datum> {
        // Numeric atoms may point past the string data (discarded source).
        let token = || -> Result<Vec<u8>> { Ok(self.t.string_bytes(n.string_offset())?.to_vec()) };
        if n.global() {
            let token = token()?;
            let index = n.data() as u16;
            let name = String::from_utf8_lossy(&token).into_owned();
            if index & 0x8000 == 0 {
                let string = self.intern(&token);
                return Ok(self.push(New {
                    opcode: self.table.ty(n.opcode()).unwrap_or(n.opcode()),
                    ty: expected,
                    flags: GLOBAL,
                    string,
                    data: u32::from(index),
                }));
            }
            let Some(&(h2_index, h2_ty)) = self.h2.externals.get(&name) else {
                self.out.missing_globals.push(Unresolved {
                    definition: self.definition.clone(),
                    name,
                });
                return Ok(self.default(expected));
            };
            let string = self.intern(&token);
            return Ok(self.push(New {
                opcode: h2_ty,
                ty: expected,
                flags: GLOBAL,
                string,
                data: u32::from(h2_index | 0x8000),
            }));
        }
        let h1_source = n.opcode();
        let bits = n.data();
        let scalar = match h1_source {
            types::VOID => Some(h2t::VOID),
            types::BOOLEAN => Some(h2t::BOOLEAN),
            types::REAL => Some(h2t::REAL),
            types::SHORT => Some(h2t::SHORT),
            types::LONG => Some(h2t::LONG),
            types::SCRIPT => Some(h2t::SCRIPT),
            _ => None,
        };
        if let Some(source) = scalar {
            let text = match source {
                h2t::SCRIPT => token()?,
                _ => crate::decompile::literal(self.t, self.h1, n)?.into_bytes(),
            };
            if source == h2t::SCRIPT
                && self.cutscenes != Cutscenes::Play
                && let Some((to, i)) = self.short_path(&text)
            {
                let bits = bits & 0xffff_0000 | u32::from(i);
                return Ok(self.literal(expected, source, bits, to.as_bytes()));
            }
            return Ok(self.literal(expected, source, bits, &text));
        }
        let token = token()?;
        // A string stays a string only where the interpreter can cast one
        // (`hs::castable`: string, boolean, void, passthrough, the object
        // and name types); anywhere else it becomes a hook below.
        if h1_source == types::STRING && hs::castable(expected, h2t::STRING) {
            let at = self.intern(&token);
            return Ok(self.push(New {
                opcode: h2t::STRING,
                ty: expected,
                flags: ATOM,
                string: at,
                data: at,
            }));
        }
        // Symbolic: the converter supplies the value. A string where Halo 2
        // wants a string ID becomes a string ID literal; any other string
        // takes the expected type (b30: a seat name where Halo 2 wants a
        // `unit_seat_mapping`, which no cast makes from a string). The
        // source type is always one the interpreter casts to `expected`.
        let source = if h1_source == types::STRING {
            if expected == h2t::STRING_ID {
                h2t::STRING_ID
            } else {
                expected
            }
        } else {
            match self.table.ty(h1_source) {
                Some(t) if (t == expected || !is_scalar(expected)) && hs::castable(expected, t) => {
                    t
                }
                _ => expected,
            }
        };
        // A seat mapping is `count << 16 | first` into the scenario's
        // mapping table (retail: `0x10004` names one seat); 0 names none,
        // so an unresolved one moves nobody instead of indexing past it.
        let seats = self.h2.types.get(source as usize).map(String::as_str);
        let empty = if seats == Some("unit_seat_mapping") {
            0
        } else {
            NONE
        };
        let d = self.literal(expected, source, empty, &token);
        let text = String::from_utf8_lossy(&token).into_owned();
        self.hook(d, source, h1_source, &text);
        conversations::named_atom(self, expected, source, &text, d)
    }
}

fn is_scalar(t: u16) -> bool {
    (h2t::VOID..=h2t::STRING_ID).contains(&t)
}

impl Compiled {
    /// Writes each hook's value into its atom: `resolve` returns the data
    /// word for a hook, or `None` to leave `none`. Returns the hooks left.
    pub fn resolve_hooks(&mut self, mut resolve: impl FnMut(&Hook) -> Option<u32>) -> Vec<Hook> {
        let mut left = Vec::new();
        for h in std::mem::take(&mut self.hooks) {
            match resolve(&h) {
                Some(v) => {
                    let mut b = self.nodes[h.slot as usize].encode();
                    b[16..20].copy_from_slice(&v.to_le_bytes());
                    self.nodes[h.slot as usize] = Node::decode(&b).expect("20 bytes");
                }
                None => left.push(h.clone()),
            }
            self.hooks.push(h);
        }
        left
    }

    /// The tables as `map::build` blocks, installed into the scenario block
    /// `scnr` (fields per [`field`]). `reference` maps each Halo 1 reference
    /// to a Halo 2 group and tag; unresolved ones are dropped.
    pub fn install(
        &self,
        scnr: &mut Block,
        mut reference: impl FnMut(h1map::Group, &str) -> Option<(map::FourCC, map::TagId)>,
    ) -> Result<()> {
        if scnr.bytes.len() < field::MIN_LEN {
            return Err(invalid("scenario block too short for script fields"));
        }
        let nodes: Vec<u8> = self.nodes.iter().flat_map(|n| n.encode()).collect();
        scnr.block(field::NODES, self.nodes.len() as u32, Block::new(nodes));
        scnr.data(field::STRINGS, Block::new(self.strings.clone()));
        scnr.block(
            field::SCRIPTS,
            self.scripts.len() as u32,
            Block::new(self.scripts.concat()),
        );
        scnr.block(
            field::GLOBALS,
            self.globals.len() as u32,
            Block::new(self.globals.concat()),
        );
        let refs: Vec<_> = self
            .references
            .iter()
            .filter_map(|(g, name)| reference(*g, name))
            .collect();
        let mut block = Block::zeroed(refs.len() * 8);
        for (i, (group, id)) in refs.iter().enumerate() {
            block.bytes[i * 8..i * 8 + 4]
                .copy_from_slice(&u32::from_be_bytes(group.0).to_le_bytes());
            block.tag_id(i as u32 * 8 + 4, *id);
        }
        scnr.block(field::REFERENCES, refs.len() as u32, block);
        Ok(())
    }

    /// A minimal Halo 2 scenario payload holding only the script fields,
    /// laid out from `base`, for `hs::Scenario::decode`. References are
    /// written null.
    pub fn scenario_bytes(&self, base: u32) -> Vec<u8> {
        let mut out = vec![0u8; field::MIN_LEN];
        let table = |out: &mut Vec<u8>, at: u32, count: usize, bytes: &[u8]| {
            while !out.len().is_multiple_of(4) {
                out.push(0);
            }
            let address = if count == 0 {
                0
            } else {
                base + out.len() as u32
            };
            out[at as usize..at as usize + 4].copy_from_slice(&(count as u32).to_le_bytes());
            out[at as usize + 4..at as usize + 8].copy_from_slice(&address.to_le_bytes());
            out.extend_from_slice(bytes);
        };
        let nodes: Vec<u8> = self.nodes.iter().flat_map(|n| n.encode()).collect();
        table(&mut out, field::NODES, self.nodes.len(), &nodes);
        table(&mut out, field::STRINGS, self.strings.len(), &self.strings);
        table(
            &mut out,
            field::SCRIPTS,
            self.scripts.len(),
            &self.scripts.concat(),
        );
        table(
            &mut out,
            field::GLOBALS,
            self.globals.len(),
            &self.globals.concat(),
        );
        let refs: Vec<u8> = self
            .references
            .iter()
            .flat_map(|(g, _)| {
                let mut r = [0xffu8; 8];
                r[..4].copy_from_slice(&g.0.to_le_bytes());
                r
            })
            .collect();
        table(&mut out, field::REFERENCES, self.references.len(), &refs);
        out
    }
}
