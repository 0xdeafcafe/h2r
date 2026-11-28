//! The Skulls page between the campaign difficulty and the level
//! (docs/game/skulls.md "Menu"), on our own list class,
//! and the flow that puts it there.
//!
//! **The flow.** Level select (or New Campaign) → difficulty → the page →
//! the level. The difficulty list's row chosen (`0x2c9d38`, thiscall on the
//! list, the player chain and a pointer to the row, `ret 8`) stores the
//! difficulty and goes on: in the list's mode 0 (the level select's and New
//! Campaign's difficulty, `list + 0x2a0` clear) it pushes the loading
//! screen (`0x2b1467`, screen 209) onto window 5 slot 4, over whatever is
//! on top; in mode 1 (a lobby's Game Setup changing its difficulty,
//! `0x2bb945`) it stores the lobby's difficulty and plays the difficulty
//! screen's out transition, back to the lobby. A `before` hook on it, for
//! rows 0 to 3, asks the [`PageFlow`]: when the pressing player's profile
//! has collected a skull, or the level is a Halo 1 level (its options,
//! `crate::h1_options::CAMPAIGN`), it keeps the press ([`Pending`]) and opens
//! the page instead; otherwise the original runs, as retail.
//!
//! **The page.** The first row is Start Level; then a Halo 1 level's
//! options; then the skulls. A on Start Level goes on; A, left or right on
//! any other row changes it. B goes back to the difficulty list. X and
//! Start do nothing (X going on read as a mystery).
//!
//! **Going on** ([`action`]) runs the kept press the way the game would
//! have run it from the difficulty list, chosen by the list's mode
//! ([`Continue::for_press`]):
//!
//! - mode 0, [`Continue::Stay`]: the press runs at once, from the page's
//!   own row press, with the page still up. Its loading screen goes on top
//!   of the page as it would have gone on top of the difficulty screen, so
//!   the level loads straight away. The first builds closed the page first
//!   and ran the press once the stack had freed it ([`Continue::Freed`]):
//!   the page animated out, the difficulty screen came back, then the
//!   level loaded; and a continue pressed while the page was still
//!   animating in went back and never loaded (the press then ran from
//!   inside the stack's own free of the page, while it was still settling
//!   which screen was on top). No close, no free: neither can happen.
//!   Before that, closing and pressing at once ([`Continue`] had a `Now`)
//!   put the loading screen on top of a page still closing, and the close
//!   then took the loading screen down: Start seemed stuck.
//! - mode 1, [`Continue::Freed`]: the press only stores the lobby's
//!   difficulty and animates the difficulty screen out, which with the
//!   page on top would leave the page up over the lobby. The page closes
//!   (`0x14800c`, as B does) and the press runs once it is freed
//!   ([`freed`]), as before; nothing loads in this mode.
//!
//! **The press that opened the page** is not taken as a continue: the
//! game raises a button event only on the frame the button goes down
//! (`list_class::LEFT_EVENT` has the trace), so a held A raises no second
//! one, and a continue within [`GUARD`] of the page opening (sooner than
//! two presses can be made) is dropped as that press seen twice.
//!
//! **The page** is a [`SkullsScreen`] (rows, strings and descriptions in
//! [`skulls_menu`]) on a class with the level select's layout
//! ([`skulls_menu::LIST_LAYOUT`]), the picture and the description.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::h1_options::{self, OptionRow};
use crate::layout_glue::{ReadGm, chosen_row};
use crate::list_class::{self, Action, ListClass, ListLayout, OnFree, PictureSpec, TextBlock};
use crate::skulls;
use crate::skulls_menu::{self, SkullsScreen};
use crate::ui_kit::input::OpenerGuard;
use crate::ui_kit::lifecycle;
use crate::ui_watch;
use guest_layout::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use guest_layout::memory::MemFault;
use hooks::{BuildError, Builder, Call, Flow, Manifest};
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
pub const SCREEN_CLOSE: u32 = lifecycle::SCREEN_CLOSE;
#[cfg(test)]
use lifecycle::{SCREEN_SLOT, SCREEN_WINDOW};
/// Where the difficulty press (mode 0) opens the loading screen: window 5,
/// slot 4 (`0x149f49(.., 5, 4, 0x2b1467)` at `0x2c9dae`..`0x2c9dc3`).
pub const PRESS_WINDOW: u32 = 5;
pub const PRESS_SLOT: u32 = 4;
/// The loading screen's id (`0x2b1467`'s, `screen + 0x70`).
pub const LOADING_SCREEN: u32 = 0xd1;
/// Where in the scratch page the stand-in list goes ([`write_stand_in`]):
/// past the press's arguments, its mode byte inside the page.
pub const STAND_IN: u32 = 0x100;
/// The difficulty rows: Easy, Normal, Heroic, Legendary.
pub const DIFFICULTIES: std::ops::RangeInclusive<i16> = 0..=3;
const DIFFICULTY_NAMES: [&str; 4] = ["Easy", "Normal", "Heroic", "Legendary"];

fn difficulty_name(row: i16) -> &'static str {
    usize::try_from(row)
        .ok()
        .and_then(|r| DIFFICULTY_NAMES.get(r).copied())
        .unwrap_or("?")
}

/// A continue this soon after the page opened is the press that opened it
/// seen again, not a new one: under two frames at 30 fps, and well under
/// the gap between two presses of a thumb.
pub const GUARD: Duration = crate::ui_kit::input::OPENER_GUARD;

/// The page's class: the level select's layout, the picture and the
/// description. A starts the level, or opens the value picker on a Halo 1
/// option or a collected skull; it does nothing on a locked one. No left
/// and right steps. The destructor tells the start flow when the page
/// closes.
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
        Box::pin(async move {
            let mode = Continue::for_press(flow.pending().is_some_and(|p| p.from_select));
            action(&cx, &flow, page, mode).await
        })
    });
    let on_free: OnFree = Arc::new(move |cx, page| {
        let flow = Arc::clone(&flow);
        Box::pin(async move { freed(&cx, &flow, page).await })
    });
    skulls_menu::level_select_list(
        ListClass::with_rows(skulls_menu::FEATURE, SCREEN, screen),
        layout,
    )
    .description(block)
    .picture(picture)
    .action(act)
    .action_buttons(&[])
    .on_free(on_free)
    .footer(skulls_menu::FOOTER_ID)
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
    /// The level's options for the page: Halo 1's for a Halo 1 level
    /// ([`page_options`]), none for Halo 2's.
    pub options: &'static [&'static OptionRow],
}

/// The page's options for the level whose campaign key is `key` (the
/// selected map's campaign, `campaign_menu::campaign_key_of`): Halo 1's
/// for a Halo 1 level, none otherwise.
pub fn page_options(key: Option<&str>) -> &'static [&'static OptionRow] {
    match key {
        Some(k) if k.eq_ignore_ascii_case(crate::campaigns::HALO1_KEY) => h1_options::CAMPAIGN,
        _ => &[],
    }
}

/// What the hook does with a difficulty press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Run the original (start as retail); the line to log, if any.
    Retail(Option<String>),
    /// Open the page for this press, with these options above the skulls.
    Page(Pending, &'static [&'static OptionRow]),
}

/// Where the page is, for the kept press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Up (or about to be), waiting for Start Level or B.
    Shown,
    /// Start Level closed this page (mode 1); the press runs when it is
    /// freed.
    Closing(u32),
    /// The kept press is running: the difficulty press the hook sees now is
    /// ours, and runs as retail.
    Continuing,
    /// The kept press ran with the page still up (mode 0): the level is
    /// loading over it. The page's free, when the menus go, starts nothing.
    Started,
}

/// What A, X or Start finds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// This soon after the page opened: the press that opened it.
    TooSoon(Duration),
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
    /// The page A, X or Start closed: run this press now.
    Continue(Pending),
    /// Closed some other way (B): the press stays kept until the next
    /// difficulty press replaces it.
    Back,
    /// Freed under the loading screen after the level was started from it.
    AfterStart,
}

/// The page's state: the one kept difficulty press, where the page is,
/// when it opened, and the scratch page the press's arguments are written
/// to. The press is stored when the hook takes it and only dropped once it
/// has run, so A, X or Start always finds it while the page is up.
pub struct PageFlow {
    kept: Mutex<(Option<Pending>, Phase)>,
    opened: OpenerGuard,
    guard: Duration,
    on: AtomicBool,
    scratch: OnceLock<u32>,
}

impl Default for PageFlow {
    fn default() -> Self {
        PageFlow {
            kept: Mutex::new((None, Phase::Shown)),
            opened: OpenerGuard::new(GUARD),
            guard: GUARD,
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
        if p.collected == 0 && p.options.is_empty() {
            return Decision::Retail(Some(format!(
                "skulls page: {profile} has no skulls collected and the level has no options; starting as retail"
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
        self.opened.opened();
        Decision::Page(pending, p.options)
    }
    /// When the page was last opened for a kept press.
    pub fn opened_at(&self) -> Option<Instant> {
        self.opened.last()
    }
    /// How long after the page opened `now` is, when that is within the
    /// guard: a continue then is the press that opened the page.
    pub fn too_soon(&self, now: Instant) -> Option<Duration> {
        self.opened.too_soon_by(now, self.guard)
    }
    /// A, X or Start on `page`, now.
    pub fn start(&self, page: u32) -> Step {
        self.start_at(page, Instant::now())
    }
    /// A, X or Start on `page` at `now`.
    pub fn start_at(&self, page: u32, now: Instant) -> Step {
        let mut kept = self.lock();
        let soon = self.too_soon(now);
        match (&kept.0, kept.1) {
            (None, _) => Step::Nothing,
            (Some(_), Phase::Closing(_) | Phase::Continuing | Phase::Started) => Step::Busy,
            (Some(p), Phase::Shown) => match soon {
                Some(since) => Step::TooSoon(since),
                None => {
                    let p = p.clone();
                    kept.1 = Phase::Closing(page);
                    Step::Close(p)
                }
            },
        }
    }
    /// Start Level with the press run at once, the page up
    /// ([`Continue::Stay`]) at `now`: the press, now running, or what
    /// stops it.
    pub fn begin_at(&self, now: Instant) -> Result<Pending, Step> {
        let mut kept = self.lock();
        let soon = self.too_soon(now);
        match (&kept.0, kept.1) {
            (None, _) => Err(Step::Nothing),
            (Some(_), Phase::Shown) if soon.is_some() => Err(Step::TooSoon(soon.unwrap())),
            (Some(p), Phase::Shown) => {
                let p = p.clone();
                kept.1 = Phase::Continuing;
                Ok(p)
            }
            (Some(_), _) => Err(Step::Busy),
        }
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
            (None, Phase::Started) => {
                kept.1 = Phase::Shown;
                Freed::AfterStart
            }
            _ => Freed::Back,
        }
    }
    /// The kept press has run (or could not be): nothing is kept.
    pub fn done(&self) {
        *self.lock() = (None, Phase::Shown);
    }
    /// The kept press has run with the page up: nothing is kept, and the
    /// page's free is the menus going.
    pub fn started(&self) {
        *self.lock() = (None, Phase::Started);
    }
    /// The press kept, if any.
    pub fn pending(&self) -> Option<Pending> {
        self.lock().0.clone()
    }
    /// Why a machine snapshot must wait: the kept press is running (its
    /// guest call is a continuation the machine check reports as well).
    pub fn snapshot_live(&self) -> Option<String> {
        (self.lock().1 == Phase::Continuing)
            .then(|| "the Skulls page's kept difficulty press is running".into())
    }
}

/// The press's arguments at `at`: a player chain (`[at]` the record at
/// `at + 8`, whose `+4` is the player index, as `[[chain]]+4` is read) and
/// the row at `at + 0x10`. Returns (chain, row pointer).
pub fn write_press(
    m: &mut dyn guest_layout::memory::GuestMemory,
    at: u32,
    p: &Pending,
) -> Result<(u32, u32), MemFault> {
    m.set_u32(at, at + 8)?;
    m.set_u32(at + 8, 0)?;
    m.set_u32(at + 0xc, p.player)?;
    m.set_u32(at + 0x10, u32::from(p.row as u16))?;
    Ok((at, at + 0x10))
}

/// A stand-in for the difficulty list at `at`: its mode byte, the only
/// part of the list a mode 0 press reads (`0x2c9d79`; the mode 0 path from
/// `0x2c9d9b` never reads `this` again). Returns the list pointer to pass.
pub fn write_stand_in(
    m: &mut dyn guest_layout::memory::GuestMemory,
    at: u32,
    p: &Pending,
) -> Result<u32, MemFault> {
    m.set_u8(at + DIFFICULTY_LIST_MODE, u8::from(p.from_select))?;
    Ok(at)
}

/// Which list the kept press runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The kept difficulty list, still there.
    List(u32),
    /// The kept list is gone with the page up (the stack frees the screen
    /// under a new one); a mode 0 press runs on a stand-in.
    StandIn,
    /// Gone, and the press needs the real list (mode 1 finds the screen's
    /// root through it): not started.
    Gone,
}

impl Target {
    /// For a kept list whose vtable reads `vtable` now, with the press
    /// in `mode`.
    pub fn of(list: u32, vtable: u32, mode: Continue, from_select: bool) -> Target {
        if vtable == DIFFICULTY_LIST_VTABLE {
            Target::List(list)
        } else if mode == Continue::Stay && !from_select {
            Target::StandIn
        } else {
            Target::Gone
        }
    }
}

/// When Start Level runs the kept press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Continue {
    /// Close the page; run the press once the stack has freed it (mode 1,
    /// a lobby's difficulty: nothing loads).
    Freed,
    /// Run the press with the page still up (mode 0: the loading screen
    /// goes on top of the page).
    Stay,
}

impl Continue {
    /// The order for a press from a list in mode 1 (`from_select`) or 0.
    pub fn for_press(from_select: bool) -> Continue {
        if from_select {
            Continue::Freed
        } else {
            Continue::Stay
        }
    }
}

/// Ask the stack to close `page`, as B does.
async fn close(cx: &Cx, page: u32) -> Result<(), EntryError> {
    let (window, slot) = lifecycle::window_slot(cx, page)?;
    eprintln!("skulls page: close requested for the page {page:#x} (window {window}, slot {slot})");
    lifecycle::close_slot(cx, window, slot).await
}

fn too_soon_line(since: Duration) -> String {
    format!(
        "skulls page: Start Level {} ms after the page opened: the press that opened it; ignored",
        since.as_millis()
    )
}

/// Start Level on the page `page`.
pub async fn action(cx: &Cx, flow: &PageFlow, page: u32, mode: Continue) -> Result<(), EntryError> {
    match mode {
        Continue::Freed => match flow.start(page) {
            Step::TooSoon(since) => {
                eprintln!("{}", too_soon_line(since));
                Ok(())
            }
            Step::Nothing => {
                eprintln!("skulls page: Start Level with no difficulty kept; nothing to start");
                Ok(())
            }
            Step::Busy => {
                eprintln!(
                    "skulls page: Start Level again; already closing, the difficulty is set when the page is freed"
                );
                Ok(())
            }
            Step::Close(p) => {
                eprintln!(
                    "skulls page: Start Level: setting the lobby's difficulty {} ({}) once the page is freed",
                    p.row,
                    difficulty_name(p.row)
                );
                close(cx, page).await
            }
        },
        Continue::Stay => match flow.begin_at(Instant::now()) {
            Err(Step::TooSoon(since)) => {
                eprintln!("{}", too_soon_line(since));
                Ok(())
            }
            Err(Step::Busy) => {
                eprintln!("skulls page: Start Level again; the level is already starting");
                Ok(())
            }
            Err(_) => {
                eprintln!("skulls page: Start Level with no difficulty kept; nothing to start");
                Ok(())
            }
            Ok(p) => {
                eprintln!(
                    "skulls page: Start Level: difficulty {} ({}) now, the page stays under the loading screen",
                    p.row,
                    difficulty_name(p.row)
                );
                if continue_level(cx, flow, &p, Continue::Stay).await? {
                    flow.started();
                }
                Ok(())
            }
        },
    }
}

/// The page `page` was freed (our destructor slot, after the Controls one).
pub async fn freed(cx: &Cx, flow: &PageFlow, page: u32) -> Result<(), EntryError> {
    match flow.freed(page) {
        Freed::Continue(p) => {
            eprintln!("skulls page: the page {page:#x} is freed; back on the difficulty list");
            continue_level(cx, flow, &p, Continue::Freed).await.map(|_| ())
        }
        Freed::Back => {
            eprintln!("skulls page: the page {page:#x} is freed (B); nothing started");
            Ok(())
        }
        Freed::AfterStart => {
            eprintln!("skulls page: the page {page:#x} is freed under the loading screen");
            Ok(())
        }
    }
}

/// Run the kept difficulty press on its own list, as the game would have:
/// it stores the difficulty and starts the level. The hook lets it through
/// ([`Phase::Continuing`]); nothing is kept after. Whether it ran.
pub async fn continue_level(
    cx: &Cx,
    flow: &PageFlow,
    p: &Pending,
    mode: Continue,
) -> Result<bool, EntryError> {
    let Some(&scratch) = flow.scratch.get() else {
        eprintln!("skulls page: no scratch page; not started");
        flow.done();
        return Ok(false);
    };
    let vtable = cx.read(|m| m.u32(p.list)).unwrap_or(0);
    let list = match Target::of(p.list, vtable, mode, p.from_select) {
        Target::List(list) => list,
        Target::StandIn => {
            eprintln!(
                "skulls page: the difficulty list at {:#x} is gone under the page \
                 (vtable {vtable:#x}); the press runs on a stand-in with its mode byte",
                p.list
            );
            match cx.write(|m| write_stand_in(m, scratch + STAND_IN, p)) {
                Ok(list) => list,
                Err(e) => {
                    flow.done();
                    return Err(e.into());
                }
            }
        }
        Target::Gone => {
            eprintln!(
                "skulls page: the difficulty list at {:#x} is gone (vtable {vtable:#x}); not started",
                p.list
            );
            flow.done();
            return Ok(false);
        }
    };
    eprintln!(
        "skulls page: starting: the difficulty press ({} for {}, list {:#x}) with {}",
        difficulty_name(p.row),
        p.profile,
        list,
        skulls_on_line(&p.profile)
    );
    let before = cx.read(|m| ui_watch::stack(&ReadGm(m), PRESS_WINDOW, PRESS_SLOT));
    eprintln!("{}", stack_line(false, before.as_ref()));
    let r = match cx.write(|m| write_press(m, scratch, p)) {
        Ok((chain, row)) => {
            cx.call(
                GuestCall::new(guest_layout::points::DIFFICULTY_ROW_CHOSEN.base)
                    .ecx(list)
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
    let after = cx.read(|m| ui_watch::stack(&ReadGm(m), PRESS_WINDOW, PRESS_SLOT));
    eprintln!("{}", stack_line(true, after.as_ref()));
    Ok(true)
}

/// The log line of the press's window and slot before or after it ran:
/// its screens, and after it whether the loading screen is among them (a
/// mode 0 press opens it there).
pub fn stack_line(after: bool, stack: Option<&ui_watch::Stack>) -> String {
    let when = if after { "after" } else { "before" };
    let Some(s) = stack else {
        return format!(
            "skulls page: {when} the press: window {PRESS_WINDOW} slot {PRESS_SLOT} unreadable"
        );
    };
    let loading = match (after, s.holds(LOADING_SCREEN)) {
        (false, _) => String::new(),
        (true, true) => format!("; the loading screen ({LOADING_SCREEN:#x}) is there"),
        (true, false) => {
            format!("; no loading screen ({LOADING_SCREEN:#x}) there: the press opened none")
        }
    };
    format!("skulls page: {when} the press: {}{loading}", s.line())
}

/// The page's state as a machine snapshot keeps it
/// (docs/runtime/snapshots.md): the kept press, where the page is, and
/// whose skulls the campaign screen shows with which options. When the
/// page last opened is wall-clock time and is not kept: a loaded page
/// takes Start Level at once.
pub fn save_state(flow: &PageFlow, screen: &SkullsScreen) -> Vec<u8> {
    use snapshot::codec::Writer;
    let w = &mut Writer::new();
    {
        let kept = flow.lock();
        match &kept.0 {
            Some(p) => w
                .bool(true)
                .u32(p.list)
                .u32(u32::from(p.row as u16))
                .u32(p.player)
                .str(&p.profile)
                .bool(p.from_select),
            None => w.bool(false),
        };
        let (phase, page) = match kept.1 {
            Phase::Shown => (0, 0),
            Phase::Closing(page) => (1, page),
            Phase::Continuing => (2, 0),
            Phase::Started => (3, 0),
        };
        w.u8(phase).u32(page);
    }
    match screen.profile() {
        Some(p) => w.bool(true).str(&p),
        None => w.bool(false),
    };
    // The page's only options are Halo 1's (`page_options`).
    w.bool(!screen.options().is_empty());
    std::mem::take(w).into_bytes()
}

/// Puts [`save_state`]'s bytes back; nothing changes unless all of them
/// decode. A screen saved never opened is left as it is: it is opened
/// again before it shows.
pub fn load_state(flow: &PageFlow, screen: &SkullsScreen, bytes: &[u8]) -> Result<(), String> {
    use snapshot::codec::Reader;
    let e = |e: snapshot::Error| e.to_string();
    let r = &mut Reader::new(bytes, "skulls-page");
    let pending = if r.bool().map_err(e)? {
        Some(Pending {
            list: r.u32().map_err(e)?,
            row: r.u32().map_err(e)? as u16 as i16,
            player: r.u32().map_err(e)?,
            profile: r.str().map_err(e)?.to_owned(),
            from_select: r.bool().map_err(e)?,
        })
    } else {
        None
    };
    let phase = match (r.u8().map_err(e)?, r.u32().map_err(e)?) {
        (0, _) => Phase::Shown,
        (1, page) => Phase::Closing(page),
        (2, _) => Phase::Continuing,
        (3, _) => Phase::Started,
        (n, _) => return Err(format!("no such Skulls page phase {n}")),
    };
    let profile = if r.bool().map_err(e)? {
        Some(r.str().map_err(e)?.to_owned())
    } else {
        None
    };
    let options: &'static [&'static OptionRow] = if r.bool().map_err(e)? {
        h1_options::CAMPAIGN
    } else {
        &[]
    };
    r.finish().map_err(e)?;
    *flow.lock() = (pending, phase);
    if let Some(profile) = profile {
        screen.open_with(profile, options);
    }
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
    let (row, player, profile, from_select, map) = {
        let mem = c.ctx.runtime().memory();
        let player = skulls_menu::player_index(&*mem, c.cpu.esp);
        (
            chosen_row(&*mem, c.cpu.esp),
            player,
            player.and_then(|p| skulls_menu::profile_of(&*mem, p)),
            from_select_level(&*mem, list),
            crate::layout_glue::Gm::u32(&*mem, crate::campaigns::SELECTED_MAP),
        )
    };
    // The level the difficulty is for: the level select's press (or New
    // Campaign) left its map id in the menu's selection. A lobby's
    // difficulty (mode 1) starts no level here.
    let key = map
        .filter(|_| !from_select)
        .and_then(crate::campaign_menu::campaign_key_of);
    let options = page_options(key.as_deref());
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
        options,
    };
    let (pending, options) = match page.flow.difficulty(press) {
        Decision::Retail(line) => {
            if let Some(line) = line {
                eprintln!("{line}");
            }
            return Ok(Flow::Continue);
        }
        Decision::Page(p, options) => (p, options),
    };
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("skulls page: difficulty chosen; the Skulls page comes first");
    }
    let Some(loader) = list_class::loader_of(SCREEN) else {
        eprintln!("skulls page: its list class is off; starting as retail");
        page.flow.done();
        return Ok(Flow::Continue);
    };
    eprintln!(
        "{}{}",
        pending.kept_line(),
        if options.is_empty() {
            ""
        } else {
            "; with the Halo 1 options"
        }
    );
    page.screen.open_with(pending.profile.clone(), options);
    let mask = 1u32 << pending.player;
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            cx.call(GuestCall::new(guest_layout::points::OPEN_SCREEN.base).args([mask, loader]))
                .await?;
            Ok(Ret::new(0, 8))
        })
    })
}

impl hooks::Feature for DifficultyPage {
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
            &guest_layout::points::DIFFICULTY_ROW_CHOSEN,
            "the Skulls page before the level",
            move |c| row_chosen(c, &page, &seen),
        );
        let (live, save, load) = (
            Arc::clone(&self.flow),
            Arc::clone(&self.flow),
            Arc::clone(&self.flow),
        );
        let (shown, reopen) = (Arc::clone(&self.screen), Arc::clone(&self.screen));
        b.snapshot(hooks::SnapshotPolicy::section_forbid_while(
            move || live.snapshot_live(),
            move || save_state(&save, &shown),
            move |bytes, _| load_state(&load, &reopen, bytes),
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list_class::Rows;
    use guest_layout::entry::{GuestCall as Gc, Regs, testing};
    use guest_layout::memory::GuestMemory;
    use guest_layout::testing::FakeGuest;
    use settings::Store;

    fn screen() -> Arc<SkullsScreen> {
        Arc::new(SkullsScreen::new(Arc::new(Store::defaults(
            crate::setting_defs::ALL,
        ))))
    }

    /// A flow with no guard: a continue right after the open counts (the
    /// guard has tests of its own).
    fn unguarded() -> PageFlow {
        PageFlow {
            guard: Duration::ZERO,
            ..PageFlow::default()
        }
    }

    fn press(row: i16, collected: usize) -> Press {
        Press {
            list: 0x5000,
            row: Some(row),
            player: Some(1),
            profile: Some("Chief".into()),
            from_select: false,
            collected,
            options: &[],
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
        let f = unguarded();
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
        let f = unguarded();
        assert_eq!(f.difficulty(press(2, 1)), Decision::Page(kept(2), &[]));
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
        let f = unguarded();
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
        let f = unguarded();
        f.difficulty(press(1, 1));
        assert_eq!(f.freed(0x100), Freed::Back);
        assert_eq!(f.difficulty(press(0, 1)), Decision::Page(kept(0), &[]));
        assert_eq!(f.start(0x300), Step::Close(kept(0)));
        assert_eq!(f.freed(0x100), Freed::Back, "not the page closed");
        assert_eq!(f.freed(0x300), Freed::Continue(kept(0)));
    }

    /// B closes the page (the generic back handler); on the difficulty list
    /// again, another difficulty opens the page with that one.
    #[test]
    fn back_then_another_difficulty_keeps_the_new_one() {
        let f = unguarded();
        assert!(matches!(f.difficulty(press(3, 2)), Decision::Page(..)));
        assert!(matches!(f.difficulty(press(0, 2)), Decision::Page(p, _) if p.row == 0));
        assert!(matches!(f.start(1), Step::Close(p) if p.row == 0));
    }

    #[test]
    fn only_the_four_difficulties_and_a_known_player_open_the_page() {
        let f = unguarded();
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

    /// The level select's (and New Campaign's) difficulty goes straight
    /// on with the page up; a lobby's closes the page first.
    #[test]
    fn the_order_follows_the_lists_mode() {
        assert_eq!(Continue::for_press(false), Continue::Stay);
        assert_eq!(Continue::for_press(true), Continue::Freed);
    }

    const PAGE: u32 = 0x0010_0000;
    const LIST: u32 = 0x0020_0000;
    const SCRATCH: u32 = 0x0030_0000;

    fn guest(list_vtable: u32) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(PAGE, 0x100).map(LIST, 0x400).map(SCRATCH, 0x1000);
        g.put_u32(PAGE + SCREEN_WINDOW, 5)
            .put_u32(PAGE + SCREEN_SLOT, 4)
            .put_u32(LIST, list_vtable);
        g
    }

    fn flow() -> Arc<PageFlow> {
        let f = Arc::new(unguarded());
        f.scratch.set(SCRATCH).unwrap();
        assert!(matches!(
            f.difficulty(Press {
                list: LIST,
                ..press(1, 1)
            }),
            Decision::Page(..)
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
        Gc::new(guest_layout::points::DIFFICULTY_ROW_CHOSEN.base)
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

    /// Start Level from the level select: the press at once, the page
    /// left up (its loading screen goes on top of it), no close and no
    /// wait for a free: the level loads straight away. Pressed again, or
    /// the page freed later under the loading screen, nothing more.
    #[test]
    fn start_level_runs_the_press_at_once_without_closing() {
        let f = flow();
        let run = run(&f, guest(DIFFICULTY_LIST_VTABLE), |cx, f| {
            Box::pin(async move {
                action(&cx, &f, PAGE, Continue::Stay).await?;
                action(&cx, &f, PAGE, Continue::Stay).await?;
                freed(&cx, &f, PAGE).await?;
                Ok(Ret::new(0, 0))
            })
        });
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
        assert_eq!(run.calls, [the_press()], "once, and never a close");
        assert_eq!(f.pending(), None);
        assert_eq!(f.freed(PAGE), Freed::Back, "the next page starts fresh");
    }

    /// The second failure: a continue pressed while the page was still
    /// animating in went back and never loaded. With the press run from the
    /// page itself there is no close to race: any press after the guard
    /// starts the level, at any moment of the page's opening.
    #[test]
    fn start_level_right_after_the_guard_still_loads() {
        let f = Arc::new(PageFlow::default());
        f.scratch.set(SCRATCH).unwrap();
        f.difficulty(Press {
            list: LIST,
            ..press(1, 1)
        });
        let opened = f.opened_at().unwrap();
        assert!(matches!(f.begin_at(opened), Err(Step::TooSoon(_))));
        assert_eq!(f.pending().map(|p| p.row), Some(1), "kept: not dropped");
        let p = f
            .begin_at(opened + GUARD)
            .expect("the press, at the guard's end");
        assert_eq!(p.row, 1);
        assert_eq!(f.begin_at(opened + GUARD), Err(Step::Busy));
    }

    /// The lines around the press say what window 5 slot 4 holds, and
    /// after it whether the loading screen went there.
    #[test]
    fn the_press_is_logged_with_the_screens_around_it() {
        let stack = |list: Vec<(u32, Option<u32>)>| ui_watch::Stack {
            window: 5,
            slot: 4,
            active: list[0].0,
            pending: 0,
            list,
        };
        let page = stack(vec![(0x100, Some(0x2a)), (0x200, Some(0x0c))]);
        assert_eq!(
            stack_line(false, Some(&page)),
            "skulls page: before the press: window 5 slot 4: active 0x100, pending 0x0, \
             list [0x100/0x2a 0x200/0xc]"
        );
        let after = stack_line(true, Some(&page));
        assert!(
            after.ends_with("no loading screen (0xd1) there: the press opened none"),
            "{after}"
        );
        let loading = stack(vec![(0x300, Some(LOADING_SCREEN)), (0x100, Some(0x2a))]);
        let after = stack_line(true, Some(&loading));
        assert!(after.ends_with("; the loading screen (0xd1) is there"), "{after}");
        assert_eq!(
            stack_line(true, None),
            "skulls page: after the press: window 5 slot 4 unreadable"
        );
    }

    /// Seen live (2026-11-19, `h2 serve --settings`, a Halo 1 level from
    /// the level select): with the page up the difficulty list was gone
    /// (vtable 0x458788 at its address), so Start Level said "not started"
    /// and every press after found nothing kept. A mode 0 press reads only
    /// the list's mode byte, so it runs on a stand-in holding it.
    #[test]
    fn start_level_starts_when_the_list_under_the_page_is_gone() {
        let f = flow();
        let run = run(&f, guest(0x0045_8788), |cx, f| {
            Box::pin(async move {
                action(&cx, &f, PAGE, Continue::Stay).await?;
                Ok(Ret::new(0, 0))
            })
        });
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
        let stand_in = SCRATCH + STAND_IN;
        assert_eq!(
            run.calls,
            [Gc::new(guest_layout::points::DIFFICULTY_ROW_CHOSEN.base)
                .ecx(stand_in)
                .args([SCRATCH, SCRATCH + 0x10])],
            "the press, once, on the stand-in"
        );
        assert_eq!(run.guest.u8(stand_in + DIFFICULTY_LIST_MODE), Ok(0), "mode 0");
        assert_eq!(f.pending(), None);
    }

    #[test]
    fn the_press_runs_on_the_list_or_a_stand_in_by_mode() {
        let ok = DIFFICULTY_LIST_VTABLE;
        assert_eq!(Target::of(LIST, ok, Continue::Stay, false), Target::List(LIST));
        assert_eq!(Target::of(LIST, ok, Continue::Freed, true), Target::List(LIST));
        assert_eq!(Target::of(LIST, 0x458788, Continue::Stay, false), Target::StandIn);
        // A lobby's press finds the screen through the real list.
        assert_eq!(Target::of(LIST, 0x458788, Continue::Freed, true), Target::Gone);
        assert_eq!(Target::of(LIST, 0, Continue::Stay, true), Target::Gone);
        assert_eq!(Target::of(LIST, 0, Continue::Freed, false), Target::Gone);
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

    /// Start Level first, then the skulls; A on Start Level is the class's
    /// action (on to the level). With no profile every skull is locked, so
    /// A on one does nothing. X and Start are not the page's
    /// (`action_buttons(&[])`), and the class takes no left or right.
    #[test]
    fn start_level_is_first_and_a_on_it_goes_on() {
        let r = screen();
        assert_eq!(r.count(), 1 + skulls::TOTAL);
        assert_eq!(r.press(0), crate::ui::Chosen::Act);
        assert_eq!(r.texts(0), [(crate::ui::LABEL_SLOT, skulls_menu::START_ID)]);
        for row in 1..r.count() as i16 {
            assert_eq!(r.press(row), crate::ui::Chosen::Nothing, "row {row}");
            assert_eq!(r.sideways(row, true), None, "row {row}");
        }
        let class = class(
            Arc::clone(&r),
            skulls_menu::DESCRIPTION_TEXT,
            skulls_menu::LIST_LAYOUT,
            skulls_menu::PICTURE,
            Arc::new(PageFlow::default()),
        );
        assert!(!class.behaviour().sideways, "no left and right steps");
    }

    /// A Halo 1 level's page: Start Level, its options, then the skulls,
    /// and it opens even with no skull collected.
    #[test]
    fn a_halo1_level_gets_its_options_above_the_skulls() {
        assert_eq!(page_options(Some("halo1")), h1_options::CAMPAIGN);
        assert!(page_options(Some("other")).is_empty());
        assert!(page_options(None).is_empty());
        let f = unguarded();
        let p = Press {
            options: h1_options::CAMPAIGN,
            ..press(2, 0)
        };
        assert_eq!(
            f.difficulty(p),
            Decision::Page(kept(2), h1_options::CAMPAIGN)
        );
        let r = screen();
        r.open_with("Chief".into(), h1_options::CAMPAIGN);
        assert_eq!(r.count(), 1 + 2 + skulls::TOTAL);
        assert_eq!(
            r.page_row(1),
            Some(skulls_menu::PageRow::Option(&h1_options::ASSETS))
        );
        assert_eq!(
            r.page_row(2),
            Some(skulls_menu::PageRow::Option(&h1_options::HUD))
        );
        assert_eq!(r.row_of_skull(crate::skulls::by_tier()[0]), Some(3));
    }

    /// Mode 1 (a lobby's difficulty): Start Level closes the page; the
    /// press runs once it is freed, and a second press while closing does
    /// nothing more.
    #[test]
    fn a_lobby_difficulty_closes_the_page_then_sets_it() {
        let f = flow();
        let run = run(&f, guest(DIFFICULTY_LIST_VTABLE), |cx, f| {
            Box::pin(async move {
                action(&cx, &f, PAGE, Continue::Freed).await?;
                // A second press (Start after A) while it closes: nothing more.
                action(&cx, &f, PAGE, Continue::Freed).await?;
                freed(&cx, &f, PAGE).await?;
                Ok(Ret::new(0, 0))
            })
        });
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
        assert_eq!(
            run.calls,
            [Gc::new(SCREEN_CLOSE).eax(4).ecx(5), the_press()]
        );
        assert_eq!(f.pending(), None);
    }

    /// The A that opened the page is not a continue: within the guard of
    /// the open a press is dropped and the page stays; a press after it
    /// (a second tap of a player spamming A) goes on.
    #[test]
    fn the_press_that_opened_the_page_does_not_continue() {
        let f = PageFlow::default();
        assert!(matches!(f.difficulty(press(2, 1)), Decision::Page(..)));
        let opened = f.opened_at().expect("opened");
        assert!(matches!(f.start_at(0x100, opened), Step::TooSoon(d) if d.is_zero()));
        assert_eq!(
            f.start_at(0x100, opened + Duration::from_millis(20)),
            Step::TooSoon(Duration::from_millis(20))
        );
        assert_eq!(f.pending(), Some(kept(2)), "still kept, page still up");
        assert_eq!(f.freed(0x100), Freed::Back, "not closing");
        // A new tap, 80 ms on: through.
        assert_eq!(
            f.start_at(0x100, opened + Duration::from_millis(80)),
            Step::Close(kept(2))
        );
        // Nothing kept: no guard to speak of.
        let f = PageFlow::default();
        assert_eq!(f.start(0x100), Step::Nothing);
        assert_eq!(
            too_soon_line(Duration::from_millis(12)),
            "skulls page: Start Level 12 ms after the page opened: the press that opened it; ignored"
        );
    }

    /// Through `action` too: right after the open nothing is called; the
    /// same flow a moment later closes the page.
    #[test]
    fn a_continue_right_after_the_open_calls_nothing() {
        for mode in [Continue::Freed, Continue::Stay] {
            let f = Arc::new(PageFlow {
                guard: Duration::from_secs(3600),
                ..PageFlow::default()
            });
            f.scratch.set(SCRATCH).unwrap();
            f.difficulty(Press {
                list: LIST,
                ..press(1, 1)
            });
            let run = testing::run(
                &{
                    let f = Arc::clone(&f);
                    move |cx: Cx| -> EntryFuture {
                        let f = Arc::clone(&f);
                        Box::pin(async move {
                            action(&cx, &f, PAGE, mode).await?;
                            Ok(Ret::new(0, 0))
                        })
                    }
                },
                guest(DIFFICULTY_LIST_VTABLE),
                Regs::default(),
                |_, _| Regs::default(),
            );
            assert_eq!(run.calls, [], "{mode:?}");
            assert_eq!(f.pending().map(|p| p.row), Some(1), "{mode:?}");
            assert_eq!(f.freed(PAGE), Freed::Back, "{mode:?}: not closing");
        }
    }

    #[test]
    fn the_guard_is_under_a_second_tap() {
        assert!(
            GUARD < Duration::from_millis(80),
            "under a quick second tap"
        );
    }

    #[test]
    fn the_hook_and_the_class_are_on_the_plan() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        let s = screen();
        let flow = Arc::new(unguarded());
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
        let text = r.build(&hooks::Base).unwrap().text();
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

    #[test]
    fn a_snapshot_carries_the_kept_press_and_the_open_screen() {
        let (f, s) = (flow(), screen());
        s.open_with("Chief".into(), h1_options::CAMPAIGN);
        assert!(matches!(f.start(0x7000), Step::Close(_)));
        assert_eq!(f.snapshot_live(), None);
        let bytes = save_state(&f, &s);

        let (g, t) = (unguarded(), screen());
        load_state(&g, &t, &bytes).unwrap();
        assert_eq!(g.pending(), f.pending());
        assert_eq!(g.freed(0x7000), Freed::Continue(f.pending().unwrap()));
        assert_eq!(t.profile().as_deref(), Some("Chief"));
        assert_eq!(t.options().len(), h1_options::CAMPAIGN.len());
        load_state(&g, &t, &bytes).unwrap();
        assert_eq!(save_state(&g, &t), bytes, "the same state again");

        assert!(load_state(&g, &t, &bytes[..bytes.len() - 1]).is_err());
        let mut odd = bytes.clone();
        odd.push(0);
        assert!(load_state(&g, &t, &odd).is_err(), "trailing bytes");

        // Nothing kept and a screen never opened.
        let (h, u) = (unguarded(), screen());
        let empty = save_state(&h, &u);
        load_state(&g, &t, &empty).unwrap();
        assert_eq!(g.pending(), None);
        assert_eq!(t.profile().as_deref(), Some("Chief"), "left as it was");
    }

    #[test]
    fn snapshots_wait_while_the_kept_press_runs() {
        let f = flow();
        assert_eq!(f.snapshot_live(), None, "the page is up");
        assert!(matches!(f.start(0x7000), Step::Close(_)));
        assert_eq!(f.snapshot_live(), None, "closing");
        assert!(matches!(f.freed(0x7000), Freed::Continue(_)));
        assert_eq!(
            f.snapshot_live().as_deref(),
            Some("the Skulls page's kept difficulty press is running")
        );
        f.done();
        assert_eq!(f.snapshot_live(), None);
    }
}
