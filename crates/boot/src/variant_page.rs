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
//! **Which options.** Only those the game applies: gravity (slice 3). The
//! others join the page with their slices.

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

const _: () = assert!(GRAVITY_LABELS + GRAVITY_STEPS.len() as u32 <= IDS.len);

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
}

impl Default for Page {
    fn default() -> Self {
        Self {
            block: Mutex::new([0; ext::LEN]),
            dirty: AtomicBool::new(false),
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
        let mut b = self.block.lock().unwrap_or_else(|e| e.into_inner());
        let mut e = Ext::in_effect(&b);
        let i = nearest_step(e.gravity);
        let n = GRAVITY_STEPS.len();
        let j = match (up, wrap) {
            (true, true) => (i + 1) % n,
            (false, true) => (i + n - 1) % n,
            (true, false) => (i + 1).min(n - 1),
            (false, false) => i.saturating_sub(1),
        };
        let was = e.gravity;
        e.gravity = GRAVITY_STEPS[j];
        if e.gravity == was {
            return format!("variant options: gravity stays {was}%");
        }
        *b = e.encode();
        self.dirty.store(true, Ordering::SeqCst);
        format!("variant options: gravity {was}% -> {}%", e.gravity)
    }
}

impl Rows for Page {
    fn title(&self) -> u32 {
        TITLE_ID
    }
    fn count(&self) -> usize {
        1
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        match row {
            0 => vec![(
                LABEL_SLOT,
                gravity_label_id(nearest_step(self.ext().gravity)),
            )],
            _ => Vec::new(),
        }
    }
    fn press(&self, row: i16) -> Chosen {
        match row {
            0 => Chosen::Stepped(self.step_gravity(true, true)),
            _ => Chosen::Nothing,
        }
    }
    fn sideways(&self, row: i16, right: bool) -> Option<String> {
        (row == 0).then(|| self.step_gravity(right, false))
    }
    fn description(&self, row: i16) -> Option<u32> {
        (row == 0).then_some(GRAVITY_DESCRIPTION_ID)
    }
    /// An edit goes to the record before the row is drawn again.
    fn live_text(&self) -> LiveText {
        LiveText {
            writes: self
                .pending()
                .map(|b| vec![(EDITED_BLOCK, b.to_vec())])
                .unwrap_or_default(),
            redraw: false,
        }
    }
    /// Reads the record's block (when the page opens and at each row text).
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        let mut b = [0u8; ext::LEN];
        m.read(EDITED_BLOCK, &mut b)?;
        self.read(b);
        Ok(())
    }
    fn describe(&self, _count: usize) -> Option<String> {
        let i = nearest_step(self.ext().gravity);
        Some(format!("\"{TITLE}\"; rows: [0] \"{}\"", gravity_label(i)))
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
        assert_eq!(p.press(1), Chosen::Nothing);
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
