//! Community game variants: classics the Halo 2 community played in custom
//! games (Zombies, SWAT, Jousting, Hide and Seek and so on), added to the
//! game's own list of built-in variants and marked with the "two players"
//! icon (U+E006) where the retail ones carry the padlock (U+E008).
//!
//! The design, the addresses and the sources are in
//! docs/game/community-gametypes.md and docs/game/rules-variants.md
//! ("Built-in list entries"). In short: the built-in variants are a block
//! in the `goof` tag of `mainmenu.map`, each a name, a game type, a list of
//! `(setting, value)` pairs (the choices a player makes in the editor) and a
//! description number. `0x149f88` turns every entry into a variant at load,
//! through the game's own setting code, and files it as a built-in, which
//! the game shows read-only and copies on edit. So this module only:
//!
//! 1. appends our entries to that block before `0x149f88` reads it,
//! 2. writes each listed script's `hx` block into its built-in record once
//!    `0x149f88` returns ([`write_blocks`]),
//! 3. serves our names and descriptions through the localized-string lookup
//!    `0x1a0180` (ids in a private range), and
//! 4. maps our description numbers to those ids where the variant list asks
//!    `0x19fd00` for a description.
//!
//! Each entry is built from a script in `rules::builtin::LISTED`
//! ([`Listed`]): its name, game type, `about` and `list` row, and the
//! record `variants::portable::record_for` makes of it. The entry carries
//! the record's value of every setting the editor offers
//! (`crate::game_variants::inject::pairs`, the store's rule); a script
//! with a value the editor does not offer is left out by name. Retail
//! entries are copied unchanged and saved variants are never touched.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use ::rules::types::Type;
use runtime::guest::Permissions;
use runtime::{Guest, TransferAbi, abi};
use variants::ext::{self, Ext};
use variants::record::{GameType, GameVariant};

// ---- guest addresses (hook points; see docs/game/community-gametypes.md) ----

/// Loads the built-in variants from the `goof` tag (`[esp+4]`).
const BUILTINS_LOAD: u32 = 0x0014_9f88;
/// Localized string of a `unic` tag: EAX = tag, EDI = string id, `[esp+4]`
/// = a 0x100-unit UTF-16 buffer. `ret 4`.
const LOCALIZED_STRING: u32 = 0x001a_0180;
/// Integer 0..=100 (ECX) to the string id of its decimal text in EAX (0
/// out of range). Plain `ret`.
const NUMBER_STRING_ID: u32 = 0x0019_fd00;
// The string and description hooks are marked context-only
// (`Dispatch::mark_context_only`): they read registers, the stack and the
// memory of the context they run on, write only the caller's buffer there,
// and otherwise run the original on that same context, so they act on
// whichever context calls them as the original would. `community:builtins`
// is not marked: it prints, and runs only when the variant list loads.
const STRINGS_HOOK: &str = "community:strings";
const DESCRIPTIONS_HOOK: &str = "community:descriptions";
/// The continuation that writes the listed records' blocks when
/// `0x149f88` returns.
const LOAD_RETURN: &str = "community: the listed records' blocks";

/// The return address of the variant list's description lookup, the one
/// `NUMBER_STRING_ID` call we answer for.
const LIST_DESCRIPTION_RETURN: u32 = 0x002c_a1fd;
/// The tag instance table: 16-byte entries, address at `+8`.
pub const TAG_INSTANCES: u32 = 0x004e_3b44;

// ---- goof layout (read from 0x149f88 and 0x2369b3) ----

/// `+0x18`: setting pages; each `+8` count, `+0xc` address of `sily` refs.
pub const GOOF_PAGES: u32 = 0x18;
pub const PAGE_SIZE: u32 = 0x18;
/// `+0x28`: built-in variants.
const GOOF_BUILTINS: u32 = 0x28;
const ENTRY_SIZE: u32 = 0x14;
/// `sily`: `+0` setting, `+0x1c` count, `+0x20` options of `0xc` bytes,
/// value at `+4`.
pub const SILY_OPTIONS: u32 = 0x1c;
pub const OPTION_SIZE: u32 = 0xc;

/// Our guest page: settings pairs, then the merged entry block.
pub const DATA_BASE: u32 = 0xa0fd_0000;
const DATA_SIZE: u32 = 0x4000;
/// The settings pairs' room: every listed entry writes one pair for each
/// setting of its type the editor offers, so this is most of the page. The
/// merged block, 101 entries at most, takes the rest.
const PAIRS_SIZE: u32 = 0x3800;
const ENTRIES_AT: u32 = DATA_BASE + PAIRS_SIZE;
const MAX_SLOTS: u32 = (DATA_SIZE - PAIRS_SIZE) / ENTRY_SIZE;
const _: () = assert!(MAX_SLOTS >= MAX_BUILTINS);
const _: () = assert!(MAX_ENTRIES as u32 <= MAX_SLOTS);

/// The icon after every community name, where retail names carry U+E008.
pub const ICON: char = '\u{e006}';

/// Our game type rows after Territories, in list order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Row {
    /// Party Games: the community's party modes, practice and the maps'
    /// tricks.
    PartyGames,
    Firefight,
    Siege,
}

impl Row {
    pub const ALL: [Row; 3] = [Row::PartyGames, Row::Firefight, Row::Siege];

    /// The custom row of this script type. Retail types use the existing rows.
    pub fn of_type(ty: Type) -> Option<Row> {
        Self::ALL.into_iter().find(|row| row.game_type() == ty)
    }

    pub fn game_type(self) -> Type {
        match self {
            Row::PartyGames => Type::Party,
            Row::Firefight => Type::Firefight,
            Row::Siege => Type::Siege,
        }
    }

    /// The row's number in a variant list screen's mark (0 is none).
    pub fn mark(self) -> u8 {
        match self {
            Row::PartyGames => 1,
            Row::Firefight => 2,
            Row::Siege => 3,
        }
    }

    pub fn from_mark(mark: u8) -> Option<Row> {
        Row::ALL.into_iter().find(|r| r.mark() == mark)
    }

    /// The label on the game type list.
    pub fn label(self) -> &'static str {
        self.game_type().row().label
    }

    /// The title over its pane and its variant list, the label in capitals
    /// as SLAYER is Slayer's.
    pub fn title(self) -> &'static str {
        match self {
            Row::PartyGames => "PARTY GAMES",
            Row::Firefight => "FIREFIGHT",
            Row::Siege => "SIEGE",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Row::Firefight => "Fight waves of AI enemies.",
            Row::Siege => "Capture the sites and break through the defenders' line.",
            Row::PartyGames => {
                "Zombies, jousting, superbounce practice: the party games and tricks the community made up for custom games."
            }
        }
    }

    fn ids_at(self) -> u32 {
        match self {
            Row::PartyGames => 0x7d,
            Row::Firefight => 0x74,
            Row::Siege => 0x77,
        }
    }
    pub fn label_id(self) -> u32 {
        ID_BASE | self.ids_at()
    }
    pub fn title_id(self) -> u32 {
        ID_BASE | (self.ids_at() + 1)
    }
    pub fn description_id(self) -> u32 {
        ID_BASE | (self.ids_at() + 2)
    }
    /// The text of one of the rows' ids.
    fn text(id: u32) -> Option<&'static str> {
        Row::ALL.into_iter().find_map(|r| {
            if id == r.label_id() {
                Some(r.label())
            } else if id == r.title_id() {
                Some(r.title())
            } else if id == r.description_id() {
                Some(r.description())
            } else {
                None
            }
        })
    }
}

// ---- the listed scripts ----

/// One listed script (`rules::builtin::LISTED`), compiled once: what its
/// list entry and its record's block come from
/// (docs/game/rules-variants.md, "Built-in list entries").
#[derive(Clone, Debug)]
pub struct Listed {
    /// `header.name`.
    pub name: String,
    /// `header.about`: the entry's description.
    pub about: String,
    /// The retail loader template's index in `GameType::ALL`. A script-index
    /// record loads through the Slayer template.
    pub list_type: u32,
    /// The category selected by the script, independent of its loader template.
    pub game_type: Type,
    /// The record of the script (`variants::portable::record_for`, the
    /// default options): its settings, and the `hx` block that names it.
    pub record: GameVariant,
}

impl Listed {
    fn of(source: &str) -> Result<Listed, String> {
        let ir = ::rules::compile(source).map_err(|e| format!("its script does not compile: {e}"))?;
        let record = variants::portable::record_for(&ir, &[])
            .map_err(|e| format!("its script makes no record: {e}"))?;
        let ty = record.game_type();
        let list_type = goof_type(ty)
            .ok_or_else(|| format!("engine {} has no list type", ty.engine()))?;
        Ok(Listed {
            name: ir.header.name.clone(),
            about: ir.header.about.clone(),
            list_type,
            game_type: ir.header.game_type,
            record,
        })
    }

    /// The name as the list shows it: the name, a space and the icon.
    pub fn display_name(&self) -> String {
        format!("{} {ICON}", self.name)
    }

    /// The `hx` block its built-in record gets ([`write_blocks`]).
    pub fn block(&self) -> [u8; ext::LEN] {
        *self.record.ext_block()
    }

    pub fn ext(&self) -> Ext {
        self.record.ext()
    }
}

/// The scripts a boot's packages list as built-in entries, in list order
/// (`rules::builtin::Registry::listed`, from the `variant` items of the
/// set), as many as the id ranges hold ([`MAX_ENTRIES`]).
static REGISTRY: OnceLock<::rules::builtin::Registry> = OnceLock::new();

/// Takes the set's scripts for the list. It must come before the first
/// read of [`scripts`]; `false` when the list was already read, or a set
/// was already given. With none given the list is the first-party
/// scripts' (`Registry::baked`), which the tests run on.
pub fn install_registry(registry: ::rules::builtin::Registry) -> bool {
    REGISTRY.set(registry).is_ok()
}

fn registry() -> &'static ::rules::builtin::Registry {
    REGISTRY.get_or_init(::rules::builtin::Registry::baked)
}

/// The script a word names: a script's key (`siege`, `gun_game`) or its
/// name (`Gun Game`), in any case, among every script of the boot's set
/// but the retail ports (a sim match's `[rules] script`,
/// docs/runtime/sim.md, "Rules scripts").
pub fn script_named(word: &str) -> Option<&'static ::rules::builtin::Script> {
    registry().scripts().iter().find(|s| {
        s.role != ::rules::builtin::Role::Retail
            && (s.key.eq_ignore_ascii_case(word) || s.name.eq_ignore_ascii_case(word))
    })
}

/// Every key [`script_named`] takes, in the set's order.
pub fn script_keys() -> Vec<&'static str> {
    registry()
        .scripts()
        .iter()
        .filter(|s| s.role != ::rules::builtin::Role::Retail)
        .map(|s| s.key.as_str())
        .collect()
}

/// The most list entries: the description numbers between
/// [`DESCRIPTION_BASE`] and the game-variant store's, and the name ids
/// below our rows' (`0x74`).
pub const MAX_ENTRIES: usize =
    (crate::game_variants::inject::DESCRIPTION_BASE - DESCRIPTION_BASE) as usize;
const _: () = assert!(MAX_ENTRIES <= 0x74);
const SCRIPT_SLOTS: usize = MAX_ENTRIES - ::firefight::builtin::SETS.len() - ::firefight::builtin::MAP_SETS.len() - 1;
const _: () = assert!(::rules::builtin::LISTED.len() <= SCRIPT_SLOTS);

/// The listed scripts, by their index in the list: the entry's name, its
/// engine and its source.
pub fn scripts() -> &'static [::rules::builtin::Script] {
    static SCRIPTS: OnceLock<Vec<::rules::builtin::Script>> = OnceLock::new();
    SCRIPTS.get_or_init(|| {
        let r = registry();
        for p in &r.problems {
            eprintln!("community variants: {p}");
        }
        let all: Vec<_> = r.listed().cloned().collect();
        if all.len() > SCRIPT_SLOTS {
            eprintln!(
                "community variants: {} entries, the list holds {SCRIPT_SLOTS} scripts: the rest are left out",
                all.len()
            );
        }
        all.into_iter().take(SCRIPT_SLOTS).collect()
    })
}

/// Every listed script, by its index in [`scripts`]: the entry's parts, or
/// why it has none (named in the log at each load).
pub fn listed() -> &'static [Result<Listed, String>] {
    static LISTED: OnceLock<Vec<Result<Listed, String>>> = OnceLock::new();
    LISTED.get_or_init(|| {
        let waves = ::firefight::builtin::variants().iter().map(|wave| {
            let def = &wave.def;
            let record = GameVariant::firefight(&def.name, def.teams, wave.prefix)
                .map_err(|e| e.to_string())?;
            Ok(Listed {
                about: format!("Fight the {} wave set.", def.name),
                name: def.name.clone(),
                list_type: goof_type(record.game_type()).unwrap(),
                game_type: Type::Firefight,
                record,
            })
        });
        // Map Waves, last: it names no set of its own, the runtime plays
        // the running map variant's (docs/game/forge-firefight.md,
        // "Playing it").
        let own = std::iter::once_with(|| {
            let name = ::firefight::builtin::MAP_WAVES_NAME;
            let record = GameVariant::firefight(name, 1, ::firefight::builtin::MAP_WAVES)
                .map_err(|e| e.to_string())?;
            Ok(Listed {
                about: "Fight the wave set the map variant carries.".to_owned(),
                name: name.to_owned(),
                list_type: goof_type(record.game_type()).unwrap(),
                game_type: Type::Firefight,
                record,
            })
        });
        scripts().iter().map(|b| Listed::of(&b.source)).chain(waves).chain(own).collect()
    })
}

/// The listed script named `name` (its `header.name`).
pub fn listed_named(name: &str) -> Option<&'static Listed> {
    listed().iter().flatten().find(|l| l.name == name)
}

/// The listed script whose built-in this list name is (the name as the
/// list holds it, with our icon).
fn listed_by_display(name: &str) -> Option<&'static Listed> {
    listed_named(name.strip_suffix(ICON)?.strip_suffix(' ')?)
}

/// The retail template the goof loader can construct. After construction
/// index-6 records receive their script index and empty type area.
pub fn goof_type(ty: GameType) -> Option<u32> {
    GameType::ALL.iter().position(|t| *t == ty.menu_template()).map(|i| i as u32)
}

/// Script references take precedence over the displayed name. The latter
/// identifies a listed built-in while the goof loader is constructing it.
pub fn script_type(name: &str, block: &[u8; ext::LEN]) -> Option<Type> {
    let script = Ext::in_effect(block).script;
    match script.mode {
        ext::Mode::Rules => crate::rules::header::script_type(script),
        ext::Mode::Firefight => Some(Type::Firefight),
        ext::Mode::None => listed_by_display(name).map(|l| l.game_type),
        ext::Mode::Unknown(_) => None,
    }
}

/// The type shown by Change Rules, including renamed saves. With no local
/// script, a record stays under its physical template.
pub fn filed_type(name: &str, block: &[u8; ext::LEN], engine: GameType) -> Option<Type> {
    let fallback = Type::of_engine(u8::try_from(engine.menu_template().engine()).ok()?)?;
    Some(script_type(name, block).unwrap_or(fallback))
}

#[cfg(test)]
pub(crate) fn row_of(name: &str, block: &[u8; ext::LEN]) -> Option<Row> {
    script_type(name, block).and_then(Row::of_type)
}

// ---- guest memory seen by the logic ----

pub trait Mem {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool;
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool;
    fn u32(&self, a: u32) -> Option<u32> {
        let mut b = [0; 4];
        self.rd(a, &mut b).then_some(u32::from_le_bytes(b))
    }
    fn set_u32(&mut self, a: u32, v: u32) -> bool {
        self.wr(a, &v.to_le_bytes())
    }
}

impl Mem for Guest {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        self.read_bytes(addr, out, 1).is_ok()
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        self.write_bytes(addr, data, 1).is_ok()
    }
}

// ---- strings ----

/// Our string ids: a private index range no map uses, with a top byte of
/// 0. The ids reach the lookup exactly as written: [`name_id`] into the
/// `goof` entries (read back whole by `0x149f88` and the list) and
/// [`description_id`] from [`description_for`]. So the claim is on the
/// whole id: another family of ours may share the index with another top
/// byte (`crate::string_ids` checks every family against every other).
const ID_BASE: u32 = crate::ui_kit::ids::family("community variants").base;
pub fn is_ours(id: u32) -> bool {
    id & 0xffff_ff00 == ID_BASE
}
/// The name id of listed script `i` (its index in [`scripts`]).
pub fn name_id(i: usize) -> u32 {
    ID_BASE | i as u32
}
pub fn description_id(i: usize) -> u32 {
    ID_BASE | 0x80 | i as u32
}
/// Our rows' texts (`crate::tweakers`, [`Row`]): each row's label, the
/// title of its pane and of its variant list, and their description. At
/// the top of the name range (`0x74` to `0x7f`), past every listed
/// script's.
pub fn row_ids() -> impl Iterator<Item = u32> {
    Row::ALL
        .into_iter()
        .flat_map(|r| [r.label_id(), r.title_id(), r.description_id()])
}

/// The text for one of our ids: a listed script's name and icon, or its
/// `about`.
pub fn text(id: u32) -> Option<String> {
    if !is_ours(id) {
        return None;
    }
    if let Some(t) = Row::text(id) {
        return Some(t.into());
    }
    let l = listed().get((id & 0x7f) as usize)?.as_ref().ok()?;
    Some(if id & 0x80 == 0 {
        l.display_name()
    } else {
        l.about.clone()
    })
}

/// Description numbers: retail uses 0 to 38 of the 0 to 100 the game
/// accepts, the game-variant store 94 to 100
/// ([`crate::game_variants::inject::DESCRIPTION_BASE`]); listed script `i`
/// has `DESCRIPTION_BASE + i`, 40 to 93 at most.
pub const DESCRIPTION_BASE: u32 = 40;
/// The id the variant list should show for description number `n`, if it
/// is one of ours.
pub fn description_for(n: u32) -> Option<u32> {
    let i = n.checked_sub(DESCRIPTION_BASE)? as usize;
    (i < listed().len()).then(|| description_id(i))
}

/// What `0x19fd00` returns instead of the original, given its return
/// address and number: our description id for the variant list only.
pub fn description_answer(ret: u32, n: u32) -> Option<u32> {
    if ret == LIST_DESCRIPTION_RETURN {
        description_for(n).or_else(|| crate::game_variants::inject::description_for(n))
    } else {
        None
    }
}

/// `built_in_variant` in `mainmenu.map`'s strings, "Built-in Variant
/// (cannot edit)": the line the variant pickers (`mp_variant_select` and
/// `mp_variant_select_lobby`, their widget text at `+0x1a0`) show under a
/// built-in. Every entry of the list is a built-in to the game, so we
/// answer it with where the focused one comes from ([`origin_text`]).
pub const BUILT_IN_LINE_ID: u32 = 0x1000_01dc;

/// The description number the variant list last asked for (`0x19fd00`
/// from `0x2ca1fd`): the focused entry's. `u32::MAX` before any.
static FOCUSED_DESCRIPTION: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(u32::MAX);

/// Where a built-in with description number `n` comes from: the store's
/// (94 to 100, [`crate::game_variants::inject::DESCRIPTION_BASE`]), ours
/// (from 40, [`DESCRIPTION_BASE`], one a listed script) or retail's (0 to
/// 38, and anything else).
pub fn origin_text(n: u32) -> &'static str {
    use crate::game_variants::inject;
    if (inject::DESCRIPTION_BASE..=inject::SHARED_DESCRIPTION).contains(&n) {
        "Downloaded Content"
    } else if description_for(n).is_some() {
        "Community Classic"
    } else {
        "Bungie Original"
    }
}

/// The text for [`BUILT_IN_LINE_ID`]: the focused entry's origin, once
/// the list has asked for a description.
fn built_in_line(id: u32) -> Option<String> {
    if id != BUILT_IN_LINE_ID {
        return None;
    }
    let n = FOCUSED_DESCRIPTION.load(std::sync::atomic::Ordering::Relaxed);
    (n != u32::MAX).then(|| origin_text(n).into())
}

/// The UTF-16 the lookup writes: at most 0xff units and a NUL.
pub fn utf16_buffer(s: &str) -> Vec<u8> {
    let mut units: Vec<u16> = s.encode_utf16().take(0xff).collect();
    units.push(0);
    units.iter().flat_map(|u| u.to_le_bytes()).collect()
}

// ---- the setting definitions the game offers ----

/// Every `(setting, value)` the editor offers, read from the `sily` tags
/// the `goof` tag's pages name.
pub fn offered<M: Mem>(m: &M, goof: u32, tags: u32) -> Option<HashMap<u32, Vec<i32>>> {
    let mut out = HashMap::<u32, Vec<i32>>::new();
    let (pages, at) = (m.u32(goof + GOOF_PAGES)?, m.u32(goof + GOOF_PAGES + 4)?);
    for p in 0..pages.min(0x100) {
        let page = at + p * PAGE_SIZE;
        let (n, refs) = (m.u32(page + 8)?, m.u32(page + 0xc)?);
        for r in 0..n.min(0x100) {
            let datum = m.u32(refs + r * 8 + 4)?;
            if datum == u32::MAX {
                continue;
            }
            let sily = m.u32(tags + (datum & 0xffff) * 16 + 8)?;
            let setting = m.u32(sily)?;
            let (k, opts) = (m.u32(sily + SILY_OPTIONS)?, m.u32(sily + SILY_OPTIONS + 4)?);
            let values = out.entry(setting).or_default();
            for o in 0..k.min(0x100) {
                values.push(m.u32(opts + o * OPTION_SIZE + 4)? as i32);
            }
        }
    }
    Some(out)
}

/// What the editor offered at the last load of the built-ins, kept for
/// screens that offer a setting's values where the `sily` tags are not
/// loaded (the pause menu's Quick Options, `crate::pause_change_map`).
static LAST_OFFERED: std::sync::Mutex<Option<HashMap<u32, Vec<i32>>>> =
    std::sync::Mutex::new(None);

/// Keeps `offer` as [`last_offered`]'s answer.
pub fn remember_offered(offer: &HashMap<u32, Vec<i32>>) {
    *LAST_OFFERED.lock().unwrap_or_else(|e| e.into_inner()) = Some(offer.clone());
}

/// Every `(setting, values)` the editor offered at the last load of the
/// built-ins; `None` before the first.
pub fn last_offered() -> Option<HashMap<u32, Vec<i32>>> {
    LAST_OFFERED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The listed scripts' entries, in [`scripts`]' order: each
/// its script's list type, its record's value of every setting of its
/// type the editor offers (`crate::game_variants::inject::pairs`) and its
/// description number. A script with a value the editor does not offer,
/// or with no record, is left out, named in the lines.
pub fn entries(offered: &HashMap<u32, Vec<i32>>) -> (Vec<NewEntry>, Vec<String>) {
    let mut out = Vec::new();
    let mut lines = Vec::new();
    for (i, l) in listed().iter().enumerate() {
        let name = l.as_ref().map(|l| l.name.as_str()).unwrap_or_else(|_| {
            scripts().get(i).map_or("Firefight", |b| b.name.as_str())
        });
        let entry = l.as_ref().map_err(String::clone).and_then(|l| {
            Ok(NewEntry {
                name_id: name_id(i),
                list_type: l.list_type,
                pairs: crate::game_variants::inject::pairs(&l.record, offered)?,
                description: DESCRIPTION_BASE + i as u32,
            })
        });
        match entry {
            Ok(e) => out.push(e),
            Err(why) => lines.push(format!("community variants: {name} left out: {why}")),
        }
    }
    (out, lines)
}

/// The most built-ins the content list takes (`0x217080`).
pub const MAX_BUILTINS: u32 = 101;

/// One entry to append: its name id, list type, `(setting, value)` pairs
/// and description number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewEntry {
    pub name_id: u32,
    pub list_type: u32,
    pub pairs: Vec<(u32, i32)>,
    pub description: u32,
}

impl NewEntry {
    fn encode(&self, pairs_at: u32) -> [u8; ENTRY_SIZE as usize] {
        let mut e = [0; ENTRY_SIZE as usize];
        e[0..4].copy_from_slice(&self.name_id.to_le_bytes());
        e[4..8].copy_from_slice(&self.list_type.to_le_bytes());
        e[8..12].copy_from_slice(&(self.pairs.len() as u32).to_le_bytes());
        e[12..16].copy_from_slice(&pairs_at.to_le_bytes());
        e[16..20].copy_from_slice(&self.description.to_le_bytes());
        e
    }
}

/// One group of entries and the page `[start, end)` their pairs go to.
pub struct Group<'a> {
    pub entries: &'a [NewEntry],
    pub start: u32,
    pub end: u32,
}

/// What [`extend_with`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Merged {
    /// Entries appended, by group.
    pub added: [usize; 2],
    /// Entries left out past [`MAX_BUILTINS`].
    pub over: usize,
}

/// Append `ours` to the `goof` tag's built-in list. The retail entries
/// are copied unchanged in front, into `base` (settings pairs) and
/// `entries` (the merged block); the tag then points at the merged block.
/// Does nothing (`Some(false)`) if the tag already points there. `None`
/// is a read or write failure, with the tag unchanged.
pub fn extend_builtins<M: Mem>(
    m: &mut M,
    goof: u32,
    ours: &[NewEntry],
    base: u32,
    entries: u32,
) -> Option<bool> {
    let groups = [Group {
        entries: ours,
        start: base,
        end: entries,
    }];
    extend_with(m, goof, &groups, entries).map(|d| d.added.iter().sum::<usize>() > 0)
}

/// [`extend_builtins`] for up to two groups (the community variants, then
/// the game-variant store's): retail, then each group in order, up to
/// [`MAX_BUILTINS`] in all, each group's pairs in its own page.
/// `Some(Merged::default())` when the tag already points at `entries` or
/// there is nothing to add.
pub fn extend_with<M: Mem>(
    m: &mut M,
    goof: u32,
    groups: &[Group<'_>],
    entries: u32,
) -> Option<Merged> {
    let (n, old) = (
        m.u32(goof + GOOF_BUILTINS)?,
        m.u32(goof + GOOF_BUILTINS + 4)?,
    );
    let total: usize = groups.iter().take(2).map(|g| g.entries.len()).sum();
    if old == entries || total == 0 {
        return Some(Merged::default());
    }
    let room = MAX_BUILTINS.saturating_sub(n) as usize;
    let take = total.min(room);
    if n + take as u32 > (DATA_BASE + DATA_SIZE - entries) / ENTRY_SIZE {
        return None;
    }
    let mut block = vec![0; (n * ENTRY_SIZE) as usize];
    if !m.rd(old, &mut block) {
        return None;
    }
    let mut merged = Merged {
        over: total - take,
        ..Merged::default()
    };
    let mut left = take;
    for (k, g) in groups.iter().enumerate().take(2) {
        let mut at = g.start;
        for e in g.entries.iter().take(left) {
            let pairs: Vec<u8> = e
                .pairs
                .iter()
                .flat_map(|(s, v)| s.to_le_bytes().into_iter().chain(v.to_le_bytes()))
                .collect();
            if at + pairs.len() as u32 > g.end || !m.wr(at, &pairs) {
                return None;
            }
            block.extend_from_slice(&e.encode(at));
            at += pairs.len() as u32;
            merged.added[k] += 1;
        }
        left -= merged.added[k];
    }
    if !m.wr(entries, &block) {
        return None;
    }
    let count = n + take as u32;
    // The block first, then the count that makes it visible.
    (m.set_u32(goof + GOOF_BUILTINS + 4, entries) && m.set_u32(goof + GOOF_BUILTINS, count))
        .then_some(merged)
}

// ---- the listed records' blocks ----

/// A listed record is identified by its display name and loader template.
/// An already restored script record also matches on a second load.
fn listed_record(m: &dyn guest_layout::memory::GuestMemory, record: u32) -> Option<&'static Listed> {
    let l = listed_by_display(&crate::tweakers::record_name(m, record)?)?;
    let ty = GameType::from_engine(crate::tweakers::record_engine(m, record)?);
    let want = l.record.game_type();
    (ty == want || ty == want.menu_template()).then_some(l)
}

/// Writes each listed script's `hx` block into its built-in record in the
/// content list (`[0x51ea14]`, `crate::tweakers::BUILTIN_LIST`), where it
/// is not there yet. The pairs cannot carry the block, so `0x149f88`
/// files the record without it; from then a game on the built-in, and a
/// copy Edit makes of it, name its script. How many changed.
pub fn write_blocks(
    m: &mut dyn guest_layout::memory::GuestMemory,
) -> Result<usize, guest_layout::memory::MemFault> {
    use crate::tweakers::{BUILTIN_LIST, BUILTIN_RECORD, BUILTIN_VARIANT};
    let list = m.u32(BUILTIN_LIST)?;
    if list == 0 {
        return Ok(0);
    }
    let n = m.u32(list)?.min(MAX_BUILTINS);
    let mut changed = 0;
    for i in 0..n {
        let record = list + 4 + i * BUILTIN_RECORD + BUILTIN_VARIANT;
        if crate::game_variants::inject::restore_listed(m, record)? {
            changed += 1;
            continue;
        }
        let Some(l) = listed_record(&*m, record) else {
            continue;
        };
        if restore_script_record(m, record, l.record.game_type(), &l.block())? {
            changed += 1;
        }
    }
    Ok(changed)
}

/// Finish the record built from the goof template. Keep the loader's
/// name, flags and selected settings, including the built-in icon.
pub(crate) fn restore_script_record(
    m: &mut dyn guest_layout::memory::GuestMemory,
    record: u32,
    ty: GameType,
    block: &[u8; ext::LEN],
) -> Result<bool, guest_layout::memory::MemFault> {
    let mut raw = [0; variants::record::RECORD_SIZE];
    m.read(record, &mut raw)?;
    let before = raw;
    finish_record(&mut raw, ty, block);
    if raw == before {
        return Ok(false);
    }
    m.write(record, &raw)?;
    Ok(true)
}

pub(crate) fn finish_record(
    raw: &mut [u8; variants::record::RECORD_SIZE],
    ty: GameType,
    block: &[u8; ext::LEN],
) {
    use variants::record::{ENGINE_AT, TYPE_AREA};
    if ty == GameType::Script {
        raw[ENGINE_AT..ENGINE_AT + 4].copy_from_slice(&ty.engine().to_le_bytes());
        raw[TYPE_AREA].fill(0);
    }
    raw[ext::AT..ext::AT + ext::LEN].copy_from_slice(block);
}

// ---- hooks ----

/// Set while the wrapper's own call of `0x149f88` is on its way: that
/// call goes straight to the original.
static IN_ORIGINAL: AtomicBool = AtomicBool::new(false);

/// The string lookup's hook: our ids' text into the caller's buffer, any
/// other id to the original, on the context it runs on (the render
/// thread's view there).
fn register_strings(
    d: &mut runtime::Dispatch,
    o: Arc<runtime::exec::OriginalHandler>,
    live: Option<Arc<live_hle::Hle>>,
) {
    d.register_override(LOCALIZED_STRING, STRINGS_HOOK, move |cpu, ctx, _| {
        let status = matchmaking_status_text(cpu.edi, || {
            live.as_ref().and_then(|hle| hle.matchmaking_status())
        });
        if let Some(t) = status
            .or_else(|| text(cpu.edi))
            .or_else(|| built_in_line(cpu.edi))
            .or_else(|| crate::game_variants::inject::text(cpu.edi))
            .or_else(|| crate::variant_options::text(cpu.edi))
            .or_else(|| crate::hud_kit::text(cpu.edi))
        {
            let mut mem = ctx.runtime().memory();
            let buf = mem.u32(cpu.esp + 4)?;
            mem.write_bytes(buf, &utf16_buffer(&t), 1)?;
            return mem.ret(cpu, 4);
        }
        Ok(o.invoke(cpu, ctx, LOCALIZED_STRING))
    });
    d.mark_context_only(LOCALIZED_STRING);
}

// dh-client-join: the status widget on the retail matchmaking progress
// screen uses these two string IDs (docs/live/live-motd.md §4).
fn matchmaking_status_text(id: u32, status: impl FnOnce() -> Option<String>) -> Option<String> {
    if matches!(id, 0x0e00_0761 | 0x1100_0762) {
        status()
    } else {
        None
    }
}

fn original(rt: &runtime::Runtime, addr: u32) -> Option<Arc<runtime::exec::OriginalHandler>> {
    let o = rt.resolve_original(addr);
    if o.is_none() {
        eprintln!("community variants: no generated entry at {addr:#x}; hook skipped");
    }
    o.map(Arc::new)
}

/// Install the three hooks. A missing original skips its hook and says so.
pub fn install(rt: &runtime::Runtime) -> anyhow::Result<()> {
    install_with_live(rt, None)
}

pub fn install_with_live(
    rt: &runtime::Runtime,
    live: Option<Arc<live_hle::Hle>>,
) -> anyhow::Result<()> {
    rt.memory()
        .map_zeroed(DATA_BASE, DATA_SIZE, Permissions::RW)?;
    rt.memory().map_zeroed(
        crate::game_variants::PAIRS_BASE,
        crate::game_variants::PAIRS_SIZE,
        Permissions::RW,
    )?;
    rt.memory().map_zeroed(
        crate::variant_options::BASE,
        crate::variant_options::SIZE,
        Permissions::RW,
    )?;
    // Resolve the originals first: `resolve_original` takes the dispatch
    // read lock, which `edit_dispatch` holds for writing.
    let (load, strings, numbers) = (
        original(rt, BUILTINS_LOAD),
        original(rt, LOCALIZED_STRING),
        original(rt, NUMBER_STRING_ID),
    );
    rt.edit_dispatch(|d| {
        if let Some(o) = load {
            d.register_override(BUILTINS_LOAD, "community:builtins", move |cpu, ctx, _| {
                // The wrapper's own call below: the original's.
                if IN_ORIGINAL.swap(false, Ordering::SeqCst) {
                    return Ok(o.invoke(cpu, ctx, BUILTINS_LOAD));
                }
                {
                    let mut mem = ctx.runtime().memory();
                    let goof = mem.u32(cpu.esp + 4)?;
                    let tags = mem.u32(TAG_INSTANCES)?;
                    // Our choices first: the community variants below are
                    // checked against the lists as the editor will show them.
                    use crate::variant_options as vo;
                    match vo::extend(&mut *mem, goof, tags, vo::BASE, vo::SIZE) {
                        Some(d) if d.lists > 0 || d.no_room > 0 => eprintln!(
                            "variant options: {} choices added to {} option lists{}",
                            d.choices,
                            d.lists,
                            if d.no_room > 0 {
                                format!("; {} lists left as retail (no room)", d.no_room)
                            } else {
                                String::new()
                            }
                        ),
                        Some(_) => {}
                        None => eprintln!("variant options: could not read the setting pages"),
                    }
                }
                {
                    let mut mem = ctx.runtime().memory();
                    let goof = mem.u32(cpu.esp + 4)?;
                    let tags = mem.u32(TAG_INSTANCES)?;
                    let offer = offered(&*mem, goof, tags);
                    let (ours, left) = match &offer {
                        Some(o) => entries(o),
                        None => (
                            Vec::new(),
                            vec!["community variants: none added: the editor's setting definitions could not be read".to_owned()],
                        ),
                    };
                    for line in left {
                        eprintln!("{line}");
                    }
                    // The game-variant store's after ours, in its own pairs
                    // page (docs/game/community-gametypes.md, "The merged block").
                    let offer = offer.unwrap_or_default();
                    remember_offered(&offer);
                    let retail = mem.u32(goof + GOOF_BUILTINS).unwrap_or(0);
                    let room = (MAX_BUILTINS.saturating_sub(retail) as usize).saturating_sub(ours.len());
                    let (stored, lines) = {
                        let store = crate::game_variants::store();
                        let store = store.lock().unwrap_or_else(|e| e.into_inner());
                        crate::game_variants::inject::entries(&store, &offer, room)
                    };
                    for line in lines {
                        eprintln!("{line}");
                    }
                    let groups = [
                        Group {
                            entries: &ours,
                            start: DATA_BASE,
                            end: ENTRIES_AT,
                        },
                        Group {
                            entries: &stored,
                            start: crate::game_variants::PAIRS_BASE,
                            end: crate::game_variants::PAIRS_BASE + crate::game_variants::PAIRS_SIZE,
                        },
                    ];
                    match extend_with(&mut *mem, goof, &groups, ENTRIES_AT) {
                        Some(d) if d.added != [0, 0] => eprintln!(
                            "community variants: {} added to the built-in list; game variants: {} added{}",
                            d.added[0],
                            d.added[1],
                            if d.over > 0 {
                                format!(", {} left out past 101 built-ins", d.over)
                            } else {
                                String::new()
                            }
                        ),
                        Some(_) => {}
                        None => eprintln!("community variants: could not extend the built-in list"),
                    }
                }
                // The Live variants for the next load, off the game thread.
                crate::game_variants::refresh_live();
                // The original, through the dispatch (the flag above sends
                // it there), then the listed records' blocks once it has
                // filed them, then back to our caller. The return address
                // is ours to restore, the stack as the original left it.
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let Ok(return_pc) = mem.u32(cpu.esp) else {
                    drop(mem);
                    return Ok(o.invoke(cpu, ctx, BUILTINS_LOAD));
                };
                cpu.esp = cpu.esp.wrapping_add(4);
                IN_ORIGINAL.store(true, Ordering::SeqCst);
                let transfer = ctx.call_guest_named(cpu, &mut mem, BUILTINS_LOAD, LOAD_RETURN, move |_, ctx| {
                    IN_ORIGINAL.store(false, Ordering::SeqCst);
                    let rt = Arc::clone(ctx.runtime());
                    let mut mem = rt.memory();
                    match write_blocks(&mut *mem) {
                        Ok(0) => {}
                        Ok(n) => eprintln!("community variants: {n} built-in records given their script's block"),
                        Err(_) => eprintln!("community variants: the built-in records could not be given their scripts' blocks"),
                    }
                    Ok(TransferAbi::ret(return_pc))
                });
                drop(mem);
                match transfer {
                    Ok(t) if t.kind != abi::FAULT => Ok(t),
                    _ => {
                        eprintln!("community variants: could not wrap the built-ins' load; their records name no script");
                        IN_ORIGINAL.store(false, Ordering::SeqCst);
                        cpu.esp = cpu.esp.wrapping_sub(4);
                        Ok(o.invoke(cpu, ctx, BUILTINS_LOAD))
                    }
                }
            });
        }
        if let Some(o) = strings {
            register_strings(d, o, live);
        }
        if let Some(o) = numbers {
            d.register_override(NUMBER_STRING_ID, DESCRIPTIONS_HOOK, move |cpu, ctx, _| {
                let mem = ctx.runtime().memory();
                let ret = mem.u32(cpu.esp)?;
                if ret == LIST_DESCRIPTION_RETURN {
                    FOCUSED_DESCRIPTION.store(cpu.ecx, std::sync::atomic::Ordering::Relaxed);
                }
                if cpu.ecx >= DESCRIPTION_BASE {
                    if let Some(id) = description_answer(ret, cpu.ecx) {
                        cpu.eax = id;
                        return mem.ret(cpu, 0);
                    }
                }
                drop(mem);
                Ok(o.invoke(cpu, ctx, NUMBER_STRING_ID))
            });
            d.mark_context_only(NUMBER_STRING_ID);
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests;
