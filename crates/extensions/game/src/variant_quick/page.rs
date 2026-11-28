//! The lobby's Quick Options dialog for a variant whose record names a
//! rules script (docs/game/rules-variants.md, "Quick Options in the
//! lobby"; switch [`super::FEATURE`]).
//!
//! **The page** is the shared settings dialog
//! (docs/game/ui-builder.md "The settings dialog") from
//! `content/ui/screens/variant-quick.yaml`: Quick Options' own panel, the
//! variant's name in the header (`settings.name: "{variant}"`), eight row
//! slots bound `quick.0` to `quick.7` and the focused row's help under
//! the rows. Its Rust is its bindings ([`page`]): a slot is the dialog's
//! row of that place ([`super::dialog_rows_with`]: the script's marks,
//! then its unmarked options when they all fit, so Siege's dialog is its
//! whole options page), its label, its value and its help ([`help`]: an
//! option's description, else what the setting sets), and a slot past
//! the last row is not drawn. A opens the value picker on the row's
//! values, a wide range as a ladder ([`super::ladder`]); there are no
//! left and right steps.
//!
//! **Which Quick Options.** Game Setup's "Quick Options..." row (action
//! [`QUICK_OPTIONS`]) opens this page for a script variant with marks,
//! retail's screen 222 for any other, and nothing for a script with no
//! marks: that row is greyed ([`entry`]). The row's press is
//! [`SetupRow`]'s hook; the variant is the lobby's
//! (`crate::pause_change_map::lobby_variant`), read as the row is
//! pressed, so the page shows the record as it is then
//! ([`Dialog::open`]).
//!
//! **A change** ([`change`]) is one setting (`GameVariant::set`) or one
//! option's bits in the block (`variant_page::OptionRow::set`, the More
//! Options page's own, through `Ext::encode`, which keeps the CRC),
//! checked with `record::refused`. A refusal is said in the row's help
//! and changes nothing. A change that passes waits in [`Changes`] for the
//! next UI update, which hands the record to the lobby variant setter
//! (`points::LOBBY_SET_VARIANT`, [`set_variant`]); a record the setter
//! refuses is said in the row's help and the page shows the lobby's
//! record again. An option change on a record built from one of our list
//! entries also retargets that entry (`game_variants::inject::retarget`),
//! so the block the game writes at activation is the changed one. The
//! change is the lobby's variant, not a save, as retail's Quick Options
//! changes are. `variants.quick.set` makes the same change through the
//! same queue ([`agent_set`]).
//!
//! **Greyed.** No research names how retail draws a Game Setup row
//! disabled, so the greyed row keeps retail's text and its press does
//! nothing (docs/game/rules-variants.md, "Not verified").

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use ::rules::ir::Ir;
use agent_proto::{RpcError, code};
use guest_layout::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use guest_layout::memory::GuestMemory;
use hooks::{BuildError, Builder, Flow, Manifest, SnapshotPolicy, points};
use runtime::guest::Permissions;
use variants::ext::Ext;
use variants::record::{self, GameVariant, NAME_AT, SettingId};

use super::{DIALOG_ROWS, Mark, QuickRow, ROWS};
use crate::h1_options_row::{self as h1row, QUICK_OPTIONS};
use crate::list_class;
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::picker::Pick;
use crate::ui_kit::screen::{Filled, KitScreen, KitTexts};
use crate::ui_kit::settings::{self, Bindings};

/// The page's screen name (`crate::ui_kit::screens::NAMES`).
pub const SCREEN: &str = "variant-quick";
/// The page's list class (`features.variant-quick-screen`).
pub const SCREEN_FEATURE: &str = "variant-quick-screen";
/// Game Setup's row and the setter's driver (`features.variant-quick-row`).
pub const ROW_FEATURE: &str = "variant-quick-row";

const IDS: Family = ids::family("variant-quick");

/// The slots' bindings, in row order (the doc's `rows.row.N.bind`).
pub const SLOTS: [&str; DIALOG_ROWS] = [
    "quick.0", "quick.1", "quick.2", "quick.3", "quick.4", "quick.5", "quick.6", "quick.7",
];

/// The header's name with no variant read (the preview's, before a press).
const NO_VARIANT: &str = "Quick Options";

// ---- which Quick Options ----

/// What Game Setup's "Quick Options..." row does for the lobby's variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// Retail's screen 222: a variant with no script, one naming a script
    /// this machine lacks, or the page off.
    Retail,
    /// This page: a script with Quick Options marks.
    Ours,
    /// A script with no marks: the row is greyed and its press does
    /// nothing. Change Rules edits everything.
    Greyed,
}

/// The row's entry for a variant whose script (this machine's) is `ir`;
/// `on`: the page's class is installed.
pub fn entry(ir: Option<&Ir>, on: bool) -> Entry {
    match ir {
        Some(ir) if on && ir.quick.is_empty() => Entry::Greyed,
        Some(_) if on => Entry::Ours,
        _ => Entry::Retail,
    }
}

// ---- a change ----

/// `record` with row `row` of its script `ir` at `value`: one setting
/// changed, or one option's bits in the block in effect (the rest of the
/// block kept, its CRC made again). Why not, when the value is not one
/// the row offers or the record it makes is one the game would not keep
/// (`record::refused`).
pub fn change(
    record: &GameVariant,
    ir: &Ir,
    row: &QuickRow,
    value: i32,
) -> Result<GameVariant, String> {
    if !row.values.iter().any(|(v, _)| *v == value) {
        return Err(if row.values.is_empty() {
            format!("{} has no values to choose from yet", row.label)
        } else {
            format!("{value} is not a value of {}", row.label)
        });
    }
    let mut next = record.clone();
    match row.mark {
        Mark::Setting { setting, .. } => next
            .set(SettingId(setting), value)
            .map_err(|e| format!("{}: {e}", row.label))?,
        Mark::Option { index } => {
            let field = crate::variant_page::option_rows(ir)
                .into_iter()
                .nth(usize::from(index))
                .ok_or_else(|| format!("{}: the script has no option {index}", row.label))?;
            let mut e = Ext::in_effect(&super::block_of(record));
            e.script.options = field.set(e.script.options, value);
            next.set_ext(&e)
                .map_err(|e| format!("{}: {e}", row.label))?;
        }
    }
    if let Some(p) = record::refused(&next).into_iter().next() {
        return Err(p.what);
    }
    Ok(next)
}

/// A change waiting for the setter ([`Changes`]).
pub type Ticket = u64;

struct Request {
    ticket: Ticket,
    record: GameVariant,
    /// The lobby's record before it.
    was: GameVariant,
}

/// Changes waiting for the lobby variant setter, and how the last ones
/// went. The game's is [`changes`]; a test makes its own.
#[derive(Default)]
pub struct Changes {
    waiting: Mutex<VecDeque<Request>>,
    done: Mutex<VecDeque<(Ticket, Result<(), String>)>>,
    next: AtomicU64,
}

/// Outcomes kept for `variants.quick.set` to collect.
const KEPT: usize = 16;

/// What the setter answered for a refused record.
pub const REFUSED: &str = "the game refused the variant";

impl Changes {
    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Queues `record` (the lobby's was `was`) for the setter: its ticket.
    pub fn queue(&self, record: GameVariant, was: GameVariant) -> Ticket {
        let ticket = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        Self::lock(&self.waiting).push_back(Request {
            ticket,
            record,
            was,
        });
        ticket
    }
    /// Whether a change waits.
    pub fn waiting(&self) -> bool {
        !Self::lock(&self.waiting).is_empty()
    }
    /// How change `ticket` went, once: `None` while it waits (or once it
    /// was collected).
    pub fn outcome(&self, ticket: Ticket) -> Option<Result<(), String>> {
        let mut done = Self::lock(&self.done);
        let i = done.iter().position(|(t, _)| *t == ticket)?;
        done.remove(i).map(|(_, r)| r)
    }
    fn settle(&self, ticket: Ticket, r: Result<(), String>) {
        let mut done = Self::lock(&self.done);
        done.push_back((ticket, r));
        while done.len() > KEPT {
            done.pop_front();
        }
    }
    /// Hands each waiting change to the setter in turn, the record
    /// written at `at` first; `dialog` hears how each went.
    pub async fn drain(&self, cx: &Cx, at: u32, dialog: &Dialog) -> Result<(), EntryError> {
        loop {
            let Some(r) = Self::lock(&self.waiting).pop_front() else {
                return Ok(());
            };
            let taken = set_variant(cx, at, &r.record).await?;
            let outcome = if taken {
                if r.record.ext_block() != r.was.ext_block() {
                    let name =
                        &r.record.bytes()[NAME_AT..NAME_AT + crate::rules::gate::RECORD_NAME_LEN];
                    let engine = r.record.game_type().engine();
                    crate::game_variants::inject::retarget(name, engine, *r.record.ext_block());
                }
                eprintln!(
                    "variant quick: the lobby's variant {} is set",
                    r.record.name()
                );
                Ok(())
            } else {
                eprintln!("variant quick: {REFUSED}: {}", r.record.name());
                Err(REFUSED.to_string())
            };
            dialog.settled(r.ticket, &outcome, &r.was);
            self.settle(r.ticket, outcome);
        }
    }
}

/// The game's changes: the dialog's and `variants.quick.set`'s.
pub fn changes() -> &'static Arc<Changes> {
    static CHANGES: LazyLock<Arc<Changes>> = LazyLock::new(Arc::default);
    &CHANGES
}

/// Writes `record` at `at` and hands it to the lobby variant setter
/// (`points::LOBBY_SET_VARIANT`, stack the record): whether it took it
/// (AL nonzero).
pub async fn set_variant(cx: &Cx, at: u32, record: &GameVariant) -> Result<bool, EntryError> {
    cx.write(|m| m.write(at, record.bytes()))?;
    let r = cx
        .call(GuestCall::new(points::LOBBY_SET_VARIANT.base).args([at]))
        .await?;
    Ok(r.eax as u8 != 0)
}

// ---- the dialog ----

/// A row's help under the rows: an option's description, else what the
/// row sets ("Sets the respawn time for this lobby's games."), as every
/// settings row has its help.
pub fn help(row: &QuickRow) -> String {
    row.help.clone().unwrap_or_else(|| {
        format!(
            "Sets the {} for this lobby's games.",
            row.label.to_lowercase()
        )
    })
}

#[derive(Default)]
struct Open {
    record: Option<GameVariant>,
    ir: Option<Arc<Ir>>,
    offered: Option<HashMap<u32, Vec<i32>>>,
    rows: Vec<QuickRow>,
    /// A refusal, said in that slot's help until the next change or open.
    said: Option<(usize, String)>,
    /// The last change made here, while it waits for the setter.
    waiting: Option<Ticket>,
}

impl Open {
    fn read(&mut self) {
        self.rows = match (&self.record, &self.ir) {
            (Some(r), Some(ir)) => super::dialog_rows_with(r, ir, self.offered.as_ref()),
            _ => Vec::new(),
        };
    }
}

/// What the page shows: the lobby's record as it was when the row was
/// pressed, with the changes made here since, and its script's rows.
pub struct Dialog {
    changes: Arc<Changes>,
    open: Mutex<Open>,
}

impl Dialog {
    /// A dialog whose changes wait in `changes`.
    pub fn new(changes: Arc<Changes>) -> Self {
        Dialog {
            changes,
            open: Mutex::default(),
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Open> {
        self.open.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Shows `record`, its script `ir` and the values the editor offered
    /// for each setting (`community_variants::last_offered`).
    pub fn open(&self, record: GameVariant, ir: Arc<Ir>, offered: Option<HashMap<u32, Vec<i32>>>) {
        let mut o = self.lock();
        *o = Open {
            record: Some(record),
            ir: Some(ir),
            offered,
            ..Open::default()
        };
        o.read();
    }
    /// The header's name: the variant's.
    pub fn name(&self) -> String {
        self.lock()
            .record
            .as_ref()
            .map_or_else(|| NO_VARIANT.to_string(), GameVariant::name)
    }
    /// The rows shown.
    pub fn rows(&self) -> Vec<QuickRow> {
        self.lock().rows.clone()
    }
    /// The record shown.
    pub fn record(&self) -> Option<GameVariant> {
        self.lock().record.clone()
    }
    /// Slot `slot`'s label, value and help; `None` past the last mark.
    pub fn fill(&self, slot: usize) -> Option<Filled> {
        let o = self.lock();
        let row = o.rows.get(slot)?;
        let said = o
            .said
            .as_ref()
            .filter(|(s, _)| *s == slot)
            .map(|(_, why)| why.clone());
        Some(Filled {
            label: row.label.clone(),
            shown: row.text.clone(),
            help: Some(said.unwrap_or_else(|| help(row))),
        })
    }
    /// The pick A opens on slot `slot`: the row's values, its value held.
    pub fn offer(self: &Arc<Self>, slot: usize) -> Result<Pick, String> {
        let row = self
            .lock()
            .rows
            .get(slot)
            .cloned()
            .ok_or_else(|| format!("variant quick: no row {slot}"))?;
        if row.values.is_empty() {
            return Err(format!(
                "variant quick: {} has no values to choose from yet",
                row.label
            ));
        }
        // A wide range is offered as a ladder, the held value on it.
        let offered = super::ladder(&row.values, row.value);
        let current = offered
            .iter()
            .position(|(v, _)| *v == row.value)
            .unwrap_or(0);
        let values: Vec<i32> = offered.iter().map(|(v, _)| *v).collect();
        let me = Arc::clone(self);
        Ok(Pick::new(
            &row.label,
            offered.iter().map(|(_, t)| t.clone()).collect(),
            current,
            &help(&row),
            Arc::new(move |i| match values.get(i) {
                Some(&v) => me.choose(slot, v),
                None => format!("variant quick: no value {i}"),
            }),
        ))
    }
    /// Sets slot `slot` to `value`: the change queued for the setter and
    /// shown at once, or the refusal said in the slot's help. What to log.
    pub fn choose(&self, slot: usize, value: i32) -> String {
        let mut o = self.lock();
        let (Some(record), Some(ir), Some(row)) =
            (o.record.clone(), o.ir.clone(), o.rows.get(slot).cloned())
        else {
            return format!("variant quick: no row {slot}");
        };
        if value == row.value {
            o.said = None;
            return format!("variant quick: {} stays {}", row.label, row.text);
        }
        match change(&record, &ir, &row, value) {
            Ok(next) => {
                o.waiting = Some(self.changes.queue(next.clone(), record));
                o.record = Some(next);
                o.said = None;
                o.read();
                let text = o
                    .rows
                    .get(slot)
                    .map_or_else(String::new, |r| r.text.clone());
                format!("variant quick: {} {} -> {text}", row.label, row.text)
            }
            Err(why) => {
                let line = format!("variant quick: {} not changed: {why}", row.label);
                o.said = Some((slot, why));
                line
            }
        }
    }
    /// Change `ticket` went `outcome`; the lobby's record before it was
    /// `was`. A refusal of the last change made here shows the lobby's
    /// record again and says why.
    fn settled(&self, ticket: Ticket, outcome: &Result<(), String>, was: &GameVariant) {
        let mut o = self.lock();
        if o.waiting != Some(ticket) {
            return;
        }
        o.waiting = None;
        if let Err(why) = outcome {
            o.record = Some(was.clone());
            o.read();
            o.said = Some((0, why.clone()));
        }
    }
}

/// The game's dialog.
pub fn dialog() -> &'static Arc<Dialog> {
    static DIALOG: LazyLock<Arc<Dialog>> =
        LazyLock::new(|| Arc::new(Dialog::new(Arc::clone(changes()))));
    &DIALOG
}

/// The page over `dialog`: `variant-quick.yaml` with its bindings.
pub fn page(dialog: &Arc<Dialog>) -> KitScreen {
    let d = Arc::clone(dialog);
    let mut binds = Bindings::new().name(move || d.name());
    for (slot, bind) in SLOTS.into_iter().enumerate() {
        let (fill, offer) = (Arc::clone(dialog), Arc::clone(dialog));
        binds = binds.slot(bind, move || fill.fill(slot), move || offer.offer(slot));
    }
    settings::screen(SCREEN_FEATURE, SCREEN, IDS, None, binds).unwrap_or_else(|e| panic!("{e}"))
}

// ---- the agent's change ----

/// `variants.quick.set` (`agent::variants`): a change of the lobby's
/// variant through [`changes`].
pub fn agent_set(m: &dyn GuestMemory, slot: u8, value: i32) -> Result<Ticket, RpcError> {
    let record = crate::pause_change_map::lobby_variant(m)
        .ok_or_else(|| RpcError::new(code::UNAVAILABLE, "no lobby variant to change"))?;
    let ir = super::script_for(&record).ok_or_else(|| {
        RpcError::new(
            code::UNAVAILABLE,
            format!(
                "{} names no rules script this machine has: retail's Quick Options",
                record.name()
            ),
        )
    })?;
    let rows = super::rows(&record, &ir);
    let row = rows.get(usize::from(slot)).ok_or_else(|| {
        RpcError::new(
            code::INVALID_PARAMS,
            format!("slot {slot}: the script marks {} rows", rows.len()),
        )
    })?;
    let next =
        change(&record, &ir, row, value).map_err(|why| RpcError::new(code::INVALID_PARAMS, why))?;
    Ok(changes().queue(next, record))
}

/// `variants.quick.set`'s outcome for `ticket`.
pub fn agent_outcome(ticket: Ticket) -> Option<Result<(), RpcError>> {
    changes()
        .outcome(ticket)
        .map(|r| r.map_err(|why| RpcError::new(code::UNAVAILABLE, why)))
}

// ---- the Game Setup row and the setter's driver ----

/// Game Setup's "Quick Options..." row for a script variant, and the UI
/// update that hands the changes to the setter.
pub struct SetupRow {
    pub dialog: Arc<Dialog>,
    pub changes: Arc<Changes>,
}

/// The setter's own UI update call is let through (see
/// `crate::pause_change_map`'s driver, whose pattern this is).
static PASSING: AtomicBool = AtomicBool::new(false);

impl hooks::Feature for SetupRow {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ROW_FEATURE,
            "Quick Options for a script variant on Game Setup",
        )
        .doc("docs/game/rules-variants.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use points::SQUAD_SETTINGS_ROW_CHOSEN;
        if !b.switches().feature(super::FEATURE, true) {
            return Ok(());
        }
        let request_at = b.guest_alloc(0x1000, Permissions::RW)?;
        let record_at = b.guest_alloc(0x1000, Permissions::RW)?;
        let dialog = Arc::clone(&self.dialog);
        let greyed = Arc::new(AtomicBool::new(false));
        b.raw()
            .before(&SQUAD_SETTINGS_ROW_CHOSEN, "the Quick Options row's press", move |c| {
                let p = h1row::setup_press(c);
                if p.action != Some(QUICK_OPTIONS) {
                    return Ok(Flow::Continue);
                }
                let record = crate::pause_change_map::lobby_variant(&*c.ctx.runtime().memory());
                // The script is found with guest memory released.
                let Some(record) = record else {
                    return Ok(Flow::Continue);
                };
                let ir = super::script_for(&record);
                match entry(ir.as_deref(), list_class::loader_of(SCREEN).is_some()) {
                    Entry::Retail => Ok(Flow::Continue),
                    Entry::Greyed => {
                        if !greyed.swap(true, Ordering::Relaxed) {
                            eprintln!(
                                "variant quick: {} marks no Quick Options rows: the row is greyed",
                                record.name()
                            );
                        }
                        c.ret(8).map(Flow::Return)
                    }
                    Entry::Ours => {
                        let ir = ir.expect("Ours has a script");
                        dialog.open(record, ir, crate::community_variants::last_offered());
                        h1row::open_from_setup(c, request_at, p, SCREEN, "variant quick")
                    }
                }
            })
            .after_feature(crate::clankers::ROW_FEATURE)
            .after_feature(crate::clankermatch::SWITCH_FEATURE)
            .after_feature(h1row::ROW_FEATURE)
            .after_feature(crate::firefight::options::ROW_FEATURE)
            .after_feature(crate::lobby_mode::FEATURE);

        let (changes, dialog) = (Arc::clone(&self.changes), Arc::clone(&self.dialog));
        b.raw()
            .before(
                &points::UI_UPDATE,
                "variant quick: set the lobby's variant",
                move |c| {
                    if PASSING.load(Ordering::SeqCst) || !changes.waiting() {
                        return Ok(Flow::Continue);
                    }
                    let (changes, dialog) = (Arc::clone(&changes), Arc::clone(&dialog));
                    c.answer_async(move |cx| -> EntryFuture {
                        Box::pin(async move {
                            let elapsed = cx.arg(0)?;
                            if let Err(e) = changes.drain(&cx, record_at, &dialog).await {
                                eprintln!("variant quick: {e}");
                            }
                            PASSING.store(true, Ordering::SeqCst);
                            let out = cx
                                .call(GuestCall::new(points::UI_UPDATE.base).args([elapsed]))
                                .await;
                            PASSING.store(false, Ordering::SeqCst);
                            Ok(Ret::new(out?.eax, 4))
                        })
                    })
                },
            )
            // Inside the other UI update drivers, as theirs are.
            .after_feature("live-join-link")
            .after_feature("theater-link")
            .after_feature("forge-screens")
            .after_feature("lab-takeover")
            .after_feature(crate::pause_change_map::FEATURE)
            .optional();

        let live = Arc::clone(&self.changes);
        b.snapshot(SnapshotPolicy::forbid_while(move || {
            live.waiting()
                .then(|| "a Quick Options change waits for the lobby variant setter".to_string())
        }));
        Ok(())
    }
}

/// Adds the page, its texts and the Game Setup row (`variant-quick` on).
pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    let page = Arc::new(page(dialog()));
    registry
        .add(&KitTexts(Arc::clone(&page)))
        .map_err(|e| anyhow::anyhow!("variant quick texts: {e}"))?;
    registry
        .add(&page.class())
        .map_err(|e| anyhow::anyhow!("variant quick page: {e}"))?;
    registry
        .add(&SetupRow {
            dialog: Arc::clone(dialog()),
            changes: Arc::clone(changes()),
        })
        .map_err(|e| anyhow::anyhow!("variant quick row: {e}"))?;
    Ok(())
}

// ---- the preview's samples ----

/// The settings editor's offer for Superbounce's marked settings, as a
/// list load leaves it: respawn time, starting weapon, shields.
fn sample_offer() -> HashMap<u32, Vec<i32>> {
    HashMap::from([
        (17, vec![0, 3, 5, 10]),
        (89, vec![3, 4, 9]),
        (19, vec![0, 1, 2]),
    ])
}

/// Superbounce's record and script, with `marks` of its five marks.
fn sample_dialog(marks: usize) -> Arc<Dialog> {
    let mut ir = ::rules::compile(::rules::builtin::SUPERBOUNCE).expect("Superbounce compiles");
    let record = variants::portable::record_for(&ir, &[]).expect("Superbounce's record");
    ir.quick.truncate(marks);
    let d = Arc::new(Dialog::new(Arc::default()));
    d.open(record, Arc::new(ir), Some(sample_offer()));
    d
}

/// Siege's record and script: its five marks and its two options more.
fn siege_dialog() -> Arc<Dialog> {
    let ir = ::rules::compile(::rules::builtin::SIEGE).expect("Siege compiles");
    let record = variants::portable::record_for(&ir, &[]).expect("Siege's record");
    let d = Arc::new(Dialog::new(Arc::default()));
    d.open(record, Arc::new(ir), Some(HashMap::from([(17, vec![0, 3, 5, 10])])));
    d
}

/// The page over `dialog`, as a list page.
fn sample_list(dialog: &Arc<Dialog>) -> crate::ui_kit::page::ListPage {
    let s = Arc::new(page(dialog));
    crate::ui_kit::page::ListPage::new(s.class(), crate::ui_kit::page::described(&s))
}

/// The page as Superbounce opens it: five rows, three settings and two
/// options (`crate::ui_kit::screens::list_page`).
pub fn sample_page() -> crate::ui_kit::page::ListPage {
    sample_list(&sample_dialog(ROWS))
}

/// Every state the preview draws: Superbounce's five rows focused on a
/// setting (what it sets), on an option (its description) and on the last;
/// a script with three marks (the slots after them not drawn); a refused
/// change, said in the row's help; and Siege's whole options page, its
/// marks and the two options after them, focused on the first and the last.
pub fn samples() -> Vec<(&'static str, crate::ui_kit::page::ListPage, usize)> {
    let refused = sample_dialog(ROWS);
    refused.choose(0, 7);
    vec![
        ("focus-0", sample_page(), 0),
        ("focus-3", sample_page(), 3),
        ("focus-4", sample_page(), 4),
        ("three-rows", sample_list(&sample_dialog(3)), 2),
        ("refused", sample_list(&refused), 0),
        ("siege", sample_list(&siege_dialog()), 0),
        ("siege-rounds", sample_list(&siege_dialog()), 6),
    ]
}

/// The page's states for `h2 ui preview variant-quick` and the checks
/// (`crate::ui_kit::screens::live`).
pub struct Samples {
    states: Vec<(&'static str, crate::ui_kit::page::ListPage, usize)>,
    at: usize,
    focus: crate::ui_kit::page::Focus,
}

impl Samples {
    pub fn new() -> Self {
        let states = samples();
        let focus = crate::ui_kit::page::Focus(states[0].2);
        Samples {
            states,
            at: 0,
            focus,
        }
    }
}

impl Default for Samples {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::ui_kit::page::Live for Samples {
    fn press(&mut self, input: crate::ui_kit::input::Input) -> String {
        use crate::ui_kit::page::Page;
        format!("{:?}", self.states[self.at].1.step(&mut self.focus, input))
    }
    fn audit_press(&mut self, input: crate::ui_kit::input::Input) -> Option<bool> {
        use crate::ui_kit::page::{ListPage, Page};
        Some(ListPage::handled(
            &self.states[self.at].1.step(&mut self.focus, input),
        ))
    }
    fn spec(&self) -> crate::ui_kit::spec::ScreenSpec {
        use crate::ui_kit::page::Page;
        self.states[self.at].1.spec(&self.focus)
    }
    fn summary(&self) -> String {
        crate::ui_kit::page::focus_line(&self.spec())
    }
    fn state_names(&self) -> Vec<String> {
        self.states.iter().map(|(n, _, _)| n.to_string()).collect()
    }
    fn specs(&self) -> Vec<(String, crate::ui_kit::spec::ScreenSpec)> {
        use crate::ui_kit::page::{Focus, Page};
        self.states
            .iter()
            .map(|(n, p, f)| (n.to_string(), p.spec(&Focus(*f))))
            .collect()
    }
    fn go(&mut self, name: &str) -> bool {
        let Some(i) = self.states.iter().position(|(n, _, _)| *n == name) else {
            return false;
        };
        self.at = i;
        self.focus = crate::ui_kit::page::Focus(self.states[i].2);
        true
    }
}

#[cfg(test)]
mod tests;
