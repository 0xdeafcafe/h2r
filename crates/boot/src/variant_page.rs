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
//! **Which options.** Only those the game applies: gravity (slice 3),
//! unlimited grenades and the team count (slice 5). The others join the
//! page with their slices. The team count is a retail field with no
//! retail row (setting 93, [`variants::record::TEAM_COUNT`]), so its row
//! writes the record's `+0xb4` rather than the block.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use engine::entry::{EntryFuture, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use feature::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;
use variants::ext::{self, Ext};

use crate::layout_glue::chosen_row;
use crate::list_class::{self, ListClass, ListLayout, LiveText, Rows, TextBlock};
use crate::skulls_mp::{
    SCREEN_SLOT, SCREEN_WINDOW, VARIANT_LIST_NAME, VARIANT_ROWS, VARIANT_SCREEN_ID,
};
use crate::ui::{Chosen, LABEL_SLOT};
use crate::ui_kit::ids::{self, Family};

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
pub const PROMPT: &str = "\u{e10a}  \u{e10b} CHANGE  \u{e101} BACK";
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

/// The damage, shield and health scales: their rows, after the team count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    Damage,
    Shield,
    Health,
}

impl Scale {
    pub const ALL: [Scale; 3] = [Scale::Damage, Scale::Shield, Scale::Health];

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
        }
    }

    pub fn get(self, e: &Ext) -> u16 {
        match self {
            Scale::Damage => e.damage,
            Scale::Shield => e.shield,
            Scale::Health => e.health,
        }
    }

    pub fn set(self, e: &mut Ext, v: u16) {
        match self {
            Scale::Damage => e.damage = v,
            Scale::Shield => e.shield = v,
            Scale::Health => e.health = v,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Scale::Damage => "Damage every player deals, as a share of the game's. Not on System Link.",
            Scale::Shield => "How much every player's shield holds, as a share of the game's. Not on System Link.",
            Scale::Health => "How much every player's body holds, as a share of the game's. Not on System Link.",
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

    /// Step `i`'s label: "Damage: < 150% >"; the game's own says Normal.
    pub fn label(self, i: usize) -> String {
        match GRAVITY_STEPS[i] {
            100 => format!("{}: < Normal >", self.name()),
            v => format!("{}: < {v}% >", self.name()),
        }
    }
}

/// The page's rows.
pub const GRAVITY_ROW: i16 = 0;
pub const GRENADES_ROW: i16 = 1;
pub const TEAMS_ROW: i16 = 2;
pub const ROWS: usize = 6;

/// The team count in the edited record (`+0xb4`, an i32).
pub const EDITED_TEAMS: u32 = EDITED_VARIANT + 0xb4;
const TEAM_COUNTS: [i32; 8] = variants::record::TEAM_COUNTS;

/// The scales' ids: per scale a description, then a label per step (the
/// gravity steps, which are [`ext::PERCENT`]'s too).
const SCALE_IDS: u32 = TEAMS_LABELS + TEAM_COUNTS.len() as u32;

const _: () = assert!(SCALE_IDS + 3 * (1 + GRAVITY_STEPS.len() as u32) <= IDS.len);

/// The step of team count `t`: its place in [`TEAM_COUNTS`], or 2 teams
/// for a value we don't offer (1).
pub fn teams_step(t: i32) -> usize {
    TEAM_COUNTS.iter().position(|&c| c == t).unwrap_or(1)
}

/// The label id of team count step `i`.
pub fn teams_label_id(i: usize) -> u32 {
    IDS.id(TEAMS_LABELS + i as u32)
}

/// Team count step `i`'s label: "Team Count: < 4 >".
pub fn teams_label(i: usize) -> String {
    match TEAM_COUNTS[i] {
        0 => "Team Count: < Map Default >".into(),
        t => format!("Team Count: < {t} >"),
    }
}

/// The label id of the grenades row, off or on.
pub fn grenades_label_id(on: bool) -> u32 {
    IDS.id(GRENADES_LABELS + u32::from(on))
}

/// The grenades row's label.
pub fn grenades_label(on: bool) -> String {
    format!(
        "Unlimited Grenades: < {} >",
        if on { "On" } else { "Off" }
    )
}

/// The label id of gravity step `i`.
pub fn gravity_label_id(i: usize) -> u32 {
    IDS.id(GRAVITY_LABELS + i as u32)
}

/// Gravity step `i`'s label: "Gravity: < 50% >"; the game's own says so.
pub fn gravity_label(i: usize) -> String {
    match GRAVITY_STEPS[i] {
        100 => "Gravity: < Normal >".into(),
        g => format!("Gravity: < {g}% >"),
    }
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
        let at = c.cpu.ecx.wrapping_sub(engine::view::ui::LIST);
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

impl feature::Feature for MoreRow {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ROW_FEATURE,
            "The More Options row in the game variant editor",
        )
        .doc("docs/game/game-options.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use engine::points::{VARIANT_OPTIONS_ROW_CHOSEN, VARIANT_OPTIONS_ROW_TEXT};
        let request_at = b.guest_alloc(0x1000, Permissions::RW)?;
        b.raw().before(
            &VARIANT_OPTIONS_ROW_TEXT,
            "the More Options row's text",
            row_text,
        )
        // The Skulls row hooks both points too. The two act on different
        // rows (more_row() sits after the Skulls row), so the order is
        // only for the plan: ours runs inside the Skulls row's hooks.
        .after_feature(crate::skulls_mp_row::ROW_FEATURE);
        b.raw().before(
            &VARIANT_OPTIONS_ROW_CHOSEN,
            "the More Options row's press",
            move |c| row_chosen(c, request_at),
        )
        .after_feature(crate::skulls_mp_row::ROW_FEATURE);
        ROW_ON.store(true, Ordering::SeqCst);
        Ok(())
    }
}

// ---- the page ----

/// The More Options page: rows over the edited record's block.
pub struct Page {
    /// The block as last read from the record, with our edits.
    block: Mutex<[u8; ext::LEN]>,
    /// An edit not yet written to the record.
    dirty: AtomicBool,
    /// The record's team count (`+0xb4`), with our edit.
    teams: Mutex<i32>,
    teams_dirty: AtomicBool,
}

impl Default for Page {
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

    /// The options shown (the game's own for a block the game ignores).
    pub fn ext(&self) -> Ext {
        Ext::in_effect(&self.block.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// The record's block, read into the page (unless an edit is pending).
    pub fn read(&self, block: [u8; ext::LEN]) {
        if !self.dirty.load(Ordering::SeqCst) {
            *self.block.lock().unwrap_or_else(|e| e.into_inner()) = block;
        }
    }

    /// The write the record needs, once per edit.
    pub fn pending(&self) -> Option<[u8; ext::LEN]> {
        self.dirty
            .swap(false, Ordering::SeqCst)
            .then(|| *self.block.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Steps gravity up (`up`) or down a step; `wrap` goes round at the
    /// ends (A), else it stops there (left, right). What to log.
    pub fn step_gravity(&self, up: bool, wrap: bool) -> String {
        self.step_percent(
            "gravity",
            |e| e.gravity,
            |e, v| e.gravity = v,
            up,
            wrap,
        )
    }

    /// Steps a scale as gravity steps. What to log.
    pub fn step_scale(&self, sc: Scale, up: bool, wrap: bool) -> String {
        let what = sc.name().to_ascii_lowercase();
        self.step_percent(&what, |e| sc.get(e), |e, v| sc.set(e, v), up, wrap)
    }

    fn step_percent(
        &self,
        what: &str,
        get: impl Fn(&Ext) -> u16,
        set: impl Fn(&mut Ext, u16),
        up: bool,
        wrap: bool,
    ) -> String {
        let mut b = self.block.lock().unwrap_or_else(|e| e.into_inner());
        let mut e = Ext::in_effect(&b);
        let i = nearest_step(get(&e));
        let n = GRAVITY_STEPS.len();
        let j = match (up, wrap) {
            (true, true) => (i + 1) % n,
            (false, true) => (i + n - 1) % n,
            (true, false) => (i + 1).min(n - 1),
            (false, false) => i.saturating_sub(1),
        };
        let was = get(&e);
        set(&mut e, GRAVITY_STEPS[j]);
        if GRAVITY_STEPS[j] == was {
            return format!("variant options: {what} stays {was}%");
        }
        *b = e.encode();
        self.dirty.store(true, Ordering::SeqCst);
        format!("variant options: {what} {was}% -> {}%", GRAVITY_STEPS[j])
    }

    /// The record's team count, as shown.
    pub fn teams(&self) -> i32 {
        *self.teams.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The record's team count, read into the page (unless an edit is
    /// pending).
    pub fn read_teams(&self, t: i32) {
        if !self.teams_dirty.load(Ordering::SeqCst) {
            *self.teams.lock().unwrap_or_else(|e| e.into_inner()) = t;
        }
    }

    /// The team count write the record needs, once per edit.
    pub fn pending_teams(&self) -> Option<i32> {
        self.teams_dirty
            .swap(false, Ordering::SeqCst)
            .then(|| self.teams())
    }

    /// Steps the team count as gravity steps. What to log.
    pub fn step_teams(&self, up: bool, wrap: bool) -> String {
        let mut t = self.teams.lock().unwrap_or_else(|e| e.into_inner());
        let i = teams_step(*t);
        let n = TEAM_COUNTS.len();
        let j = match (up, wrap) {
            (true, true) => (i + 1) % n,
            (false, true) => (i + n - 1) % n,
            (true, false) => (i + 1).min(n - 1),
            (false, false) => i.saturating_sub(1),
        };
        let was = *t;
        if TEAM_COUNTS[j] == was {
            return format!("variant options: team count stays {was}");
        }
        *t = TEAM_COUNTS[j];
        self.teams_dirty.store(true, Ordering::SeqCst);
        format!("variant options: team count {was} -> {}", *t)
    }

    /// Turns unlimited grenades on or off. What to log.
    pub fn toggle_grenades(&self) -> String {
        let mut b = self.block.lock().unwrap_or_else(|e| e.into_inner());
        let mut e = Ext::in_effect(&b);
        e.unlimited_grenades = !e.unlimited_grenades;
        *b = e.encode();
        self.dirty.store(true, Ordering::SeqCst);
        format!(
            "variant options: unlimited grenades {}",
            if e.unlimited_grenades { "on" } else { "off" }
        )
    }
}

impl Rows for Page {
    fn title(&self) -> u32 {
        TITLE_ID
    }
    fn count(&self) -> usize {
        ROWS
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        match row {
            GRAVITY_ROW => vec![(
                LABEL_SLOT,
                gravity_label_id(nearest_step(self.ext().gravity)),
            )],
            GRENADES_ROW => vec![(
                LABEL_SLOT,
                grenades_label_id(self.ext().unlimited_grenades),
            )],
            TEAMS_ROW => vec![(LABEL_SLOT, teams_label_id(teams_step(self.teams())))],
            r => match Scale::of_row(r) {
                Some(sc) => vec![(LABEL_SLOT, sc.label_id(nearest_step(sc.get(&self.ext()))))],
                None => Vec::new(),
            },
        }
    }
    fn press(&self, row: i16) -> Chosen {
        match row {
            GRAVITY_ROW => Chosen::Stepped(self.step_gravity(true, true)),
            GRENADES_ROW => Chosen::Stepped(self.toggle_grenades()),
            TEAMS_ROW => Chosen::Stepped(self.step_teams(true, true)),
            r => match Scale::of_row(r) {
                Some(sc) => Chosen::Stepped(self.step_scale(sc, true, true)),
                None => Chosen::Nothing,
            },
        }
    }
    fn sideways(&self, row: i16, right: bool) -> Option<String> {
        match row {
            GRAVITY_ROW => Some(self.step_gravity(right, false)),
            GRENADES_ROW => Some(self.toggle_grenades()),
            TEAMS_ROW => Some(self.step_teams(right, false)),
            r => Scale::of_row(r).map(|sc| self.step_scale(sc, right, false)),
        }
    }
    fn description(&self, row: i16) -> Option<u32> {
        match row {
            GRAVITY_ROW => Some(GRAVITY_DESCRIPTION_ID),
            GRENADES_ROW => Some(GRENADES_DESCRIPTION_ID),
            TEAMS_ROW => Some(TEAMS_DESCRIPTION_ID),
            r => Scale::of_row(r).map(Scale::description_id),
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
        LiveText {
            writes,
            redraw: false,
        }
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
        Some(d)
    }
}

impl crate::ui_kit::spec::Describe for Page {
    fn spec(&self) -> crate::ui_kit::spec::ScreenSpec {
        use crate::ui_kit::input::{Glyph, Input, Prompt};
        use crate::ui_kit::spec::{ListPlace, Press, RowSpec, ScreenSpec, ValueStyle};
        let mut s = ScreenSpec::new(SCREEN, TITLE);
        s.ids = vec![TITLE_ID, FOOTER_ID];
        s.list = ListPlace::from(Some(crate::skulls_menu::LIST_LAYOUT));
        s.style = ValueStyle::Inline;
        s.description = Some(crate::skulls_menu::DESCRIPTION_TEXT.into());
        s.footer = true;
        s.prompts = vec![
            Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Change"),
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
        let choices: Vec<String> = GRAVITY_STEPS
            .iter()
            .map(|&g| match g {
                100 => "Normal".into(),
                g => format!("{g}%"),
            })
            .collect();
        let choices: Vec<&str> = choices.iter().map(String::as_str).collect();
        let i = nearest_step(self.ext().gravity);
        s.rows.push(
            RowSpec::value("Gravity", &choices, i)
                .press(Press::Step)
                .describe(GRAVITY_DESCRIPTION)
                .ids(&[gravity_label_id(i), GRAVITY_DESCRIPTION_ID]),
        );
        let on = self.ext().unlimited_grenades;
        s.rows.push(
            RowSpec::value("Unlimited Grenades", &["Off", "On"], usize::from(on))
                .press(Press::Step)
                .describe(GRENADES_DESCRIPTION)
                .ids(&[grenades_label_id(on), GRENADES_DESCRIPTION_ID]),
        );
        let choices: Vec<String> = TEAM_COUNTS
            .iter()
            .map(|&t| match t {
                0 => "Map Default".into(),
                t => t.to_string(),
            })
            .collect();
        let choices: Vec<&str> = choices.iter().map(String::as_str).collect();
        let i = teams_step(self.teams());
        s.rows.push(
            RowSpec::value("Team Count", &choices, i)
                .press(Press::Step)
                .describe(TEAMS_DESCRIPTION)
                .ids(&[teams_label_id(i), TEAMS_DESCRIPTION_ID]),
        );
        let steps: Vec<String> = GRAVITY_STEPS
            .iter()
            .map(|&g| match g {
                100 => "Normal".into(),
                g => format!("{g}%"),
            })
            .collect();
        let steps: Vec<&str> = steps.iter().map(String::as_str).collect();
        for sc in Scale::ALL {
            let i = nearest_step(sc.get(&self.ext()));
            s.rows.push(
                RowSpec::value(sc.name(), &steps, i)
                    .press(Press::Step)
                    .describe(sc.description())
                    .ids(&[sc.label_id(i), sc.description_id()]),
            );
        }
        s
    }
}

/// The page's class: the skulls page's layout and description, no
/// picture; left, right and A step a row.
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
        // Left at the bottom stops; A wraps round.
        for _ in 0..10 {
            p.sideways(0, false);
        }
        assert_eq!(p.ext().gravity, 10);
        assert!(p.sideways(0, false).unwrap().contains("stays"));
        assert!(matches!(p.press(0), Chosen::Stepped(_)));
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
        let mut e = Ext::default();
        e.gravity = 125;
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
        assert!(p.describe(2).unwrap().contains("Unlimited Grenades: < Off >"));
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
        assert!(matches!(p.press(TEAMS_ROW), Chosen::Stepped(_)));
        assert_eq!(p.teams(), 0, "A wraps round to the map's");
        // A stored 1 (not offered) shows as 2 and steps from there.
        let p = Page::new();
        p.read_teams(1);
        assert_eq!(p.texts(TEAMS_ROW), [(LABEL_SLOT, teams_label_id(1))]);
        p.sideways(TEAMS_ROW, true);
        assert_eq!(p.teams(), 3);
        // Both edits go out together.
        p.toggle_grenades();
        assert_eq!(p.live_text().writes.len(), 2);
        assert_eq!(teams_label(0), "Team Count: < Map Default >");
        assert_eq!(teams_label(3), "Team Count: < 4 >");
    }

    /// The damage, shield and health rows step their percents in the
    /// block, each on its own.
    #[test]
    fn the_scale_rows_step_their_percents() {
        let p = Page::new();
        p.read([0; ext::LEN]);
        assert_eq!(Scale::of_row(3), Some(Scale::Damage));
        assert_eq!(Scale::of_row(5), Some(Scale::Health));
        assert_eq!(Scale::of_row(6), None);
        for sc in Scale::ALL {
            assert_eq!(p.texts(sc.row()), [(LABEL_SLOT, sc.label_id(4))]);
            assert_eq!(p.description(sc.row()), Some(sc.description_id()));
        }
        assert!(p.sideways(Scale::Damage.row(), true).unwrap().contains("damage 100% -> 125%"));
        assert!(p.sideways(Scale::Health.row(), false).unwrap().contains("health 100% -> 75%"));
        assert!(matches!(p.press(Scale::Shield.row()), Chosen::Stepped(_)));
        let e = Ext::in_effect(&p.pending().unwrap());
        assert_eq!((e.damage, e.shield, e.health, e.gravity), (125, 125, 75, 100));
        assert_eq!(
            p.texts(Scale::Damage.row()),
            [(LABEL_SLOT, Scale::Damage.label_id(5))]
        );
        // Left at the bottom stops at 10%.
        for _ in 0..12 {
            p.sideways(Scale::Health.row(), false);
        }
        assert_eq!(p.ext().health, 10);
        assert!(p.sideways(Scale::Health.row(), false).unwrap().contains("stays 10%"));
        assert_eq!(Scale::Shield.label(4), "Shields: < Normal >");
        assert_eq!(Scale::Damage.label(7), "Damage: < 200% >");
        assert!(p.describe(ROWS).unwrap().contains("[5] \"Health: < 10% >\""));
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
        assert_eq!(gravity_label(2), "Gravity: < 50% >");
        assert_eq!(gravity_label(4), "Gravity: < Normal >");
        assert!(GRAVITY_DESCRIPTION.encode_utf16().count() <= 120);
        assert!(GRENADES_DESCRIPTION.encode_utf16().count() <= 120);
        assert!(TEAMS_DESCRIPTION.encode_utf16().count() <= 120);
        for sc in Scale::ALL {
            assert!(sc.description().encode_utf16().count() <= 120, "{sc:?}");
        }
        for g in GRAVITY_STEPS {
            assert!(ext::PERCENT.contains(&g));
        }
        assert_eq!(grenades_label(true), "Unlimited Grenades: < On >");
        for g in GRAVITY_STEPS {
            assert!(ext::GRAVITY.contains(&g));
        }
    }

    /// The Skulls row hooks the same two points; with both on, the plan
    /// builds and runs the Skulls row's hook first (a refused plan stops
    /// the game at boot).
    #[test]
    fn the_row_plans_beside_the_skulls_row() {
        let mut r = feature::Registry::new(feature::Switches::defaults());
        let store = Arc::new(settings::Store::defaults(crate::setting_defs::ALL));
        let mp = Arc::new(skulls_menu::SkullsScreen::multiplayer(store));
        r.add(&crate::skulls_mp_row::VariantRow(mp)).unwrap();
        r.add(&MoreRow).unwrap();
        let text = r.build(&feature::Base).unwrap().text();
        for point in ["VARIANT_OPTIONS_ROW_TEXT", "VARIANT_OPTIONS_ROW_CHOSEN"] {
            let line = text.lines().find(|l| l.contains(point)).unwrap();
            let skulls = line.find("skulls-mp-row").unwrap();
            let more = line.find("variant-more-options-row").unwrap();
            assert!(skulls < more, "{line}");
        }
    }

    #[test]
    fn the_row_and_the_page_are_on_the_plan() {
        let mut r = feature::Registry::new(feature::Switches::defaults());
        r.add(&class(
            Arc::new(Page::new()),
            skulls_menu::DESCRIPTION_TEXT,
            skulls_menu::LIST_LAYOUT,
        ))
        .unwrap();
        r.add(&MoreRow).unwrap();
        let text = r.build(&feature::Base).unwrap().text();
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
}
