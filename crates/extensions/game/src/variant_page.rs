//! The game variant editor's "More Options" row and page: our options the
//! record has no field for, kept in its extension block
//! ([`variants::ext`], docs/game/game-options.md section 3, "The variant
//! editor's page list").
//!
//! **Where.** The editor's category list (screen 178, list handler
//! `0x2b74c3`) is built with a fixed six rows; the shared datum-array hook
//! grows it ([`variant_count`]) by the Skulls row (word 6,
//! `crate::skulls_mp_row`) and ours (word 7, or 6 without Skulls;
//! [`more_row`]). Retail's row text leaves an unknown word blank and its
//! row chosen returns for words above 5, so our `before` hooks draw the row
//! and open the page on the editor's own screen stack. The tag has a pane
//! for each of the six categories only, so the pane hook keeps the last
//! one for our row ([`skip_pane`]).
//!
//! **What it edits.** The variant the editor edits is the 0x130-byte record
//! at [`EDITED_VARIANT`] (`0x54e4a0`: zeroed whole by `0x148a8d`, its name
//! at `+4` copied by `0x2387c6`, its engine read at `+0x44` by the row
//! chosen). The page reads the block there when it opens and at each row
//! text, and writes it back after a change ([`Rows::live_text`]), so the
//! game's editor saves it with the record.
//!
//! **Script options.** A record that names a rules script (the block's
//! mode 2 and hash prefix) gets one row per option of the script after the
//! fixed rows ([`OptionRow`]; docs/game/rules-variants.md, "Editors and the
//! IDE"). A step writes that option's field in the block's 32 option bits
//! and keeps every other byte of the block. A record with no script, or a
//! script this machine lacks, has the fixed rows only.
//!
//! **Which options.** Only those the game applies: gravity (slice 3),
//! unlimited grenades and the team count (slice 5). The others join the
//! page with their slices. The team count is a retail field with no
//! retail row (setting 93, [`variants::record::TEAM_COUNT`]), so its row
//! writes the record's `+0xb4` rather than the block.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use guest_layout::entry::{EntryFuture, GuestCall, Ret};
use guest_layout::memory::{GuestMemory, MemFault};
use hooks::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;
use variants::ext::{self, Ext};
use variants::ext_weapons;

use crate::layout_glue::chosen_row;
use crate::list_class::{self, ListClass, ListLayout, LiveText, Rows, TextBlock};
use crate::skulls_mp::{
    SCREEN_SLOT, SCREEN_WINDOW, VARIANT_LIST_NAME, VARIANT_ROWS, VARIANT_SCREEN_ID,
};
use crate::ui::{Chosen, LABEL_SLOT};
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::picker::{self, Pick};
use crate::ui_kit::spec::{Describe, Press, RowKind, RowSpec, ScreenSpec};

/// The record the game's variant editor edits.
pub const EDITED_VARIANT: u32 = 0x0054_e4a0;
/// Its extension block.
pub const EDITED_BLOCK: u32 = EDITED_VARIANT + ext::AT as u32;

/// The page's id for `Open` rows and the loader registry.
pub const SCREEN: &str = "variant-more-options";
/// The page's list class (`features.variant-more-options` switches it).
pub const SCREEN_FEATURE: &str = "variant-more-options";
/// The row (`features.variant-more-options-row` switches it).
pub const ROW_FEATURE: &str = "variant-more-options-row";

/// Our strings: the title, the footer, the row on the category list, then
/// per row its description and a label per step.
pub const IDS: Family = ids::family("variant page");
pub const TITLE_ID: u32 = IDS.id(0);
pub const FOOTER_ID: u32 = IDS.id(1);
pub const ROW_ID: u32 = IDS.id(2);
pub const GRAVITY_DESCRIPTION_ID: u32 = IDS.id(3);
const GRAVITY_LABELS: u32 = 4;
pub const TITLE: &str = "More Options";
pub const PROMPT: &str = "\u{e10a}   \u{e10b} CHANGE  \u{e100} SELECT  \u{e101} BACK";
pub const GRAVITY_DESCRIPTION: &str = "Gravity for everyone, as a share of the game's. Low Gravity halves it again. \
     Not on System Link.";

/// Gravity's steps on the page, percent. Studio sets any value in
/// [`ext::GRAVITY`]; the page shows the nearest step.
pub const GRAVITY_STEPS: [u16; 10] = [10, 25, 50, 75, 100, 125, 150, 200, 300, 400];

pub const GRENADES_DESCRIPTION_ID: u32 = IDS.id(GRAVITY_LABELS + GRAVITY_STEPS.len() as u32);
const GRENADES_LABELS: u32 = GRAVITY_LABELS + GRAVITY_STEPS.len() as u32 + 1;
pub const GRENADES_DESCRIPTION: &str = "Grenades never run out: everyone keeps at least four of each. \
     Not on System Link.";

pub const TEAMS_DESCRIPTION_ID: u32 = IDS.id(GRENADES_LABELS + 2);
const TEAMS_LABELS: u32 = GRENADES_LABELS + 3;
pub const TEAMS_DESCRIPTION: &str = "How many teams a team game makes, at most the map's. \
     Every build plays it.";

/// The damage, shield, health and speed scales: their rows, after the team count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    Damage,
    Shield,
    Health,
    Speed,
}

impl Scale {
    pub const ALL: [Scale; 4] = [Scale::Damage, Scale::Shield, Scale::Health, Scale::Speed];

    pub fn row(self) -> i16 {
        TEAMS_ROW + 1 + self as i16
    }

    pub fn of_row(row: i16) -> Option<Scale> {
        Scale::ALL.into_iter().find(|s| s.row() == row)
    }

    pub fn name(self) -> &'static str {
        match self {
            Scale::Damage => "Damage",
            Scale::Shield => "Shields",
            Scale::Health => "Health",
            Scale::Speed => "Speed",
        }
    }

    pub fn get(self, e: &Ext) -> u16 {
        match self {
            Scale::Damage => e.damage,
            Scale::Shield => e.shield,
            Scale::Health => e.health,
            Scale::Speed => e.speed,
        }
    }

    pub fn set(self, e: &mut Ext, v: u16) {
        match self {
            Scale::Damage => e.damage = v,
            Scale::Shield => e.shield = v,
            Scale::Health => e.health = v,
            Scale::Speed => e.speed = v,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Scale::Damage => {
                "Damage every player deals, as a share of the game's. Not on System Link."
            }
            Scale::Shield => {
                "How much every player's shield holds, as a share of the game's. Not on System Link."
            }
            Scale::Health => {
                "How much every player's body holds, as a share of the game's. Not on System Link."
            }
            Scale::Speed => {
                "How fast every player moves, as a share of the game's. Not on System Link."
            }
        }
    }

    fn ids_at(self) -> u32 {
        SCALE_IDS + self as u32 * (1 + GRAVITY_STEPS.len() as u32)
    }

    pub fn description_id(self) -> u32 {
        IDS.id(self.ids_at())
    }

    /// The label id of step `i`.
    pub fn label_id(self, i: usize) -> u32 {
        IDS.id(self.ids_at() + 1 + i as u32)
    }

    /// Step `i`'s label, such as "Damage: 150%".
    pub fn label(self, i: usize) -> String {
        format!("{}: {}", self.name(), percent_choice(GRAVITY_STEPS[i]))
    }
}

/// The page's rows.
pub const GRAVITY_ROW: i16 = 0;
pub const GRENADES_ROW: i16 = 1;
pub const TEAMS_ROW: i16 = 2;
pub const ROWS: usize = 9;

/// The other-weapon rows, after the scales: the weapon the block names
/// for the starting and the secondary weapon ([`variants::ext_weapons`],
/// the weapons the record's byte cannot name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Primary,
    Secondary,
}

impl Slot {
    pub const ALL: [Slot; 2] = [Slot::Primary, Slot::Secondary];

    pub fn row(self) -> i16 {
        TEAMS_ROW + 1 + Scale::ALL.len() as i16 + self as i16
    }

    pub fn of_row(row: i16) -> Option<Slot> {
        Slot::ALL.into_iter().find(|s| s.row() == row)
    }

    pub fn name(self) -> &'static str {
        match self {
            Slot::Primary => "Primary",
            Slot::Secondary => "Secondary",
        }
    }

    pub fn get(self, e: &Ext) -> u16 {
        match self {
            Slot::Primary => e.primary,
            Slot::Secondary => e.secondary,
        }
    }

    pub fn set(self, e: &mut Ext, id: u16) {
        match self {
            Slot::Primary => e.primary = id,
            Slot::Secondary => e.secondary = id,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Slot::Primary => {
                "A weapon the map's list lacks, given as the starting weapon in place of the one above. \
                 The map must carry it. Not on System Link."
            }
            Slot::Secondary => {
                "A weapon the map's list lacks, given as the secondary weapon in place of the one above. \
                 The map must carry it. Not on System Link."
            }
        }
    }

    fn ids_at(self) -> u32 {
        WEAPON_IDS + self as u32 * (1 + MAX_WEAPON_CHOICES as u32)
    }

    pub fn description_id(self) -> u32 {
        IDS.id(self.ids_at())
    }

    /// The label id of picker step `i` (0 is "not used", then the table).
    pub fn label_id(self, i: usize) -> u32 {
        IDS.id(self.ids_at() + 1 + i as u32)
    }

    /// Step `i`'s label, such as "Primary: Spiker".
    pub fn label(self, i: usize) -> String {
        let id = ext_weapons::id_at(i);
        format!("{}: {}", self.name(), ext_weapons::name(id))
    }
}

/// The most picker steps a weapon row has ("not used" and the table).
pub const MAX_WEAPON_CHOICES: usize = 16;
const _: () = assert!(ext_weapons::TABLE.len() < MAX_WEAPON_CHOICES);

/// The team count in the edited record (`+0xb4`, an i32).
pub const EDITED_TEAMS: u32 = EDITED_VARIANT + 0xb4;
const TEAM_COUNTS: [i32; 8] = variants::record::TEAM_COUNTS;

/// The scales' ids: per scale a description, then a label per step (the
/// gravity steps, which are [`ext::PERCENT`]'s too).
const SCALE_IDS: u32 = TEAMS_LABELS + TEAM_COUNTS.len() as u32;

/// The script option rows' ids: a label per row, then a description per
/// row. Their text is the script's and changes with the record, so each id
/// holds a placeholder of [`OPTION_ROOM`] bytes that `live_text` rewrites.
const OPTION_IDS: u32 = SCALE_IDS + Scale::ALL.len() as u32 * (1 + GRAVITY_STEPS.len() as u32);

/// The most option rows (an option takes at least one of the record's 32
/// option bits to be worth a row; the rest are not shown).
pub const MAX_OPTION_ROWS: usize = 32;
/// The room each option string has, NUL included.
pub const OPTION_ROOM: usize = 128;

/// The weapon rows' ids: per slot a description, then a label per step.
const WEAPON_IDS: u32 = OPTION_IDS + 2 * MAX_OPTION_ROWS as u32;

const _: () = assert!(
    WEAPON_IDS + Slot::ALL.len() as u32 * (1 + MAX_WEAPON_CHOICES as u32) <= IDS.len
);

/// The label id of script option row `i`.
pub fn option_label_id(i: usize) -> u32 {
    IDS.id(OPTION_IDS + i as u32)
}

/// The description id of script option row `i`.
pub fn option_description_id(i: usize) -> u32 {
    IDS.id(OPTION_IDS + MAX_OPTION_ROWS as u32 + i as u32)
}

/// What an option's value is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptionKind {
    Bool,
    /// The names of the values, from the option's `lo`.
    Enum(Vec<String>),
    Int,
}

/// One script option as the page shows it: a row of the variant editor,
/// a field of the record's option bits (`rules::Ir::option_offsets`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionRow {
    pub label: String,
    pub description: String,
    pub kind: OptionKind,
    pub lo: i32,
    pub hi: i32,
    /// The field's first bit in the option bits, and its width.
    pub offset: u32,
    pub bits: u32,
}

/// An enum value's name as the row shows it: `first_to` is "First to".
fn titled(name: &str) -> String {
    let spaced = name.replace('_', " ");
    let mut c = spaced.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Cuts `text` at a character boundary and ends it with NUL, to `room`.
fn served(text: &str, room: usize) -> Vec<u8> {
    let mut cut = text.len().min(room - 1);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut b = text.as_bytes()[..cut].to_vec();
    b.push(0);
    b
}

impl OptionRow {
    fn count(&self) -> i64 {
        (i64::from(self.hi) - i64::from(self.lo)).max(0) + 1
    }

    /// The value in option bits `bits`, as the VM reads it: `lo` plus the
    /// field, at most `hi`; `lo` for a field that has no bits.
    pub fn get(&self, bits: u32) -> i32 {
        if self.bits == 0 || self.offset >= 32 {
            return self.lo;
        }
        let raw = (u64::from(bits) >> self.offset) & ((1u64 << self.bits) - 1);
        (i64::from(self.lo) + raw as i64).min(i64::from(self.hi)) as i32
    }

    /// `bits` with this field set to `value` (kept within `lo..=hi`) and
    /// every other bit as it was.
    pub fn set(&self, bits: u32, value: i32) -> u32 {
        if self.bits == 0 || self.offset >= 32 {
            return bits;
        }
        let v = i64::from(value).clamp(i64::from(self.lo), i64::from(self.hi));
        let mask = ((1u64 << self.bits) - 1) << self.offset;
        let field = (((v - i64::from(self.lo)) as u64) << self.offset) & mask;
        ((u64::from(bits) & !mask) | field) as u32
    }

    /// The value as shown.
    pub fn text(&self, value: i32) -> String {
        match &self.kind {
            OptionKind::Bool => if value != 0 { "On" } else { "Off" }.into(),
            OptionKind::Enum(names) => usize::try_from(i64::from(value) - i64::from(self.lo))
                .ok()
                .and_then(|i| names.get(i))
                .cloned()
                .unwrap_or_else(|| value.to_string()),
            OptionKind::Int => value.to_string(),
        }
    }

    /// The row's label for `bits`, such as "First find: 3".
    pub fn label_for(&self, bits: u32) -> String {
        format!("{}: {}", self.label, self.text(self.get(bits)))
    }

    /// Every value, in order, when the row offers a picker: a bool, an
    /// enum, or an int of at most 16 values. A wider int only steps.
    pub fn choices(&self) -> Option<Vec<String>> {
        match &self.kind {
            OptionKind::Int if self.count() > 16 => None,
            _ => Some(
                (0..self.count())
                    .map(|i| self.text((i64::from(self.lo) + i) as i32))
                    .collect(),
            ),
        }
    }

    /// The value one step up or down from `value`: by one, or for a wide
    /// int by a sixteenth of its range.
    pub fn step(&self, value: i32, up: bool) -> i32 {
        let by = if self.choices().is_some() {
            1
        } else {
            (self.count() / 16).max(1)
        };
        let v = i64::from(value) + if up { by } else { -by };
        v.clamp(i64::from(self.lo), i64::from(self.hi)) as i32
    }
}

/// The option rows of a script, in declaration order (at most
/// [`MAX_OPTION_ROWS`]).
#[cfg(feature = "rules")]
pub fn option_rows(ir: &::rules::Ir) -> Vec<OptionRow> {
    let text = |i: u16| ir.strings.get(i as usize).cloned().unwrap_or_default();
    let offsets = ir.option_offsets();
    ir.options
        .iter()
        .enumerate()
        .take(MAX_OPTION_ROWS)
        .map(|(i, o)| {
            let name = text(o.name);
            let label = text(o.label);
            OptionRow {
                label: if label.is_empty() {
                    titled(&name)
                } else {
                    label
                },
                description: o.description.map(text).unwrap_or_default(),
                kind: match o.ty {
                    ::rules::Ty::Bool => OptionKind::Bool,
                    ::rules::Ty::Enum(_) => {
                        OptionKind::Enum(ir.option_values(i).into_iter().map(titled).collect())
                    }
                    _ => OptionKind::Int,
                },
                lo: o.lo,
                hi: o.hi,
                offset: offsets[i],
                bits: o.bits(),
            }
        })
        .collect()
}

/// The options of the script whose hash starts with `prefix`, when this
/// machine has it: a compiled-in one or the game-variant store's (the
/// lookup `rules::header` makes). `None` for a script it lacks.
#[cfg(feature = "rules")]
pub fn script_options(prefix: [u8; 8]) -> Option<Vec<OptionRow>> {
    let blob = match crate::rules::runtime::compiled_in(prefix) {
        Some(b) => b.blob.clone(),
        None => crate::game_variants::store()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .script(prefix)?
            .to_vec(),
    };
    let (ir, _) = ::rules::verify::load(&blob).ok()?;
    Some(option_rows(&ir))
}

/// Without the rules runtime no script is known.
#[cfg(not(feature = "rules"))]
pub fn script_options(_prefix: [u8; 8]) -> Option<Vec<OptionRow>> {
    None
}

/// The step of team count `t`: its place in [`TEAM_COUNTS`], or 2 teams
/// for a value we don't offer (1).
pub fn teams_step(t: i32) -> usize {
    TEAM_COUNTS.iter().position(|&c| c == t).unwrap_or(1)
}

/// The label id of team count step `i`.
pub fn teams_label_id(i: usize) -> u32 {
    IDS.id(TEAMS_LABELS + i as u32)
}

fn team_choice(teams: i32) -> String {
    match teams {
        0 => "Map Default".into(),
        t => t.to_string(),
    }
}

/// Team count step `i`'s label, such as "Team Count: 4".
pub fn teams_label(i: usize) -> String {
    format!("Team Count: {}", team_choice(TEAM_COUNTS[i]))
}

/// The label id of the grenades row, off or on.
pub fn grenades_label_id(on: bool) -> u32 {
    IDS.id(GRENADES_LABELS + u32::from(on))
}

/// The grenades row's label.
pub fn grenades_label(on: bool) -> String {
    format!("Unlimited Grenades: {}", if on { "On" } else { "Off" })
}

/// The label id of gravity step `i`.
pub fn gravity_label_id(i: usize) -> u32 {
    IDS.id(GRAVITY_LABELS + i as u32)
}

fn percent_choice(percent: u16) -> String {
    match percent {
        100 => "Normal".into(),
        p => format!("{p}%"),
    }
}

/// Gravity step `i`'s label, such as "Gravity: 50%".
pub fn gravity_label(i: usize) -> String {
    format!("Gravity: {}", percent_choice(GRAVITY_STEPS[i]))
}

/// The step nearest `g` (the lower on a tie).
pub fn nearest_step(g: u16) -> usize {
    (0..GRAVITY_STEPS.len())
        .min_by_key(|&i| GRAVITY_STEPS[i].abs_diff(g))
        .unwrap_or(4)
}

/// Every string we serve.
pub fn strings() -> Vec<(u32, String)> {
    let mut v = vec![
        (TITLE_ID, TITLE.to_string()),
        (FOOTER_ID, PROMPT.to_string()),
        (ROW_ID, TITLE.to_string()),
        (GRAVITY_DESCRIPTION_ID, GRAVITY_DESCRIPTION.to_string()),
    ];
    for i in 0..GRAVITY_STEPS.len() {
        v.push((gravity_label_id(i), gravity_label(i)));
    }
    v.push((GRENADES_DESCRIPTION_ID, GRENADES_DESCRIPTION.to_string()));
    for on in [false, true] {
        v.push((grenades_label_id(on), grenades_label(on)));
    }
    v.push((TEAMS_DESCRIPTION_ID, TEAMS_DESCRIPTION.to_string()));
    for i in 0..TEAM_COUNTS.len() {
        v.push((teams_label_id(i), teams_label(i)));
    }
    for sc in Scale::ALL {
        v.push((sc.description_id(), sc.description().to_string()));
        for i in 0..GRAVITY_STEPS.len() {
            v.push((sc.label_id(i), sc.label(i)));
        }
    }
    for sl in Slot::ALL {
        v.push((sl.description_id(), sl.description().to_string()));
        for i in 0..=ext_weapons::TABLE.len() {
            v.push((sl.label_id(i), sl.label(i)));
        }
    }
    // The script options' strings are rewritten at run time
    // (`Rows::live_text`) within their room.
    let room = " ".repeat(OPTION_ROOM - 1);
    for i in 0..MAX_OPTION_ROWS {
        v.push((option_label_id(i), room.clone()));
        v.push((option_description_id(i), room.clone()));
    }
    v
}

// ---- the row on the category list ----

static ROW_ON: AtomicBool = AtomicBool::new(false);

/// Whether the category list gets our row.
pub fn row_on() -> bool {
    ROW_ON.load(Ordering::SeqCst)
}

/// Our row's word: after the Skulls row when that is on.
pub fn more_row_with(skulls: bool) -> i16 {
    VARIANT_ROWS as i16 + i16::from(skulls)
}

/// Our row's word in this run.
pub fn more_row() -> i16 {
    more_row_with(crate::skulls_mp_row::row_on())
}

/// The category list's count when the game builds the array `name` with
/// `retail` rows: plus the Skulls row and ours, each when on.
pub fn variant_count(name: u32, retail: u32, skulls: bool, more: bool) -> u32 {
    if name == VARIANT_LIST_NAME && retail == VARIANT_ROWS {
        retail + u32::from(skulls) + u32::from(more)
    } else {
        retail
    }
}

/// A pane change to skip: our row on the category list has no pane.
pub fn skip_pane_with(screen_id: i16, row: i16, more: bool, skulls: bool) -> bool {
    more && screen_id == VARIANT_SCREEN_ID && row == more_row_with(skulls)
}

/// [`skip_pane_with`] for this run.
pub fn skip_pane(screen_id: i16, row: i16) -> bool {
    skip_pane_with(screen_id, row, row_on(), crate::skulls_mp_row::row_on())
}

/// What a press on our row by `player` on a screen whose window and slot
/// are `stack` opens: the request for our page's loader, or why not.
pub fn opening(
    player: Option<u32>,
    stack: Option<(u32, u32)>,
    loader: Option<u32>,
) -> Result<[u8; 0x20], String> {
    let player = player.ok_or("variant options: the pressing player is unreadable; not opened")?;
    let loader = loader.ok_or("variant options: the page's class is off; not opened")?;
    let (window, slot) =
        stack.ok_or("variant options: the editor's screen stack is unreadable; not opened")?;
    Ok(crate::skulls_mp::open_request(
        1 << player,
        window,
        slot,
        loader,
    ))
}

/// The row's hooks.
pub struct MoreRow;

fn row_text(c: &mut Call) -> Result<Flow, GuestFault> {
    let (item, row) = {
        let mem = c.ctx.runtime().memory();
        let item = mem.u32(c.cpu.esp + 4).ok();
        (item, crate::video_screen::text_row(&*mem, c.cpu.esp))
    };
    let (Some(item), Some(row)) = (item, row) else {
        return Ok(Flow::Continue);
    };
    if row != more_row() {
        return Ok(Flow::Continue);
    }
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            list_class::draw_texts(&cx, item, vec![(0, ROW_ID)]).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

fn row_chosen(c: &mut Call, request_at: u32) -> Result<Flow, GuestFault> {
    let (row, player, stack) = {
        let mem = c.ctx.runtime().memory();
        let at = c.cpu.ecx.wrapping_sub(guest_layout::view::ui::LIST);
        let stack = mem
            .u32(at + SCREEN_WINDOW)
            .ok()
            .zip(mem.u32(at + SCREEN_SLOT).ok());
        (
            chosen_row(&*mem, c.cpu.esp),
            crate::skulls_menu::player_index(&*mem, c.cpu.esp),
            stack,
        )
    };
    if row != Some(more_row()) {
        return Ok(Flow::Continue);
    }
    let request = match opening(player, stack, list_class::loader_of(SCREEN)) {
        Ok(r) => r,
        Err(why) => {
            eprintln!("{why}");
            return c.ret(8).map(Flow::Return);
        }
    };
    let loader = u32::from_le_bytes(request[0x1c..0x20].try_into().unwrap_or_default());
    eprintln!("variant options: opening More Options");
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            cx.write(|m| m.write(request_at, &request))?;
            cx.call(GuestCall::new(loader).args([request_at])).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

impl hooks::Feature for MoreRow {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ROW_FEATURE,
            "The More Options row in the game variant editor",
        )
        .doc("docs/game/game-options.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use guest_layout::points::{VARIANT_OPTIONS_ROW_CHOSEN, VARIANT_OPTIONS_ROW_TEXT};
        let request_at = b.guest_alloc(0x1000, Permissions::RW)?;
        b.raw()
            .before(
                &VARIANT_OPTIONS_ROW_TEXT,
                "the More Options row's text",
                row_text,
            )
            // The Skulls row hooks both points too. The two act on different
            // rows (more_row() sits after the Skulls row), so the order is
            // only for the plan: ours runs inside the Skulls row's hooks.
            .after_feature(crate::skulls_mp_row::ROW_FEATURE);
        b.raw()
            .before(
                &VARIANT_OPTIONS_ROW_CHOSEN,
                "the More Options row's press",
                move |c| row_chosen(c, request_at),
            )
            .after_feature(crate::skulls_mp_row::ROW_FEATURE);
        ROW_ON.store(true, Ordering::SeqCst);
        // `ROW_ON` is fixed at build, the row's text and press are read
        // from guest memory and answered by continuations the machine
        // check sees. The page's edits are its class's, not this row's.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

// ---- the page ----

/// The options of the script a hash prefix names ([`script_options`]).
pub type Resolver = Arc<dyn Fn([u8; 8]) -> Option<Vec<OptionRow>> + Send + Sync>;

/// The More Options page shares its edit buffer with the open picker.
#[derive(Clone)]
pub struct Page {
    edit: Arc<Edit>,
    options: Arc<Options>,
}

impl Default for Page {
    fn default() -> Self {
        Self::with_scripts(Arc::new(script_options))
    }
}

/// The script option rows of the record being edited.
struct Options {
    resolve: Resolver,
    shown: Mutex<Shown>,
    /// The row count the list was last built or grown to.
    built: AtomicUsize,
    /// The strings last written, so only a change is written again.
    written: Mutex<Vec<(u32, String)>>,
}

#[derive(Default)]
struct Shown {
    /// The prefix the rows are for.
    prefix: Option<[u8; 8]>,
    /// Whether the resolver found that script.
    resolved: bool,
    rows: Vec<OptionRow>,
}

struct Edit {
    /// The block as last read from the record, with our edits.
    block: Mutex<[u8; ext::LEN]>,
    /// An edit not yet written to the record.
    dirty: AtomicBool,
    /// The record's team count (`+0xb4`), with our edit.
    teams: Mutex<i32>,
    teams_dirty: AtomicBool,
}

impl Default for Edit {
    fn default() -> Self {
        Self {
            block: Mutex::new([0; ext::LEN]),
            dirty: AtomicBool::new(false),
            teams: Mutex::new(0),
            teams_dirty: AtomicBool::new(false),
        }
    }
}

impl Page {
    pub fn new() -> Self {
        Self::default()
    }

    /// A page that finds scripts through `resolve` (tests give it their
    /// own).
    pub fn with_scripts(resolve: Resolver) -> Self {
        Self {
            edit: Arc::default(),
            options: Arc::new(Options {
                resolve,
                shown: Mutex::default(),
                built: AtomicUsize::new(ROWS),
                written: Mutex::default(),
            }),
        }
    }

    /// The script option rows shown: those of the script the block names.
    pub fn option_rows(&self) -> Vec<OptionRow> {
        self.options
            .shown
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .rows
            .clone()
    }

    /// The option of page row `row`, past the fixed rows.
    fn option_of(&self, row: i16) -> Option<(usize, OptionRow)> {
        let i = usize::try_from(row.checked_sub(ROWS as i16)?).ok()?;
        let r = self.option_rows().get(i)?.clone();
        Some((i, r))
    }

    /// Finds the rows of the script the block names, again only when its
    /// prefix changed or the script was not found.
    fn refresh_options(&self) {
        let script = self.ext().script;
        let want = (script.mode == ext::Mode::Rules).then_some(script.prefix);
        let mut shown = self.options.shown.lock().unwrap_or_else(|e| e.into_inner());
        if want == shown.prefix && (want.is_none() || shown.resolved) {
            return;
        }
        shown.prefix = want;
        shown.rows.clear();
        shown.resolved = false;
        if let Some(p) = want
            && let Some(rows) = (self.options.resolve)(p)
        {
            shown.rows = rows;
            shown.resolved = true;
        }
    }

    /// The text of each script option row's label and description.
    fn option_texts(&self) -> Vec<(u32, String)> {
        let bits = self.ext().script.options;
        let mut v = Vec::new();
        for (i, r) in self.option_rows().iter().enumerate() {
            v.push((option_label_id(i), r.label_for(bits)));
            v.push((option_description_id(i), r.description.clone()));
        }
        v
    }

    /// Sets option row `i` to the value `pick` makes of its current one.
    /// What to log.
    fn choose_option(&self, i: usize, pick: impl FnOnce(&OptionRow, i32) -> i32) -> String {
        let Some(row) = self.option_rows().get(i).cloned() else {
            return format!("variant options: no script option row {i}");
        };
        let mut b = self.edit.block.lock().unwrap_or_else(|e| e.into_inner());
        let mut e = Ext::in_effect(&b);
        let was = row.get(e.script.options);
        let value = pick(&row, was).clamp(row.lo, row.hi);
        let name = row.label.to_ascii_lowercase();
        if value == was {
            return format!("variant options: {name} stays {}", row.text(was));
        }
        e.script.options = row.set(e.script.options, value);
        *b = e.encode();
        self.edit.dirty.store(true, Ordering::SeqCst);
        format!(
            "variant options: {name} {} -> {}",
            row.text(was),
            row.text(value)
        )
    }

    /// The options shown (the game's own for a block the game ignores).
    pub fn ext(&self) -> Ext {
        Ext::in_effect(&self.edit.block.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// The record's block, read into the page (unless an edit is pending).
    pub fn read(&self, block: [u8; ext::LEN]) {
        if !self.edit.dirty.load(Ordering::SeqCst) {
            *self.edit.block.lock().unwrap_or_else(|e| e.into_inner()) = block;
        }
        self.refresh_options();
    }

    /// The write the record needs, once per edit.
    pub fn pending(&self) -> Option<[u8; ext::LEN]> {
        self.edit
            .dirty
            .swap(false, Ordering::SeqCst)
            .then(|| *self.edit.block.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn choose_gravity(&self, pick: impl FnOnce(usize) -> usize) -> String {
        self.choose_percent("gravity", |e| e.gravity, |e, v| e.gravity = v, pick)
    }

    fn choose_scale(&self, scale: Scale, pick: impl FnOnce(usize) -> usize) -> String {
        let what = scale.name().to_ascii_lowercase();
        self.choose_percent(&what, |e| scale.get(e), |e, v| scale.set(e, v), pick)
    }

    /// Sets the weapon of slot `slot` to the table's step `pick` makes of
    /// the current one. What to log.
    fn choose_weapon(&self, slot: Slot, pick: impl FnOnce(usize) -> usize) -> String {
        let what = slot.name().to_ascii_lowercase();
        let mut b = self.edit.block.lock().unwrap_or_else(|e| e.into_inner());
        let mut e = Ext::in_effect(&b);
        let was = slot.get(&e);
        let i = pick(ext_weapons::step(was));
        if i > ext_weapons::TABLE.len() {
            return format!("variant options: no {what} choice at {i}");
        }
        let id = ext_weapons::id_at(i);
        if id == was {
            return format!("variant options: {what} stays {}", ext_weapons::name(was));
        }
        slot.set(&mut e, id);
        *b = e.encode();
        self.edit.dirty.store(true, Ordering::SeqCst);
        format!(
            "variant options: {what} {} -> {}",
            ext_weapons::name(was),
            ext_weapons::name(id)
        )
    }

    fn choose_percent(
        &self,
        what: &str,
        get: impl FnOnce(&Ext) -> u16,
        set: impl FnOnce(&mut Ext, u16),
        pick: impl FnOnce(usize) -> usize,
    ) -> String {
        let mut b = self.edit.block.lock().unwrap_or_else(|e| e.into_inner());
        let mut e = Ext::in_effect(&b);
        let was = get(&e);
        let i = pick(nearest_step(was));
        let Some(&value) = GRAVITY_STEPS.get(i) else {
            return format!("variant options: no {what} choice at {i}");
        };
        if value == was {
            return format!("variant options: {what} stays {was}%");
        }
        set(&mut e, value);
        *b = e.encode();
        self.edit.dirty.store(true, Ordering::SeqCst);
        format!("variant options: {what} {was}% -> {value}%")
    }

    /// The record's team count, as shown.
    pub fn teams(&self) -> i32 {
        *self.edit.teams.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The record's team count, read into the page (unless an edit is
    /// pending).
    pub fn read_teams(&self, t: i32) {
        if !self.edit.teams_dirty.load(Ordering::SeqCst) {
            *self.edit.teams.lock().unwrap_or_else(|e| e.into_inner()) = t;
        }
    }

    /// The team count write the record needs, once per edit.
    pub fn pending_teams(&self) -> Option<i32> {
        self.edit
            .teams_dirty
            .swap(false, Ordering::SeqCst)
            .then(|| self.teams())
    }

    fn choose_teams(&self, pick: impl FnOnce(usize) -> usize) -> String {
        let mut teams = self.edit.teams.lock().unwrap_or_else(|e| e.into_inner());
        let was = *teams;
        let i = pick(teams_step(was));
        let Some(&value) = TEAM_COUNTS.get(i) else {
            return format!("variant options: no team count choice at {i}");
        };
        if value == was {
            return format!("variant options: team count stays {was}");
        }
        *teams = value;
        self.edit.teams_dirty.store(true, Ordering::SeqCst);
        format!("variant options: team count {was} -> {value}")
    }

    /// Turns unlimited grenades on or off. What to log.
    pub fn toggle_grenades(&self) -> String {
        let mut b = self.edit.block.lock().unwrap_or_else(|e| e.into_inner());
        let mut e = Ext::in_effect(&b);
        e.unlimited_grenades = !e.unlimited_grenades;
        *b = e.encode();
        self.edit.dirty.store(true, Ordering::SeqCst);
        format!(
            "variant options: unlimited grenades {}",
            if e.unlimited_grenades { "on" } else { "off" }
        )
    }

    fn row_spec(&self, row: i16) -> Option<RowSpec> {
        let picker = || Press::Open(picker::NAME.into());
        match row {
            GRAVITY_ROW => {
                let i = nearest_step(self.ext().gravity);
                Some(
                    percent_row("Gravity", i)
                        .press(picker())
                        .describe(GRAVITY_DESCRIPTION)
                        .ids(&[gravity_label_id(i), GRAVITY_DESCRIPTION_ID]),
                )
            }
            GRENADES_ROW => {
                let on = self.ext().unlimited_grenades;
                Some(
                    RowSpec::value("Unlimited Grenades", &["Off", "On"], usize::from(on))
                        .describe(GRENADES_DESCRIPTION)
                        .ids(&[grenades_label_id(on), GRENADES_DESCRIPTION_ID]),
                )
            }
            TEAMS_ROW => {
                let choices = TEAM_COUNTS.map(team_choice);
                let i = teams_step(self.teams());
                Some(
                    RowSpec::value("Team Count", &choices.each_ref().map(String::as_str), i)
                        .press(picker())
                        .describe(TEAMS_DESCRIPTION)
                        .ids(&[teams_label_id(i), TEAMS_DESCRIPTION_ID]),
                )
            }
            r if r >= ROWS as i16 => {
                let (i, o) = self.option_of(r)?;
                let bits = self.ext().script.options;
                let shown = o.text(o.get(bits));
                let choices = o.choices();
                let values: Vec<&str> = match &choices {
                    Some(c) => c.iter().map(String::as_str).collect(),
                    None => vec![shown.as_str()],
                };
                let current = choices
                    .as_ref()
                    .map_or(0, |_| (i64::from(o.get(bits)) - i64::from(o.lo)) as usize);
                let spec = RowSpec::value(&o.label, &values, current)
                    .describe(&o.description)
                    .ids(&[option_label_id(i), option_description_id(i)]);
                Some(if choices.is_some() {
                    spec.press(picker())
                } else {
                    spec
                })
            }
            r if Slot::of_row(r).is_some() => {
                let slot = Slot::of_row(r)?;
                let i = ext_weapons::step(slot.get(&self.ext()));
                Some(
                    RowSpec::value(slot.name(), &ext_weapons::choices(), i)
                        .press(picker())
                        .describe(slot.description())
                        .ids(&[slot.label_id(i), slot.description_id()]),
                )
            }
            r => {
                let scale = Scale::of_row(r)?;
                let i = nearest_step(scale.get(&self.ext()));
                Some(
                    percent_row(scale.name(), i)
                        .press(picker())
                        .describe(scale.description())
                        .ids(&[scale.label_id(i), scale.description_id()]),
                )
            }
        }
    }

    fn open_picker(&self, row: i16) -> Chosen {
        let Some(spec) = self.row_spec(row) else {
            return Chosen::Nothing;
        };
        let RowKind::Value { values, current } = spec.kind else {
            return Chosen::Nothing;
        };
        let page = self.clone();
        let set = Arc::new(move |index| match row {
            r if r >= ROWS as i16 => page.choose_option(r as usize - ROWS, |o, _| {
                (i64::from(o.lo) + index as i64).clamp(i64::from(o.lo), i64::from(o.hi)) as i32
            }),
            GRAVITY_ROW => page.choose_gravity(|_| index),
            TEAMS_ROW => page.choose_teams(|_| index),
            r if Slot::of_row(r).is_some() => {
                page.choose_weapon(Slot::of_row(r).expect("weapon picker row"), |_| index)
            }
            r => page.choose_scale(Scale::of_row(r).expect("scale picker row"), |_| index),
        });
        let pick = Pick::new(
            &spec.label,
            values,
            current,
            spec.description.as_deref().unwrap_or_default(),
            set,
        );
        picker::offer(pick).map_or(Chosen::Nothing, Chosen::Open)
    }
}

fn percent_row(label: &str, current: usize) -> RowSpec {
    let choices = GRAVITY_STEPS.map(percent_choice);
    RowSpec::value(label, &choices.each_ref().map(String::as_str), current)
}

fn next_step(i: usize, len: usize, up: bool) -> usize {
    if up {
        (i + 1).min(len - 1)
    } else {
        i.saturating_sub(1)
    }
}

impl Rows for Page {
    fn title(&self) -> u32 {
        TITLE_ID
    }
    fn count(&self) -> usize {
        let n = ROWS + self.option_rows().len();
        self.options.built.store(n, Ordering::SeqCst);
        n
    }
    /// The list holds room for every script option row and takes the rows
    /// of the script the record names once the page reads it.
    fn growing_capacity(&self) -> Option<usize> {
        Some(ROWS + MAX_OPTION_ROWS)
    }
    fn shrink_growing_rows(&self) -> bool {
        true
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        if let Some((i, _)) = self.option_of(row) {
            return vec![(LABEL_SLOT, option_label_id(i))];
        }
        match row {
            GRAVITY_ROW => vec![(
                LABEL_SLOT,
                gravity_label_id(nearest_step(self.ext().gravity)),
            )],
            GRENADES_ROW => vec![(LABEL_SLOT, grenades_label_id(self.ext().unlimited_grenades))],
            TEAMS_ROW => vec![(LABEL_SLOT, teams_label_id(teams_step(self.teams())))],
            r => match (Scale::of_row(r), Slot::of_row(r)) {
                (Some(sc), _) => {
                    vec![(LABEL_SLOT, sc.label_id(nearest_step(sc.get(&self.ext()))))]
                }
                (_, Some(sl)) => {
                    vec![(LABEL_SLOT, sl.label_id(ext_weapons::step(sl.get(&self.ext()))))]
                }
                _ => Vec::new(),
            },
        }
    }
    fn content_spec(&self) -> Option<ScreenSpec> {
        Some(self.spec())
    }
    fn press(&self, row: i16) -> Chosen {
        if let Some((_, o)) = self.option_of(row) {
            return if o.choices().is_some() {
                self.open_picker(row)
            } else {
                Chosen::Nothing
            };
        }
        match row {
            GRENADES_ROW => Chosen::Stepped(self.toggle_grenades()),
            _ => self.open_picker(row),
        }
    }
    fn sideways(&self, row: i16, right: bool) -> Option<String> {
        let percent = |i| next_step(i, GRAVITY_STEPS.len(), right);
        if let Some((i, _)) = self.option_of(row) {
            return Some(self.choose_option(i, |o, v| o.step(v, right)));
        }
        match row {
            GRAVITY_ROW => Some(self.choose_gravity(percent)),
            GRENADES_ROW => Some(self.toggle_grenades()),
            TEAMS_ROW => Some(self.choose_teams(|i| next_step(i, TEAM_COUNTS.len(), right))),
            r => match (Scale::of_row(r), Slot::of_row(r)) {
                (Some(s), _) => Some(self.choose_scale(s, percent)),
                (_, Some(sl)) => Some(self.choose_weapon(sl, |i| {
                    next_step(i, ext_weapons::TABLE.len() + 1, right)
                })),
                _ => None,
            },
        }
    }
    fn description(&self, row: i16) -> Option<u32> {
        if let Some((i, _)) = self.option_of(row) {
            return Some(option_description_id(i));
        }
        match row {
            GRAVITY_ROW => Some(GRAVITY_DESCRIPTION_ID),
            GRENADES_ROW => Some(GRENADES_DESCRIPTION_ID),
            TEAMS_ROW => Some(TEAMS_DESCRIPTION_ID),
            r => Scale::of_row(r)
                .map(Scale::description_id)
                .or_else(|| Slot::of_row(r).map(Slot::description_id)),
        }
    }
    /// An edit goes to the record before the row is drawn again.
    fn live_text(&self) -> LiveText {
        let mut writes = Vec::new();
        if let Some(b) = self.pending() {
            writes.push((EDITED_BLOCK, b.to_vec()));
        }
        if let Some(t) = self.pending_teams() {
            writes.push((EDITED_TEAMS, t.to_le_bytes().to_vec()));
        }
        // The script options' text, rewritten where it changed; the list
        // grows or shrinks when the script's option count is not the
        // count it was built with.
        let mut redraw =
            self.options.built.load(Ordering::SeqCst) != ROWS + self.option_rows().len();
        let mut written = self
            .options
            .written
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for (id, text) in self.option_texts() {
            if written.iter().any(|(i, t)| *i == id && *t == text) {
                continue;
            }
            let Some(at) = crate::layout_glue::string_address(id) else {
                continue;
            };
            writes.push((at, served(&text, OPTION_ROOM)));
            written.retain(|(i, _)| *i != id);
            written.push((id, text));
            redraw = true;
        }
        LiveText { writes, redraw }
    }
    /// Reads the record's block and team count (when the page opens and
    /// at each row text).
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        let mut b = [0u8; ext::LEN];
        m.read(EDITED_BLOCK, &mut b)?;
        self.read(b);
        let mut t = [0u8; 4];
        m.read(EDITED_TEAMS, &mut t)?;
        self.read_teams(i32::from_le_bytes(t));
        Ok(())
    }
    fn describe(&self, _count: usize) -> Option<String> {
        let e = self.ext();
        let mut d = format!(
            "\"{TITLE}\"; rows: [0] \"{}\", [1] \"{}\", [2] \"{}\"",
            gravity_label(nearest_step(e.gravity)),
            grenades_label(e.unlimited_grenades),
            teams_label(teams_step(self.teams()))
        );
        for sc in Scale::ALL {
            d.push_str(&format!(
                ", [{}] \"{}\"",
                sc.row(),
                sc.label(nearest_step(sc.get(&e)))
            ));
        }
        for sl in Slot::ALL {
            d.push_str(&format!(
                ", [{}] \"{}\"",
                sl.row(),
                sl.label(ext_weapons::step(sl.get(&e)))
            ));
        }
        for (i, o) in self.option_rows().iter().enumerate() {
            d.push_str(&format!(
                ", [{}] \"{}\"",
                ROWS + i,
                o.label_for(e.script.options)
            ));
        }
        Some(d)
    }
}

impl Describe for Page {
    fn spec(&self) -> ScreenSpec {
        use crate::ui_kit::input::{Glyph, Input, Prompt};
        use crate::ui_kit::spec::{ListPlace, ValueStyle};
        let mut s = ScreenSpec::new(SCREEN, TITLE);
        s.ids = vec![TITLE_ID, FOOTER_ID];
        s.list = ListPlace::from(Some(crate::skulls_menu::LIST_LAYOUT));
        s.style = ValueStyle::Inline;
        s.description = Some(crate::skulls_menu::DESCRIPTION_TEXT.into());
        s.content.row_help = true;
        s.footer = true;
        // Nine fixed rows (gravity, grenades, teams, the four scales and the
        // two other weapons)
        // and a row per script option: the list scrolls where its room is
        // shorter.
        s.scrolls = true;
        s.prompts = vec![
            Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Change"),
            Prompt::new(&[Glyph::A], "Select"),
            Prompt::new(&[Glyph::B], "Back"),
        ];
        s.inputs = vec![
            Input::Up,
            Input::Down,
            Input::Left,
            Input::Right,
            Input::A,
            Input::B,
        ];
        let rows = (ROWS + self.option_rows().len()) as i16;
        s.rows = (0..rows).filter_map(|row| self.row_spec(row)).collect();
        s
    }
}

/// The page on the skulls list layout. A opens a value picker or
/// toggles Unlimited Grenades.
pub fn class(page: Arc<Page>, block: TextBlock, layout: ListLayout) -> ListClass {
    ListClass::with_rows(SCREEN_FEATURE, SCREEN, page)
        .description(block)
        .layout(ListLayout {
            visible: 0,
            ..layout
        })
        .sideways()
        .footer(FOOTER_ID)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skulls_menu;
    use variants::record::{ENGINE_AT, NAME_AT, RECORD_SIZE};

    /// The edited record's address agrees with what the game's code says
    /// about it: its name at `+4` (`0x2387c6` copies 32 units to
    /// `0x54e4a4`), its engine at `+0x44` (the row chosen reads
    /// `[0x54e4e4]`), and `0x148a8d` zeroes 0x4c dwords from `0x54e4a0`,
    /// one whole record. `edited_record_is_where_the_xbe_says` checks the
    /// instructions on the user's XBE.
    #[test]
    fn the_edited_record_starts_at_0x54e4a0() {
        assert_eq!(EDITED_VARIANT + NAME_AT as u32, 0x0054_e4a4);
        assert_eq!(EDITED_VARIANT + ENGINE_AT as u32, 0x0054_e4e4);
        assert_eq!(0x4c * 4, RECORD_SIZE);
        assert_eq!(EDITED_BLOCK, 0x0054_e5ac);
    }

    #[test]
    #[ignore = "needs the user's XBE ($H2_XBE)"]
    fn edited_record_is_where_the_xbe_says() {
        let data = std::fs::read(std::env::var("H2_XBE").expect("set H2_XBE")).unwrap();
        let x = xbe::Xbe::parse(&data).unwrap();
        let at = |va: u32, len: usize| x.bytes_at(va, len).unwrap().to_vec();
        let le = |v: u32| v.to_le_bytes().to_vec();
        // 0x148a95: push 0x4c; pop ecx; xor eax,eax; mov edi,0x54e4a0; rep stosd
        assert_eq!(at(0x0014_8a95, 5), [0x6a, 0x4c, 0x59, 0x33, 0xc0]);
        assert_eq!(at(0x0014_8a9a, 1), [0xbf]);
        assert_eq!(at(0x0014_8a9b, 4), le(EDITED_VARIANT));
        assert_eq!(at(0x0014_8a9f, 2), [0xf3, 0xab]);
        // 0x2387c6: mov edi,0x54e4a4 (the name), 32 units in esi before it.
        assert_eq!(at(0x0023_87c3, 2), [0x6a, 0x20]);
        assert_eq!(at(0x0023_87c7, 4), le(EDITED_VARIANT + NAME_AT as u32));
        // 0x2b7630: mov eax,[0x54e4e4] (the engine).
        assert_eq!(at(0x002b_7630, 1), [0xa1]);
        assert_eq!(at(0x002b_7631, 4), le(EDITED_VARIANT + ENGINE_AT as u32));
    }

    #[test]
    fn our_row_follows_the_skulls_row_and_grows_the_list_once() {
        assert_eq!(more_row_with(true), 7);
        assert_eq!(more_row_with(false), 6);
        assert_eq!(variant_count(VARIANT_LIST_NAME, 6, true, true), 8);
        assert_eq!(variant_count(VARIANT_LIST_NAME, 6, false, true), 7);
        assert_eq!(variant_count(VARIANT_LIST_NAME, 6, true, false), 7);
        assert_eq!(variant_count(VARIANT_LIST_NAME, 8, true, true), 8, "once");
        assert_eq!(variant_count(0x1234, 6, true, true), 6, "another list");
        assert!(skip_pane_with(VARIANT_SCREEN_ID, 7, true, true));
        assert!(skip_pane_with(VARIANT_SCREEN_ID, 6, true, false));
        assert!(!skip_pane_with(VARIANT_SCREEN_ID, 7, false, true));
        assert!(!skip_pane_with(VARIANT_SCREEN_ID, 5, true, true));
        assert!(!skip_pane_with(1, 7, true, true));
    }

    #[test]
    fn a_press_opens_the_page_on_the_editors_stack() {
        let r = opening(Some(1), Some((5, 4)), Some(0xa0f0_6000)).unwrap();
        assert_eq!(r, crate::skulls_mp::open_request(2, 5, 4, 0xa0f0_6000));
        assert!(opening(None, Some((5, 4)), Some(1)).is_err());
        assert!(opening(Some(0), None, Some(1)).is_err());
        assert!(
            opening(Some(0), Some((5, 4)), None)
                .unwrap_err()
                .contains("class is off")
        );
    }

    /// The page reads the record's block, steps gravity, and hands the new
    /// block back once, in the record's own format.
    #[test]
    fn the_page_edits_the_records_block() {
        let p = Page::new();
        p.read([0; ext::LEN]);
        assert_eq!(p.ext(), Ext::default());
        assert_eq!(p.texts(0), [(LABEL_SLOT, gravity_label_id(4))]);
        assert_eq!(p.pending(), None);
        assert!(p.sideways(0, false).unwrap().contains("100% -> 75%"));
        // An edit is pending until written; a read meanwhile keeps it.
        p.read([0; ext::LEN]);
        let b = p.pending().unwrap();
        assert_eq!(Ext::in_effect(&b).gravity, 75);
        assert_eq!(p.pending(), None, "once");
        assert_eq!(p.live_text().writes, vec![]);
        // Left at the bottom stops. A opens a picker on the held value.
        for _ in 0..10 {
            p.sideways(0, false);
        }
        assert_eq!(p.ext().gravity, 10);
        assert!(p.sideways(0, false).unwrap().contains("stays"));
        assert_eq!(p.press(GRAVITY_ROW), Chosen::Open(picker::NAME));
        let pick = picker::current().expect("gravity picker");
        assert_eq!(pick.title, "GRAVITY");
        assert_eq!(pick.current, 0);
        assert_eq!(pick.values, GRAVITY_STEPS.map(percent_choice));
        assert_eq!(p.ext().gravity, 10, "opening the picker leaves the value");
        assert!((pick.set)(1).contains("10% -> 25%"));
        assert_eq!(p.ext().gravity, 25);
        let w = p.live_text().writes;
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].0, EDITED_BLOCK);
        assert_eq!(
            Ext::in_effect(&w[0].1.clone().try_into().unwrap()).gravity,
            25
        );
        // Back to normal clears the block to retail zeros.
        let p = Page::new();
        let mut e = Ext {
            gravity: 125,
            ..Default::default()
        };
        p.read(e.encode());
        p.sideways(0, false);
        assert_eq!(p.pending(), Some([0; ext::LEN]));
        // Studio's 60% shows as the nearest step and steps from there.
        let p = Page::new();
        e.gravity = 60;
        p.read(e.encode());
        assert_eq!(p.texts(0), [(LABEL_SLOT, gravity_label_id(2))]);
        p.sideways(0, true);
        assert_eq!(p.ext().gravity, 75);
        assert_eq!(p.press(ROWS as i16), Chosen::Nothing);
    }

    /// The grenades row toggles the flag, beside gravity, and off again
    /// with nothing else set leaves a retail record.
    #[test]
    fn the_grenades_row_toggles_the_flag() {
        let p = Page::new();
        p.read([0; ext::LEN]);
        assert_eq!(p.count(), ROWS);
        assert_eq!(
            p.texts(GRENADES_ROW),
            [(LABEL_SLOT, grenades_label_id(false))]
        );
        assert_eq!(p.description(GRENADES_ROW), Some(GRENADES_DESCRIPTION_ID));
        assert!(matches!(p.press(GRENADES_ROW), Chosen::Stepped(l) if l.ends_with("on")));
        let b = p.pending().unwrap();
        assert!(Ext::in_effect(&b).unlimited_grenades);
        assert_eq!(
            p.texts(GRENADES_ROW),
            [(LABEL_SLOT, grenades_label_id(true))]
        );
        p.sideways(GRAVITY_ROW, false);
        assert_eq!(p.ext().gravity, 75);
        assert!(p.ext().unlimited_grenades, "gravity keeps the flag");
        assert!(p.sideways(GRENADES_ROW, true).unwrap().ends_with("off"));
        p.sideways(GRAVITY_ROW, true);
        assert_eq!(p.pending(), Some([0; ext::LEN]), "a retail record again");
        assert!(p.describe(2).unwrap().contains("Unlimited Grenades: Off"));
    }

    /// The team count row edits the record's `+0xb4`, not the block, and
    /// steps through the counts we offer.
    #[test]
    fn the_team_count_row_edits_the_records_field() {
        assert_eq!(EDITED_TEAMS, 0x0054_e554);
        let p = Page::new();
        p.read_teams(0);
        assert_eq!(p.count(), ROWS);
        assert_eq!(p.texts(TEAMS_ROW), [(LABEL_SLOT, teams_label_id(0))]);
        assert_eq!(p.description(TEAMS_ROW), Some(TEAMS_DESCRIPTION_ID));
        assert!(p.sideways(TEAMS_ROW, false).unwrap().contains("stays 0"));
        assert!(p.sideways(TEAMS_ROW, true).unwrap().contains("0 -> 2"));
        // A read meanwhile keeps the edit.
        p.read_teams(0);
        let w = p.live_text().writes;
        assert_eq!(w, vec![(EDITED_TEAMS, 2i32.to_le_bytes().to_vec())]);
        assert_eq!(p.pending(), None, "the block is untouched");
        assert!(p.live_text().writes.is_empty(), "once");
        for _ in 0..10 {
            p.sideways(TEAMS_ROW, true);
        }
        assert_eq!(p.teams(), 8);
        assert_eq!(p.press(TEAMS_ROW), Chosen::Open(picker::NAME));
        let pick = picker::current().expect("team count picker");
        assert_eq!(pick.current, TEAM_COUNTS.len() - 1);
        assert_eq!(pick.values, TEAM_COUNTS.map(team_choice));
        assert_eq!(p.teams(), 8, "opening the picker leaves the value");
        assert!((pick.set)(0).contains("8 -> 0"));
        assert_eq!(p.pending_teams(), Some(0));
        // A stored 1 (not offered) shows as 2 and steps from there.
        let p = Page::new();
        p.read_teams(1);
        assert_eq!(p.texts(TEAMS_ROW), [(LABEL_SLOT, teams_label_id(1))]);
        p.sideways(TEAMS_ROW, true);
        assert_eq!(p.teams(), 3);
        // Both edits go out together.
        p.toggle_grenades();
        assert_eq!(p.live_text().writes.len(), 2);
        assert_eq!(teams_label(0), "Team Count: Map Default");
        assert_eq!(teams_label(3), "Team Count: 4");
    }

    /// The damage, shield and health rows step their percents in the
    /// block, each on its own.
    #[test]
    fn the_weapon_rows_pick_from_the_table_and_write_the_block() {
        let p = Page::default();
        p.read([0; ext::LEN]);
        assert_eq!(Slot::of_row(7), Some(Slot::Primary));
        assert_eq!(Slot::of_row(8), Some(Slot::Secondary));
        assert_eq!(Slot::of_row(9), None);
        assert_eq!(Slot::Secondary.row() as usize, ROWS - 1);
        for sl in Slot::ALL {
            assert_eq!(p.texts(sl.row()), [(LABEL_SLOT, sl.label_id(0))]);
            assert_eq!(p.description(sl.row()), Some(sl.description_id()));
            assert_eq!(p.press(sl.row()), Chosen::Open(picker::NAME));
        }
        assert_eq!(Slot::Primary.label(0), "Primary: Not Used");
        let hammer = ext_weapons::TABLE[1];
        assert_eq!(
            Slot::Secondary.label(2),
            format!("Secondary: {}", hammer.label)
        );
        // Right steps up the table and stops at its end; left comes back.
        let last = ext_weapons::TABLE.len();
        for _ in 0..last + 2 {
            p.sideways(Slot::Primary.row(), true);
        }
        assert_eq!(p.ext().primary, ext_weapons::TABLE[last - 1].id);
        assert_eq!(p.ext().secondary, 0);
        assert!(p.pending().is_some());
        p.sideways(Slot::Primary.row(), false);
        assert_eq!(p.ext().primary, ext_weapons::TABLE[last - 2].id);
        assert_eq!(
            p.sideways(Slot::Secondary.row(), false).as_deref(),
            Some("variant options: secondary stays Not Used")
        );
        // A choice the picker makes sets the slot; one past the table is refused.
        assert!(p.choose_weapon(Slot::Secondary, |_| 2).contains("Gravity Hammer"));
        assert_eq!(p.ext().secondary, hammer.id);
        assert!(p.choose_weapon(Slot::Secondary, |_| 99).contains("no secondary choice at 99"));
        assert!(p.describe(ROWS).unwrap().contains(&format!("[8] \"Secondary: {}\"", hammer.label)));
        // Every label and description is served.
        let served = strings();
        for sl in Slot::ALL {
            assert!(served.iter().any(|(i, _)| *i == sl.description_id()));
            for i in 0..=last {
                assert!(served.iter().any(|(id, t)| *id == sl.label_id(i) && *t == sl.label(i)));
            }
        }
        // The rows' ids are distinct from every other family id.
        let mut ids: Vec<u32> = served.iter().map(|(i, _)| *i).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }

    #[test]
    fn the_scale_rows_step_their_percents() {
        let p = Page::new();
        p.read([0; ext::LEN]);
        assert_eq!(Scale::of_row(3), Some(Scale::Damage));
        assert_eq!(Scale::of_row(5), Some(Scale::Health));
        assert_eq!(Scale::of_row(6), Some(Scale::Speed));
        assert_eq!(Scale::of_row(7), None);
        for sc in Scale::ALL {
            assert_eq!(p.texts(sc.row()), [(LABEL_SLOT, sc.label_id(4))]);
            assert_eq!(p.description(sc.row()), Some(sc.description_id()));
        }
        assert!(
            p.sideways(Scale::Damage.row(), true)
                .unwrap()
                .contains("damage 100% -> 125%")
        );
        assert!(
            p.sideways(Scale::Health.row(), false)
                .unwrap()
                .contains("health 100% -> 75%")
        );
        assert_eq!(p.press(Scale::Shield.row()), Chosen::Open(picker::NAME));
        let pick = picker::current().expect("shield picker");
        assert_eq!(pick.values[pick.current], "Normal");
        assert!((pick.set)(5).contains("100% -> 125%"));
        let e = Ext::in_effect(&p.pending().unwrap());
        assert_eq!(
            (e.damage, e.shield, e.health, e.gravity),
            (125, 125, 75, 100)
        );
        assert_eq!(
            p.texts(Scale::Damage.row()),
            [(LABEL_SLOT, Scale::Damage.label_id(5))]
        );
        // Left at the bottom stops at 10%.
        for _ in 0..12 {
            p.sideways(Scale::Health.row(), false);
        }
        assert_eq!(p.ext().health, 10);
        assert!(
            p.sideways(Scale::Health.row(), false)
                .unwrap()
                .contains("stays 10%")
        );
        assert_eq!(Scale::Shield.label(4), "Shields: Normal");
        assert_eq!(Scale::Damage.label(7), "Damage: 200%");
        assert!(p.describe(ROWS).unwrap().contains("[5] \"Health: 10%\""));
    }

    #[test]
    fn strings_are_ours_and_distinct() {
        let v = strings();
        let mut ids: Vec<u32> = v.iter().map(|(i, _)| *i).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), v.len());
        assert!(ids.iter().all(|i| IDS.owns(*i)));
        assert!(ids.iter().all(|i| crate::layout_glue::is_ours(*i)));
        assert_eq!(gravity_label(2), "Gravity: 50%");
        assert_eq!(gravity_label(4), "Gravity: Normal");
        assert!(GRAVITY_DESCRIPTION.encode_utf16().count() <= 120);
        assert!(GRENADES_DESCRIPTION.encode_utf16().count() <= 120);
        assert!(TEAMS_DESCRIPTION.encode_utf16().count() <= 120);
        for sc in Scale::ALL {
            assert!(sc.description().encode_utf16().count() <= 120, "{sc:?}");
        }
        for g in GRAVITY_STEPS {
            assert!(ext::PERCENT.contains(&g));
        }
        assert_eq!(grenades_label(true), "Unlimited Grenades: On");
        for g in GRAVITY_STEPS {
            assert!(ext::GRAVITY.contains(&g));
        }
    }

    /// The Skulls row hooks the same two points; with both on, the plan
    /// builds and runs the Skulls row's hook first (a refused plan stops
    /// the game at boot).
    #[test]
    fn the_row_plans_beside_the_skulls_row() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        let store = Arc::new(settings::Store::defaults(crate::setting_defs::ALL));
        let mp = Arc::new(skulls_menu::SkullsScreen::multiplayer(store));
        r.add(&crate::skulls_mp_row::VariantRow(mp)).unwrap();
        r.add(&MoreRow).unwrap();
        let text = r.build(&hooks::Base).unwrap().text();
        for point in ["VARIANT_OPTIONS_ROW_TEXT", "VARIANT_OPTIONS_ROW_CHOSEN"] {
            let line = text.lines().find(|l| l.contains(point)).unwrap();
            let skulls = line.find("skulls-mp-row").unwrap();
            let more = line.find("variant-more-options-row").unwrap();
            assert!(skulls < more, "{line}");
        }
    }

    #[test]
    fn the_row_and_the_page_are_on_the_plan() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        r.add(&class(
            Arc::new(Page::new()),
            skulls_menu::DESCRIPTION_TEXT,
            skulls_menu::LIST_LAYOUT,
        ))
        .unwrap();
        r.add(&MoreRow).unwrap();
        let text = r.build(&hooks::Base).unwrap().text();
        for line in [
            "VARIANT_OPTIONS_ROW_TEXT: before [variant-more-options-row:the More Options row's text]",
            "VARIANT_OPTIONS_ROW_CHOSEN: before [variant-more-options-row:the More Options row's press]",
            "entry variant-more-options:loader",
        ] {
            assert!(text.contains(line), "{line}\n{text}");
        }
        assert!(row_on());
        assert!(list_class::loader_of(SCREEN).is_some());
    }

    // ---- script options ----

    const PREFIX: [u8; 8] = [7; 8];

    /// A script's four options, packed as the IR packs them: a bool (bit
    /// 0), an enum of three values (bits 1 and 2), an int 1..5 (bits 3 to
    /// 5) and an int 0..1000 (bits 6 to 15).
    fn script_rows() -> Vec<OptionRow> {
        let row = |label: &str, kind, lo, hi, offset, bits| OptionRow {
            label: label.into(),
            description: format!("About {label}."),
            kind,
            lo,
            hi,
            offset,
            bits,
        };
        let names = ["First", "Second", "Third"].map(String::from).to_vec();
        vec![
            row("Low gravity", OptionKind::Bool, 0, 1, 0, 1),
            row("Win by", OptionKind::Enum(names), 0, 2, 1, 2),
            row("First find", OptionKind::Int, 1, 5, 3, 3),
            row("Ball reset", OptionKind::Int, 0, 1000, 6, 10),
        ]
    }

    /// A page whose only script is the one at [`PREFIX`].
    fn scripted() -> Page {
        Page::with_scripts(Arc::new(|p| (p == PREFIX).then(script_rows)))
    }

    fn naming_the_script(e: Ext, options: u32) -> [u8; ext::LEN] {
        Ext {
            script: ext::Script {
                mode: ext::Mode::Rules,
                prefix: PREFIX,
                options,
            },
            ..e
        }
        .encode()
    }

    #[test]
    fn option_rows_read_and_write_their_own_bits() {
        let o = script_rows();
        let bits = o[3].set(o[2].set(o[1].set(o[0].set(0, 1), 2), 4), 900);
        assert_eq!(
            o.iter().map(|r| r.get(bits)).collect::<Vec<_>>(),
            [1, 2, 4, 900]
        );
        // One field changes, the others stay, and a value is kept in range.
        let b = o[1].set(bits, 0);
        assert_eq!(
            o.iter().map(|r| r.get(b)).collect::<Vec<_>>(),
            [1, 0, 4, 900]
        );
        assert_eq!(o[2].get(o[2].set(0, 99)), 5);
        assert_eq!(o[2].get(0), 1, "a zero field is `lo`");
        assert_eq!(o[0].label_for(bits), "Low gravity: On");
        assert_eq!(o[1].label_for(bits), "Win by: Third");
        assert_eq!(o[2].choices().unwrap(), ["1", "2", "3", "4", "5"]);
        assert_eq!(o[3].choices(), None, "a wide int only steps");
        assert_eq!(o[3].step(0, true), 62, "by a sixteenth of the range");
        assert_eq!(o[2].step(5, true), 5);
        assert_eq!(o[2].step(1, false), 1);
        assert_eq!(titled("first_to"), "First to");
    }

    #[test]
    fn a_row_per_option_after_the_fixed_rows() {
        let p = scripted();
        let o = script_rows();
        let bits = o[2].set(0, 3);
        assert_eq!(p.count(), ROWS, "before the record is read");
        p.read(naming_the_script(Ext::default(), bits));
        // The list was built for the fixed rows: it asks to grow.
        assert!(p.live_text().redraw);
        assert_eq!(p.count(), ROWS + 4);
        assert!(!p.live_text().redraw, "built to the count it has");
        assert_eq!(p.growing_capacity(), Some(ROWS + MAX_OPTION_ROWS));
        assert!(p.shrink_growing_rows());
        // The fixed rows are as they were.
        assert_eq!(p.texts(GRAVITY_ROW), [(LABEL_SLOT, gravity_label_id(4))]);
        assert_eq!(
            p.texts(Scale::Speed.row()),
            [(LABEL_SLOT, Scale::Speed.label_id(4))]
        );
        // Then one row per option, in the script's order.
        for i in 0..4 {
            let row = (ROWS + i) as i16;
            assert_eq!(p.texts(row), [(LABEL_SLOT, option_label_id(i))]);
            assert_eq!(p.description(row), Some(option_description_id(i)));
        }
        assert!(p.texts((ROWS + 4) as i16).is_empty());
        assert_eq!(p.description((ROWS + 4) as i16), None);
        let texts = p.option_texts();
        assert_eq!(texts[0], (option_label_id(0), "Low gravity: Off".into()));
        assert_eq!(
            texts[1],
            (option_description_id(0), "About Low gravity.".into())
        );
        assert_eq!(texts[4], (option_label_id(2), "First find: 3".into()));
        assert_eq!(texts[6], (option_label_id(3), "Ball reset: 0".into()));
        // The kit sees the same rows.
        let spec = p.spec();
        assert_eq!(spec.rows.len(), ROWS + 4);
        assert_eq!(spec.rows[ROWS + 1].label, "Win by");
        assert_eq!(spec.rows[ROWS + 2].shown_value().as_deref(), Some("3"));
        assert_eq!(
            spec.rows[ROWS + 2].ids,
            [option_label_id(2), option_description_id(2)]
        );
        assert!(
            p.describe(ROWS + 4)
                .unwrap()
                .contains(&format!("[{}] \"First find: 3\"", ROWS + 2))
        );
        // A bool, an enum and a small int open a picker on the held value.
        assert_eq!(p.press(ROWS as i16 + 2), Chosen::Open(picker::NAME));
        let pick = picker::current().expect("option picker");
        assert_eq!(pick.values, ["1", "2", "3", "4", "5"]);
        assert_eq!(pick.current, 2);
        // A wide int has no picker.
        assert_eq!(p.press(ROWS as i16 + 3), Chosen::Nothing);
        // Every id is ours and the strings hold room for the text.
        let v = strings();
        assert!(
            v.iter()
                .any(|(i, t)| *i == option_label_id(MAX_OPTION_ROWS - 1)
                    && t.len() == OPTION_ROOM - 1)
        );
        assert!(option_description_id(MAX_OPTION_ROWS - 1) < IDS.base + IDS.len);
        assert_eq!(served("é", 2), [0]);
        assert_eq!(served("ab", 8), b"ab\0");
    }

    #[test]
    fn a_step_writes_the_option_bits_and_keeps_the_extras() {
        let p = scripted();
        let o = script_rows();
        let start = o[2].set(o[1].set(0, 1), 4);
        let extras = Ext {
            gravity: 50,
            unlimited_grenades: true,
            health: 200,
            ..Default::default()
        };
        p.read(naming_the_script(extras, start));
        let enum_row = ROWS as i16 + 1;
        assert!(
            p.sideways(enum_row, true)
                .unwrap()
                .contains("Second -> Third")
        );
        let n = Ext::in_effect(&p.pending().expect("one write"));
        assert_eq!(n.script.options, o[2].set(o[1].set(0, 2), 4));
        assert_eq!((n.script.mode, n.script.prefix), (ext::Mode::Rules, PREFIX));
        assert_eq!((n.gravity, n.health), (50, 200));
        assert!(n.unlimited_grenades);
        assert_eq!(p.pending(), None, "once");
        // The top stops; the page shows the new value.
        p.read(naming_the_script(extras, o[2].set(o[1].set(0, 2), 4)));
        assert!(p.sideways(enum_row, true).unwrap().contains("stays Third"));
        assert_eq!(p.option_texts()[2].1, "Win by: Third");
        // The picker writes the same field, and the write goes to the
        // record's block.
        assert_eq!(p.press(enum_row), Chosen::Open(picker::NAME));
        let pick = picker::current().expect("option picker");
        assert_eq!(pick.values, ["First", "Second", "Third"]);
        assert_eq!(pick.current, 2);
        assert!((pick.set)(0).contains("Third -> First"));
        // Option texts may be redrawn beside it (another test in the
        // process can leave the built count); the record is one write.
        let w: Vec<_> = p.live_text().writes.into_iter().filter(|(id, _)| *id == EDITED_BLOCK).collect();
        assert_eq!(w.len(), 1);
        let n = Ext::in_effect(&w[0].1.clone().try_into().unwrap());
        assert_eq!(n.script.options, o[2].set(0, 4));
        assert_eq!((n.gravity, n.health, n.unlimited_grenades), (50, 200, true));
        // A fixed row's edit keeps the script and its options.
        p.sideways(GRAVITY_ROW, true);
        let n = Ext::in_effect(&p.pending().unwrap());
        assert_eq!((n.gravity, n.script.options), (75, o[2].set(0, 4)));
        // A wide int steps by a sixteenth and stops at its ends.
        let wide = ROWS as i16 + 3;
        assert!(p.sideways(wide, true).unwrap().contains("0 -> 62"));
        assert_eq!(
            Ext::in_effect(&p.pending().unwrap()).script.options & !0x3f,
            62 << 6
        );
        for _ in 0..40 {
            p.sideways(wide, true);
        }
        assert!(p.sideways(wide, true).unwrap().contains("stays 1000"));
    }

    #[test]
    fn a_record_with_no_script_has_no_option_rows() {
        // The resolver is never asked about a record that names no script.
        let asked = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&asked);
        let p = Page::with_scripts(Arc::new(move |_| {
            seen.store(true, Ordering::SeqCst);
            Some(script_rows())
        }));
        p.read([0; ext::LEN]);
        assert!(p.option_rows().is_empty());
        assert_eq!(p.count(), ROWS);
        assert!(!p.live_text().redraw);
        assert!(p.option_texts().is_empty());
        assert_eq!(p.press(ROWS as i16), Chosen::Nothing);
        assert_eq!(p.sideways(ROWS as i16, true), None);
        assert_eq!(p.spec().rows.len(), ROWS);
        assert!(!asked.load(Ordering::SeqCst));
        // A block with extras but no script.
        p.read(
            Ext {
                gravity: 50,
                ..Default::default()
            }
            .encode(),
        );
        assert_eq!(p.count(), ROWS);
        // A firefight record is not a rules script.
        let fire = Ext {
            script: ext::Script {
                mode: ext::Mode::Firefight,
                prefix: PREFIX,
                options: 0,
            },
            ..Default::default()
        };
        p.read(fire.encode());
        assert_eq!(p.count(), ROWS);
        assert!(!asked.load(Ordering::SeqCst));
        // A script this machine lacks shows the fixed rows only.
        let other = Page::with_scripts(Arc::new(|_| None));
        other.read(naming_the_script(Ext::default(), 0));
        assert_eq!(other.count(), ROWS);
        assert_eq!(other.press(ROWS as i16), Chosen::Nothing);
        // The same page drops the rows when it opens on another record.
        let p = scripted();
        p.read(naming_the_script(Ext::default(), 0));
        assert_eq!(p.count(), ROWS + 4);
        p.read([0; ext::LEN]);
        assert_eq!(p.count(), ROWS);
        assert!(!p.live_text().redraw);
    }
}
