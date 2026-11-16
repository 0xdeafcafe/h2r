//! The Skulls page between the campaign difficulty and the level, on our
//! own list class, and the flow that puts it there (docs/game/skulls.md
//! "Menu").
//!
//! **The flow.** Level select (or New Campaign) → difficulty → the Skulls
//! page → the level. The difficulty list's row chosen (`0x2c9d38`, thiscall
//! on the list, the player chain and a pointer to the row, `ret 8`) stores
//! the difficulty and starts: from New Campaign (`list + 0x2a0` clear) it
//! opens the loading screen (`0x2b1467`, screen 209); from Select Level it
//! animates the difficulty screen out (`0x22eeee`, `0x22e957`) and the
//! level select goes on. A `before` hook on it, for rows 0 to 3, asks the
//! [`PageFlow`]: when the pressing player's profile has collected a skull,
//! it keeps the press ([`Pending`], in the flow and nowhere else) and opens
//! the page instead; with none collected the original runs, as retail.
//!
//! **Continuing.** X or Start on the page ([`action`]) closes the page as B
//! does (`0x14800c`) and nothing else; when the stack frees it (our
//! destructor slot, [`freed`]) the difficulty screen is the top one again,
//! and the kept press runs on its own list ([`continue_level`]); the hook
//! lets that call through. The press is never replayed into a closing
//! screen. `H2_SKULLS_CONTINUE=now` (close, then the press at once, the
//! first build's order) or `=stay` (the press with the page still up) are
//! for trying other orders in game. B closes the page as on any screen of
//! ours: back on the difficulty list, where a press opens the page again.
//!
//! **The page** is a [`SkullsScreen`] (rows, strings and descriptions in
//! [`skulls_menu`]) on a class with the level select's layout
//! ([`skulls_menu::LIST_LAYOUT`]), the picture and the description.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::layout_glue::chosen_row;
use crate::list_class::{self, Action, ListClass, ListLayout, OnFree, PictureSpec, TextBlock};
use crate::skulls;
use crate::skulls_menu::{self, SkullsScreen};
use engine::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use engine::memory::MemFault;
use feature::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;

/// The screen id `Open` rows and the loader registry know it by.
pub const SCREEN: &str = "skulls";
/// The difficulty list handler's vtable (`0x2c9c12` stores it): the kept
/// list is checked against it before its press is run.
pub const DIFFICULTY_LIST_VTABLE: u32 = 0x0045_d688;
/// The difficulty list's mode byte: clear from New Campaign, set from
/// Select Level (`0x2c9d38` reads it).
pub const DIFFICULTY_LIST_MODE: u32 = 0x2a0;
/// Close the top screen of a window's stack (`eax` the slot, `ecx` the
/// window, as the generic back handler `0x2300ea` calls it with the
/// screen's `+0x78` and `+0x74`). Retail's close starts the screen's out
/// transition; the screen is freed when it ends.
pub const SCREEN_CLOSE: u32 = 0x0014_800c;
const SCREEN_WINDOW: u32 = 0x74;
const SCREEN_SLOT: u32 = 0x78;
/// The difficulty rows: Easy, Normal, Heroic, Legendary.
pub const DIFFICULTIES: std::ops::RangeInclusive<i16> = 0..=3;
const DIFFICULTY_NAMES: [&str; 4] = ["Easy", "Normal", "Heroic", "Legendary"];

fn difficulty_name(row: i16) -> &'static str {
    usize::try_from(row)
        .ok()
        .and_then(|r| DIFFICULTY_NAMES.get(r).copied())
        .unwrap_or("?")
}

/// The page's class: the level select's layout, the picture and the
/// description; X and Start continue, and the destructor tells the flow.
pub fn class(
    screen: Arc<SkullsScreen>,
    block: TextBlock,
    layout: ListLayout,
    picture: PictureSpec,
    flow: Arc<PageFlow>,
) -> ListClass {
    let on_action = Arc::clone(&flow);
    let act: Action = Arc::new(move |cx, page| {
        let flow = Arc::clone(&on_action);
        Box::pin(async move { action(&cx, &flow, page, Continue::from_env()).await })
    });
    let on_free: OnFree = Arc::new(move |cx, page| {
        let flow = Arc::clone(&flow);
        Box::pin(async move { freed(&cx, &flow, page).await })
    });
    ListClass::with_rows(skulls_menu::FEATURE, SCREEN, screen)
        .description(block)
        .picture(picture)
        .layout(layout)
        .action(act)
        .on_free(on_free)
}

/// A difficulty press kept while the page is up: what to run to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    /// The difficulty list handler (the press's `this`).
    pub list: u32,
    pub row: i16,
    pub player: u32,
    pub profile: String,
    /// The list's mode byte: false from New Campaign, true from Select
    /// Level.
    pub from_select: bool,
}

impl Pending {
    /// The line logged when the press is kept.
    pub fn kept_line(&self) -> String {
        format!(
            "skulls page: kept difficulty {} ({}) for level {}, player {} ({}), list {:#x}",
            self.row,
            difficulty_name(self.row),
            if self.from_select {
                "chosen on Select Level"
            } else {
                "New Campaign's first"
            },
            self.player,
            self.profile,
            self.list
        )
    }
}

/// What a press on the difficulty list knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Press {
    pub list: u32,
    pub row: Option<i16>,
    pub player: Option<u32>,
    pub profile: Option<String>,
    pub from_select: bool,
    /// How many skulls `profile` has collected.
    pub collected: usize,
}

/// What the hook does with a difficulty press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Run the original (start as retail); the line to log, if any.
    Retail(Option<String>),
    /// Open the page for this press.
    Page(Pending),
}

/// Where the page is, for the kept press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Up (or about to be), waiting for X, Start or B.
    Shown,
    /// X or Start closed this page; the press runs when it is freed.
    Closing(u32),
    /// The kept press is running: the difficulty press the hook sees now is
    /// ours, and runs as retail.
    Continuing,
}

/// What X or Start finds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Close the page; the press runs when it is freed.
    Close(Pending),
    /// Already closing (or running): nothing more to do.
    Busy,
    /// Nothing kept.
    Nothing,
}

/// What a freed page means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Freed {
    /// The page X or Start closed: run this press now.
    Continue(Pending),
    /// Closed some other way (B): the press stays kept until the next
    /// difficulty press replaces it.
    Back,
}

/// The page's state: the one kept difficulty press, where the page is,
/// and the scratch page the press's arguments are written to. The press
/// is stored when the hook takes it and only dropped once it has run, so
/// X or Start always finds it while the page is up.
pub struct PageFlow {
    kept: Mutex<(Option<Pending>, Phase)>,
    on: AtomicBool,
    scratch: OnceLock<u32>,
}

impl Default for PageFlow {
    fn default() -> Self {
        PageFlow {
            kept: Mutex::new((None, Phase::Shown)),
            on: AtomicBool::new(true),
            scratch: OnceLock::new(),
        }
    }
}

impl PageFlow {
    fn lock(&self) -> std::sync::MutexGuard<'_, (Option<Pending>, Phase)> {
        self.kept.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Whether the page is shown at all (its class is on the plan).
    pub fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::SeqCst);
    }
    /// A press on the difficulty list.
    pub fn difficulty(&self, p: Press) -> Decision {
        let mut kept = self.lock();
        if kept.1 == Phase::Continuing {
            return Decision::Retail(None);
        }
        let Some(row) = p.row.filter(|r| DIFFICULTIES.contains(r)) else {
            return Decision::Retail(None);
        };
        if !self.on.load(Ordering::SeqCst) {
            return Decision::Retail(None);
        }
        let (Some(player), Some(profile)) = (p.player, p.profile) else {
            return Decision::Retail(Some(
                "skulls page: the pressing player is unreadable; starting without the page".into(),
            ));
        };
        if p.collected == 0 {
            return Decision::Retail(Some(format!(
                "skulls page: {profile} has no skulls collected; starting as retail"
            )));
        }
        let pending = Pending {
            list: p.list,
            row,
            player,
            profile,
            from_select: p.from_select,
        };
        *kept = (Some(pending.clone()), Phase::Shown);
        Decision::Page(pending)
    }
    /// X or Start on `page`.
    pub fn start(&self, page: u32) -> Step {
        let mut kept = self.lock();
        match (&kept.0, kept.1) {
            (None, _) => Step::Nothing,
            (Some(_), Phase::Closing(_) | Phase::Continuing) => Step::Busy,
            (Some(p), Phase::Shown) => {
                let p = p.clone();
                kept.1 = Phase::Closing(page);
                Step::Close(p)
            }
        }
    }
    /// X or Start with the press run at once (`H2_SKULLS_CONTINUE=now` or
    /// `stay`): the press, now running.
    pub fn begin(&self) -> Option<Pending> {
        let mut kept = self.lock();
        if kept.1 == Phase::Continuing {
            return None;
        }
        let p = kept.0.clone()?;
        kept.1 = Phase::Continuing;
        Some(p)
    }
    /// Page `page` was freed.
    pub fn freed(&self, page: u32) -> Freed {
        let mut kept = self.lock();
        match (&kept.0, kept.1) {
            (Some(p), Phase::Closing(closing)) if closing == page => {
                let p = p.clone();
                kept.1 = Phase::Continuing;
                Freed::Continue(p)
            }
            _ => Freed::Back,
        }
    }
    /// The kept press has run (or could not be): nothing is kept.
    pub fn done(&self) {
        *self.lock() = (None, Phase::Shown);
    }
    /// The press kept, if any.
    pub fn pending(&self) -> Option<Pending> {
        self.lock().0.clone()
    }
}

/// The press's arguments at `at`: a player chain (`[at]` the record at
/// `at + 8`, whose `+4` is the player index, as `[[chain]]+4` is read) and
/// the row at `at + 0x10`. Returns (chain, row pointer).
pub fn write_press(
    m: &mut dyn engine::memory::GuestMemory,
    at: u32,
    p: &Pending,
) -> Result<(u32, u32), MemFault> {
    m.set_u32(at, at + 8)?;
    m.set_u32(at + 8, 0)?;
    m.set_u32(at + 0xc, p.player)?;
    m.set_u32(at + 0x10, u32::from(p.row as u16))?;
    Ok((at, at + 0x10))
}

/// When X or Start runs the kept press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Continue {
    /// Close the page; run the press once the stack has freed it.
    Freed,
    /// Close the page, then run the press at once (the first build's order).
    Now,
    /// Run the press with the page still up.
    Stay,
}

impl Continue {
    /// `H2_SKULLS_CONTINUE`: `now`, `stay`, else [`Continue::Freed`].
    pub fn parse(v: Option<&str>) -> Continue {
        match v {
            Some("now") => Continue::Now,
            Some("stay") => Continue::Stay,
            _ => Continue::Freed,
        }
    }
    fn from_env() -> Continue {
        Continue::parse(std::env::var("H2_SKULLS_CONTINUE").ok().as_deref())
    }
}

/// Ask the stack to close `page`, as B does.
async fn close(cx: &Cx, page: u32) -> Result<(), EntryError> {
    let (window, slot) = cx.read(|m| -> Result<_, MemFault> {
        Ok((m.u32(page + SCREEN_WINDOW)?, m.u32(page + SCREEN_SLOT)?))
    })?;
    eprintln!("skulls page: close requested for the page {page:#x} (window {window}, slot {slot})");
    cx.call(GuestCall::new(SCREEN_CLOSE).eax(slot).ecx(window))
        .await?;
    Ok(())
}

/// X or Start on the page `page`.
pub async fn action(cx: &Cx, flow: &PageFlow, page: u32, mode: Continue) -> Result<(), EntryError> {
    match mode {
        Continue::Freed => match flow.start(page) {
            Step::Nothing => {
                eprintln!("skulls page: X/Start with no difficulty kept; nothing to start");
                Ok(())
            }
            Step::Busy => {
                eprintln!(
                    "skulls page: X/Start again; already closing, the level starts when the page is freed"
                );
                Ok(())
            }
            Step::Close(p) => {
                eprintln!(
                    "skulls page: X/Start: starting difficulty {} ({}) once the page is freed",
                    p.row,
                    difficulty_name(p.row)
                );
                close(cx, page).await
            }
        },
        Continue::Now | Continue::Stay => {
            let Some(p) = flow.begin() else {
                eprintln!(
                    "skulls page: X/Start with no difficulty kept (or already starting); nothing to start"
                );
                return Ok(());
            };
            if mode == Continue::Now
                && let Err(e) = close(cx, page).await
            {
                flow.done();
                return Err(e);
            }
            continue_level(cx, flow, &p).await
        }
    }
}

/// The page `page` was freed (our destructor slot, after the Controls one).
pub async fn freed(cx: &Cx, flow: &PageFlow, page: u32) -> Result<(), EntryError> {
    match flow.freed(page) {
        Freed::Continue(p) => {
            eprintln!("skulls page: the page {page:#x} is freed; back on the difficulty list");
            continue_level(cx, flow, &p).await
        }
        Freed::Back => {
            eprintln!("skulls page: the page {page:#x} is freed (B); nothing started");
            Ok(())
        }
    }
}

/// Run the kept difficulty press on its own list, as the game would have:
/// it stores the difficulty and starts the level. The hook lets it through
/// ([`Phase::Continuing`]); nothing is kept after.
pub async fn continue_level(cx: &Cx, flow: &PageFlow, p: &Pending) -> Result<(), EntryError> {
    let Some(&scratch) = flow.scratch.get() else {
        eprintln!("skulls page: no scratch page; not started");
        flow.done();
        return Ok(());
    };
    let vtable = cx.read(|m| m.u32(p.list)).unwrap_or(0);
    if vtable != DIFFICULTY_LIST_VTABLE {
        eprintln!(
            "skulls page: the difficulty list at {:#x} is gone (vtable {vtable:#x}); not started",
            p.list
        );
        flow.done();
        return Ok(());
    }
    eprintln!(
        "skulls page: starting: the difficulty press ({} for {}, list {:#x}) with {}",
        difficulty_name(p.row),
        p.profile,
        p.list,
        skulls_on_line(&p.profile)
    );
    let r = match cx.write(|m| write_press(m, scratch, p)) {
        Ok((chain, row)) => {
            cx.call(
                GuestCall::new(engine::points::DIFFICULTY_ROW_CHOSEN.base)
                    .ecx(p.list)
                    .args([chain, row]),
            )
            .await
        }
        Err(e) => Err(e.into()),
    };
    flow.done();
    let r = r?;
    eprintln!(
        "skulls page: the difficulty press returned ({:#x}); the level should be loading",
        r.eax
    );
    Ok(())
}

/// The store, for the start's log line.
static SKULLS_STORE: OnceLock<Arc<settings::Store>> = OnceLock::new();

/// The skulls the start will turn on for `profile`, for the log (the
/// campaign start's own line says what it applied).
fn skulls_on_line(profile: &str) -> String {
    SKULLS_STORE.get().map_or_else(
        || "its skulls".into(),
        |store| {
            let on: Vec<&str> = (0..skulls::TOTAL)
                .filter(|i| skulls::state(store, profile, *i) == Some(skulls::State::On))
                .map(|i| skulls::SKULLS[i].label)
                .collect();
            if on.is_empty() {
                "no skulls on".to_string()
            } else {
                on.join(", ")
            }
        },
    )
}

/// The page between the difficulty and the level: the hook on the
/// difficulty list's press.
pub struct DifficultyPage {
    pub screen: Arc<SkullsScreen>,
    pub flow: Arc<PageFlow>,
    pub store: Arc<settings::Store>,
}

/// Whether the difficulty list `list` was opened from Select Level (its
/// mode byte set), not New Campaign.
pub fn from_select_level<M: crate::layout_glue::Gm>(m: &M, list: u32) -> bool {
    m.u8(list.wrapping_add(DIFFICULTY_LIST_MODE))
        .is_some_and(|b| b != 0)
}

fn row_chosen(c: &mut Call, page: &DifficultyPage, seen: &AtomicBool) -> Result<Flow, GuestFault> {
    let list = c.cpu.ecx;
    let (row, player, profile, from_select) = {
        let mem = c.ctx.runtime().memory();
        let player = skulls_menu::player_index(&*mem, c.cpu.esp);
        (
            chosen_row(&*mem, c.cpu.esp),
            player,
            player.and_then(|p| skulls_menu::profile_of(&*mem, p)),
            from_select_level(&*mem, list),
        )
    };
    // The store is read with guest memory released.
    let collected = profile
        .as_deref()
        .map_or(0, |p| skulls::collected(&page.store, p).len());
    let press = Press {
        list,
        row,
        player,
        profile,
        from_select,
        collected,
    };
    let pending = match page.flow.difficulty(press) {
        Decision::Retail(line) => {
            if let Some(line) = line {
                eprintln!("{line}");
            }
            return Ok(Flow::Continue);
        }
        Decision::Page(p) => p,
    };
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("skulls page: difficulty chosen; the Skulls page comes first");
    }
    let Some(loader) = list_class::loader_of(SCREEN) else {
        eprintln!("skulls page: its list class is off; starting as retail");
        page.flow.done();
        return Ok(Flow::Continue);
    };
    eprintln!("{}", pending.kept_line());
    page.screen.open(pending.profile.clone());
    let mask = 1u32 << pending.player;
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            cx.call(GuestCall::new(engine::points::OPEN_SCREEN.base).args([mask, loader]))
                .await?;
            Ok(Ret::new(0, 8))
        })
    })
}

impl feature::Feature for DifficultyPage {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "skulls-page",
            "The Skulls page between the difficulty and the level",
        )
        .doc("docs/game/skulls.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let scratch = b.guest_alloc(0x1000, Permissions::RW)?;
        self.flow.scratch.set(scratch).ok();
        SKULLS_STORE.set(Arc::clone(&self.store)).ok();
        let seen = Arc::new(AtomicBool::new(false));
        let page = DifficultyPage {
            screen: Arc::clone(&self.screen),
            flow: Arc::clone(&self.flow),
            store: Arc::clone(&self.store),
        };
        b.raw().before(
            &engine::points::DIFFICULTY_ROW_CHOSEN,
            "the Skulls page before the level",
            move |c| row_chosen(c, &page, &seen),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list_class::Rows;
    use engine::entry::{GuestCall as Gc, Regs, testing};
    use engine::memory::GuestMemory;
    use engine::testing::FakeGuest;
    use settings::Store;

    fn screen() -> Arc<SkullsScreen> {
        Arc::new(SkullsScreen::new(Arc::new(Store::defaults(
            crate::setting_defs::ALL,
        ))))
    }

    fn press(row: i16, collected: usize) -> Press {
        Press {
            list: 0x5000,
            row: Some(row),
            player: Some(1),
            profile: Some("Chief".into()),
            from_select: false,
            collected,
        }
    }

    fn kept(row: i16) -> Pending {
        Pending {
            list: 0x5000,
            row,
            player: 1,
            profile: "Chief".into(),
            from_select: false,
        }
    }

    #[test]
    fn a_profile_with_no_skulls_goes_straight_to_the_level() {
        let f = PageFlow::default();
        match f.difficulty(press(3, 0)) {
            Decision::Retail(Some(line)) => {
                assert!(line.contains("no skulls collected"), "{line}")
            }
            d => panic!("{d:?}"),
        }
        assert_eq!(f.pending(), None);
        assert_eq!(f.start(0x100), Step::Nothing, "no page, nothing to start");
    }

    /// The whole flow: kept at the press; X/Start closes the page; the
    /// page freed runs the press, which the hook lets through; then
    /// nothing is kept.
    #[test]
    fn kept_at_the_press_closed_on_start_run_when_freed() {
        let f = PageFlow::default();
        assert_eq!(f.difficulty(press(2, 1)), Decision::Page(kept(2)));
        assert_eq!(f.pending(), Some(kept(2)));
        assert_eq!(f.start(0x100), Step::Close(kept(2)));
        // Pressed again while the page animates out: still kept, no second
        // close.
        assert_eq!(f.start(0x100), Step::Busy);
        assert_eq!(f.pending(), Some(kept(2)), "still kept while closing");
        assert_eq!(f.freed(0x100), Freed::Continue(kept(2)));
        // The press we run comes back through the hook: retail.
        assert_eq!(f.difficulty(press(2, 1)), Decision::Retail(None));
        f.done();
        assert_eq!(f.pending(), None);
        assert_eq!(f.start(0x100), Step::Nothing, "started once");
    }

    /// The first build's failure: X/Start found nothing kept. Nothing but
    /// a new difficulty press or the press having run drops it now; Start
    /// pressed any number of times finds it.
    #[test]
    fn start_always_finds_the_kept_press_while_the_page_is_up() {
        let f = PageFlow::default();
        f.difficulty(press(3, 1));
        // Presses on rows that are not difficulties, and B's free of
        // another page, leave it.
        assert_eq!(f.difficulty(press(7, 1)), Decision::Retail(None));
        assert_eq!(f.freed(0x200), Freed::Back);
        assert_eq!(f.pending(), Some(kept(3)));
        assert_eq!(f.start(0x100), Step::Close(kept(3)));
    }

    #[test]
    fn a_page_freed_by_b_starts_nothing() {
        let f = PageFlow::default();
        f.difficulty(press(1, 1));
        assert_eq!(f.freed(0x100), Freed::Back);
        assert_eq!(f.difficulty(press(0, 1)), Decision::Page(kept(0)));
        assert_eq!(f.start(0x300), Step::Close(kept(0)));
        assert_eq!(f.freed(0x100), Freed::Back, "not the page closed");
        assert_eq!(f.freed(0x300), Freed::Continue(kept(0)));
    }

    /// B closes the page (the generic back handler); on the difficulty list
    /// again, another difficulty opens the page with that one.
    #[test]
    fn back_then_another_difficulty_keeps_the_new_one() {
        let f = PageFlow::default();
        assert!(matches!(f.difficulty(press(3, 2)), Decision::Page(_)));
        assert!(matches!(f.difficulty(press(0, 2)), Decision::Page(p) if p.row == 0));
        assert!(matches!(f.start(1), Step::Close(p) if p.row == 0));
    }

    #[test]
    fn only_the_four_difficulties_and_a_known_player_open_the_page() {
        let f = PageFlow::default();
        assert_eq!(f.difficulty(press(4, 3)), Decision::Retail(None));
        assert_eq!(f.difficulty(press(-1, 3)), Decision::Retail(None));
        let mut p = press(1, 3);
        p.row = None;
        assert_eq!(f.difficulty(p), Decision::Retail(None));
        let mut p = press(1, 3);
        p.player = None;
        assert!(matches!(f.difficulty(p), Decision::Retail(Some(_))));
        f.set_on(false);
        assert_eq!(
            f.difficulty(press(1, 3)),
            Decision::Retail(None),
            "class off"
        );
        assert_eq!(f.pending(), None);
    }

    #[test]
    fn the_kept_line_names_difficulty_level_and_player() {
        assert_eq!(
            kept(3).kept_line(),
            "skulls page: kept difficulty 3 (Legendary) for level New Campaign's first, \
             player 1 (Chief), list 0x5000"
        );
        let p = Pending {
            from_select: true,
            ..kept(0)
        };
        assert!(
            p.kept_line()
                .contains("difficulty 0 (Easy) for level chosen on Select Level")
        );
    }

    #[test]
    fn the_list_mode_byte_says_where_it_came_from() {
        /// One byte of guest memory at the list's mode.
        struct Mode(u8);
        impl crate::layout_glue::Gm for Mode {
            fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
                if addr != 0x5000 + DIFFICULTY_LIST_MODE || out.len() != 1 {
                    return false;
                }
                out[0] = self.0;
                true
            }
            fn wr(&mut self, _: u32, _: &[u8]) -> bool {
                false
            }
        }
        assert!(!from_select_level(&Mode(0), 0x5000));
        assert!(from_select_level(&Mode(1), 0x5000));
        assert!(
            !from_select_level(&Mode(1), 0x6000),
            "unreadable: New Campaign"
        );
    }

    #[test]
    fn the_press_arguments_read_as_the_game_reads_them() {
        let mut g = FakeGuest::new();
        g.map(0x9000, 0x100);
        let p = Pending {
            player: 2,
            ..kept(3)
        };
        let (chain, row) = write_press(&mut g, 0x9000, &p).unwrap();
        // `[[chain]]+4` is the player; `[row] & 0xffff` the row.
        let record = g.u32(chain).unwrap();
        assert_eq!(g.u32(record + 4), Ok(2));
        assert_eq!(g.u32(row).map(|r| r & 0xffff), Ok(3));
    }

    #[test]
    fn continue_comes_from_the_env() {
        assert_eq!(Continue::parse(None), Continue::Freed);
        assert_eq!(Continue::parse(Some("freed")), Continue::Freed);
        assert_eq!(Continue::parse(Some("now")), Continue::Now);
        assert_eq!(Continue::parse(Some("stay")), Continue::Stay);
    }

    const PAGE: u32 = 0x0010_0000;
    const LIST: u32 = 0x0020_0000;
    const SCRATCH: u32 = 0x0030_0000;

    fn guest(list_vtable: u32) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(PAGE, 0x100).map(LIST, 0x400).map(SCRATCH, 0x100);
        g.put_u32(PAGE + SCREEN_WINDOW, 5)
            .put_u32(PAGE + SCREEN_SLOT, 4)
            .put_u32(LIST, list_vtable);
        g
    }

    fn flow() -> Arc<PageFlow> {
        let f = Arc::new(PageFlow::default());
        f.scratch.set(SCRATCH).unwrap();
        assert!(matches!(
            f.difficulty(Press {
                list: LIST,
                ..press(1, 1)
            }),
            Decision::Page(_)
        ));
        f
    }

    /// Runs `step` on the flow as an entry, against `g`.
    fn run(
        f: &Arc<PageFlow>,
        g: FakeGuest,
        step: fn(Cx, Arc<PageFlow>) -> EntryFuture,
    ) -> testing::Run {
        let flow = Arc::clone(f);
        let run_f = move |cx: Cx| -> EntryFuture { step(cx, Arc::clone(&flow)) };
        testing::run(&run_f, g, Regs::default(), |_, _| Regs::default())
    }

    fn the_press() -> Gc {
        Gc::new(engine::points::DIFFICULTY_ROW_CHOSEN.base)
            .ecx(LIST)
            .args([SCRATCH, SCRATCH + 0x10])
    }

    /// X/Start: the page's close as B makes it, and nothing else; the
    /// press waits for the free.
    #[test]
    fn start_only_closes_the_page() {
        let f = flow();
        let run = run(&f, guest(DIFFICULTY_LIST_VTABLE), |cx, f| {
            Box::pin(async move {
                action(&cx, &f, PAGE, Continue::Freed).await?;
                Ok(Ret::new(0, 0))
            })
        });
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
        assert_eq!(run.calls, [Gc::new(SCREEN_CLOSE).eax(4).ecx(5)]);
        assert!(f.pending().is_some(), "kept until it runs");
    }

    /// The page freed after X/Start: the press runs on the kept list.
    #[test]
    fn the_freed_page_runs_the_press() {
        let f = flow();
        assert!(matches!(f.start(PAGE), Step::Close(_)));
        let run = run(&f, guest(DIFFICULTY_LIST_VTABLE), |cx, f| {
            Box::pin(async move {
                freed(&cx, &f, PAGE).await?;
                Ok(Ret::new(0, 0))
            })
        });
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
        assert_eq!(run.calls, [the_press()]);
        assert_eq!(f.pending(), None);
        assert_eq!(f.start(PAGE), Step::Nothing);
    }

    /// B's free calls nothing.
    #[test]
    fn a_page_freed_without_start_calls_nothing() {
        let f = flow();
        let run = run(&f, guest(DIFFICULTY_LIST_VTABLE), |cx, f| {
            Box::pin(async move {
                freed(&cx, &f, PAGE).await?;
                Ok(Ret::new(0, 0))
            })
        });
        assert_eq!(run.calls, []);
        assert!(f.pending().is_some());
    }

    /// `H2_SKULLS_CONTINUE=now`: close, then the press at once.
    #[test]
    fn now_closes_then_runs_the_press() {
        let f = flow();
        let run = run(&f, guest(DIFFICULTY_LIST_VTABLE), |cx, f| {
            Box::pin(async move {
                action(&cx, &f, PAGE, Continue::Now).await?;
                Ok(Ret::new(0, 0))
            })
        });
        assert_eq!(
            run.calls,
            [Gc::new(SCREEN_CLOSE).eax(4).ecx(5), the_press()]
        );
        assert_eq!(f.pending(), None);
    }

    /// `H2_SKULLS_CONTINUE=stay`: the press with the page up.
    #[test]
    fn stay_runs_the_press_without_closing() {
        let f = flow();
        let run = run(&f, guest(DIFFICULTY_LIST_VTABLE), |cx, f| {
            Box::pin(async move {
                action(&cx, &f, PAGE, Continue::Stay).await?;
                Ok(Ret::new(0, 0))
            })
        });
        assert_eq!(run.calls, [the_press()]);
    }

    /// A kept list that is no longer a difficulty list is not called.
    #[test]
    fn the_press_is_refused_on_a_list_that_is_gone() {
        let f = flow();
        assert!(matches!(f.start(PAGE), Step::Close(_)));
        let run = run(&f, guest(0), |cx, f| {
            Box::pin(async move {
                freed(&cx, &f, PAGE).await?;
                Ok(Ret::new(0, 0))
            })
        });
        assert_eq!(run.calls, []);
        assert_eq!(f.pending(), None, "nothing kept after the refusal");
    }

    #[test]
    fn the_page_is_skulls_only_and_a_toggles() {
        let r = screen();
        assert_eq!(r.count(), skulls::TOTAL, "no Start row");
        for row in 0..r.count() as i16 {
            assert_ne!(r.press(row), crate::ui::Chosen::Act, "row {row}");
        }
        assert!(matches!(r.press(0), crate::ui::Chosen::Stepped(m) if m.contains("no profile")));
    }

    #[test]
    fn the_hook_and_the_class_are_on_the_plan() {
        let mut r = feature::Registry::new(feature::Switches::defaults());
        let s = screen();
        let flow = Arc::new(PageFlow::default());
        let c = class(
            Arc::clone(&s),
            skulls_menu::DESCRIPTION_TEXT,
            skulls_menu::LIST_LAYOUT,
            skulls_menu::PICTURE,
            Arc::clone(&flow),
        );
        r.add(&c).unwrap();
        r.add(&DifficultyPage {
            screen: s,
            flow,
            store: Arc::new(Store::defaults(crate::setting_defs::ALL)),
        })
        .unwrap();
        let text = r.build(&feature::Base).unwrap().text();
        assert!(
            text.contains(
                "0x002c9d38 DIFFICULTY_ROW_CHOSEN: before [skulls-page:the Skulls page before the level] -> original\n"
            ),
            "{text}"
        );
        assert!(
            !text.contains("DIFFICULTY_ROW_TEXT"),
            "no row on the difficulty list"
        );
        assert!(text.contains("entry skulls-screen:loader"));
        assert!(text.contains("entry skulls-screen:event"));
        assert!(text.contains("entry skulls-screen:destructor"));
    }
}
