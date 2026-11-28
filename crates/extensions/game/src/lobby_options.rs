//! Extra lines in the custom game's pregame information panel
//! (docs/game/ui-builder.md "The pregame lobby's Quick Options").
//!
//! The lobby update calls LOBBY_VARIANT_PANEL on the custom game pane.
//! Its formatter (0x25092c) fills text widgets 6 through 10, one per
//! option. Keep all lines in those five slots. The next text widget
//! sits below the Quick Options box, next to the footer.
//!
//! For a variant whose record names a rules script, the script's Quick
//! Options marks (`crate::variant_quick`, switch `variant-quick`) take the
//! rows in place of the formatter's, "Label: value" in the script's order
//! (docs/game/rules-variants.md, "Quick Options in the lobby"). A record
//! with no script keeps the formatter's rows.
//!
//! The lobby's mode (`crate::lobby_mode`) puts its lines first (the Forge
//! map; Firefight's wave set and teams, difficulty and starting lives). The party
//! lobby's header (`crate::lobby_tabs::header::panel`) has its own hook on
//! the same point, inside this one, so the header names the mode with
//! this feature off too.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use guest_layout::GuestMemory;
use guest_layout::entry::{Cx, EntryFuture, GuestCall, Ret};
use guest_layout::points;
use hooks::{BuildError, Builder, Flow, Manifest};
use settings::{Choice, Kind, Store};

use crate::clankers::{self, Want};
use crate::h1_options;
use crate::h1_options_row::LOBBY_MAP;
use crate::layout_glue::ReadGm;
use crate::map_select::{self, Category};
use crate::skulls_mp::{self, Role};
use crate::ui_kit::ids;

pub(crate) const SCREEN: u32 = 14;
const CUSTOM_PANE: u32 = 0x142c;
const FIRST_OPTION_TEXT: usize = 6;
const FIRST_OPTION_BLOCK: usize = 4;
const PANEL_ROWS: usize = 5;
/// The Quick Options content column ends before the footer, above the
/// sixth text block. Keep this limit separate from the screen safe area.
const PANEL: tags::ui::Rect = tags::ui::Rect {
    top: -75,
    left: -267,
    bottom: -173,
    right: 50,
};
const WIDGET_CHILD: u32 = 0x14;
const WIDGET_NEXT: u32 = 0x18;
const WIDGET_KIND: u32 = 4;
const WIDGET_INDEX: u32 = 0xa;
const WIDGET_VISIBLE: u32 = 0x6e;
const TEXT_KIND: u32 = 6;
const TEXT_VTABLE: u32 = 0x0045_8940;
// The text object's getter at 0x22f57f returns widget + 0x74, and
// 0x22f52e returns its inline UTF-16 buffer at that object's +0x44.
const TEXT_BUFFER: u32 = 0x74 + 0x44;
const TEXT_UNITS: usize = 255;

const IDS: ids::Family = ids::family("lobby options");
const MAX_CLANKERS: usize = 15;
const HALO1_ID: u32 = IDS.id(75);
const MAX_SKULLS: usize = live_proto::skulls::MULTIPLAYER.len();

fn difficulties() -> &'static [Choice] {
    let Kind::Choice(choices) = clankers::DIFFICULTY.kind else {
        unreachable!("Clankers' difficulty is a choice");
    };
    choices
}

fn clankers_id(count: usize, difficulty: usize) -> u32 {
    IDS.id(((count - 1) * difficulties().len() + difficulty) as u32)
}

pub(crate) fn strings() -> Vec<(u32, String)> {
    let mut strings = Vec::new();
    for count in 1..=MAX_CLANKERS {
        for (i, choice) in difficulties().iter().enumerate() {
            strings.push((
                clankers_id(count, i),
                format!("Clankers: {count}, {}", choice.label),
            ));
        }
    }
    strings.push((HALO1_ID, "Halo 1 Options: On".into()));
    for count in 1..=MAX_SKULLS {
        strings.push((HALO1_ID + count as u32, format!("Skulls: {count}")));
    }
    strings
}

fn option_ids(bots: Want, halo1: bool, skulls: usize) -> Vec<u32> {
    let mut ids = Vec::with_capacity(3);
    if bots.count > 0 {
        let difficulty = difficulties()
            .iter()
            .position(|d| d.value == bots.difficulty)
            .unwrap_or(1);
        ids.push(clankers_id(bots.count.min(MAX_CLANKERS), difficulty));
    }
    if halo1 {
        ids.push(HALO1_ID);
    }
    if skulls > 0 {
        ids.push(HALO1_ID + skulls.min(MAX_SKULLS) as u32);
    }
    ids
}

/// The panel's lines of ours: the lobby mode's first, then the options',
/// as many as the five slots hold.
fn panel(mode: Vec<u32>, options: Vec<u32>) -> Vec<u32> {
    let mut ids = mode;
    ids.extend(options);
    ids.truncate(PANEL_ROWS);
    ids
}

/// The first slot for our lines. Retain the native rows through the last
/// visible one when they fit, and otherwise take the bottom slots.
fn first_extra(retail_end: usize, extra: usize) -> usize {
    retail_end.min(PANEL_ROWS - extra)
}

/// What one of the panel's five slots shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    /// The formatter's text, as it left it.
    Retail,
    /// Nothing: the slot is hidden.
    Hidden,
    /// One of our lines.
    Ours(u32),
}

/// The panel's five slots. `retail`: which slots the formatter left
/// visible. `quick`: a script variant's lines (`variant_quick::panel_ids`),
/// which replace every formatter row, in order; `None` for a variant with
/// no script, whose formatter rows stay. `extra`: the lobby mode's and the
/// options' lines ([`panel`]), which then take the bottom by
/// [`first_extra`].
fn plan(retail: [bool; PANEL_ROWS], quick: Option<&[u32]>, extra: &[u32]) -> [Slot; PANEL_ROWS] {
    let mut slots = match quick {
        Some(lines) => {
            let mut slots = [Slot::Hidden; PANEL_ROWS];
            for (slot, &id) in slots.iter_mut().zip(lines) {
                *slot = Slot::Ours(id);
            }
            slots
        }
        None => retail.map(|shown| if shown { Slot::Retail } else { Slot::Hidden }),
    };
    let end = slots
        .iter()
        .rposition(|s| *s != Slot::Hidden)
        .map_or(0, |last| last + 1);
    let extra = &extra[..extra.len().min(PANEL_ROWS)];
    for (slot, &id) in slots[first_extra(end, extra.len())..].iter_mut().zip(extra) {
        *slot = Slot::Ours(id);
    }
    slots
}

fn halo1_active(map: Option<Category>, store: &Store) -> bool {
    map == Some(Category::Halo1)
        && h1_options::MULTIPLAYER
            .iter()
            .any(|row| h1_options::is_halo1(store, row))
}

fn visible(m: &dyn GuestMemory, widget: u32) -> bool {
    m.u8(widget.wrapping_add(WIDGET_VISIBLE)).is_ok_and(|v| v != 0)
}

/// The content box for the five option texts and the spare widget below
/// them. Check the spare too so a future write beyond the box is reported.
pub(crate) fn container(
    m: &dyn GuestMemory,
    screen: u32,
    widget: u32,
) -> Option<(&'static str, tags::ui::Rect)> {
    if m.u16(screen.checked_add(CUSTOM_PANE)?).ok()? != 0
        || m.u32(widget.checked_add(0x10)?).ok()? != screen
        || m.u32(widget.checked_add(WIDGET_KIND)?).ok()? != TEXT_KIND
        || m.u32(widget).ok()? != TEXT_VTABLE
    {
        return None;
    }
    let index = usize::from(m.u16(widget.checked_add(WIDGET_INDEX)?).ok()?);
    (FIRST_OPTION_BLOCK..=FIRST_OPTION_BLOCK + PANEL_ROWS)
        .contains(&index)
        .then_some(("Quick Options", PANEL))
}

/// Match the direct-child text indexes used by retail's widget lookup
/// (0x22edb8). The first two texts are the built-in header and footer,
/// so these indexes correspond to text blocks 4 through 8 in pane 0.
fn widgets(m: &dyn GuestMemory, screen: u32) -> Option<[u32; PANEL_ROWS]> {
    if m.u16(screen.checked_add(CUSTOM_PANE)?).ok()? != 0 {
        return None;
    }
    let mut slots = [0; PANEL_ROWS];
    let mut child = m.u32(screen.checked_add(WIDGET_CHILD)?).ok()?;
    let mut text = 0usize;
    for _ in 0..128 {
        if child == 0 {
            break;
        }
        if m.u32(child.checked_add(WIDGET_KIND)?).ok()? == TEXT_KIND {
            if let Some(slot) = text.checked_sub(FIRST_OPTION_TEXT) {
                if m.u32(child).ok()? != TEXT_VTABLE
                    || m.u16(child.checked_add(WIDGET_INDEX)?).ok()? as usize
                        != FIRST_OPTION_BLOCK + slot
                {
                    return None;
                }
                slots[slot] = child;
                if slot + 1 == PANEL_ROWS {
                    return Some(slots);
                }
            }
            text += 1;
        }
        child = m.u32(child.checked_add(WIDGET_NEXT)?).ok()?;
    }
    None
}

pub(crate) fn inspect(m: &dyn GuestMemory) -> Option<Vec<String>> {
    let top = crate::ui_watch::read(&ReadGm(m)).top?;
    if top.id != SCREEN {
        return None;
    }
    let slots = widgets(m, top.screen)?;
    let mut lines = Vec::new();
    for widget in slots.into_iter().filter(|&widget| visible(m, widget)) {
        let line = m.utf16(widget.checked_add(TEXT_BUFFER)?, TEXT_UNITS).ok()?;
        if !line.is_empty() {
            lines.push(line);
        }
    }
    Some(lines)
}

#[derive(Clone)]
pub struct LobbyOptions {
    store: Arc<Store>,
    live: Option<Arc<live_hle::Hle>>,
    bots: bool,
    passing: Arc<AtomicBool>,
}

impl LobbyOptions {
    pub fn new(store: Arc<Store>, live: Option<Arc<live_hle::Hle>>, bots: bool) -> Self {
        Self {
            store,
            live,
            bots,
            passing: Arc::default(),
        }
    }

    fn extra_options(&self, cx: &Cx) -> Vec<u32> {
        let mp = skulls_mp::shared();
        let (map, role, own_skulls) = cx.read(|m| {
            let mem = ReadGm(m);
            let map = m.u32(LOBBY_MAP).ok();
            let role = skulls_mp::role_in(&mem);
            let own_skulls = mp
                .filter(|_| !matches!(role, Role::Guest { .. }))
                .map_or(0, |mp| {
                    let (profiles, _) = crate::skulls::profiles_or_default(&mem);
                    mp.chosen_for(&profiles).len()
                });
            (map, role, own_skulls)
        });
        let bots = if self.bots && !matches!(role, Role::Guest { .. }) {
            Want::read(&self.store)
        } else {
            Want::OFF
        };
        let halo1 = halo1_active(map.and_then(map_select::selected_category), &self.store);
        let skulls = if mp.is_some() && matches!(role, Role::Guest { class: 2 }) {
            self.live
                .as_ref()
                .and_then(|live| live.cached_host_skulls())
                .map_or(0, |skulls| {
                    live_proto::skulls::MULTIPLAYER
                        .iter()
                        .filter(|skull| skulls.contains(skull))
                        .count()
                })
        } else {
            own_skulls
        };
        option_ids(bots, halo1, skulls)
    }

    async fn update(
        &self,
        cx: Cx,
        screen: u32,
        variant: u32,
        slots: [u32; PANEL_ROWS],
    ) -> Result<Ret, guest_layout::entry::EntryError> {
        // Retail marks each populated row visible again. Clearing first
        // removes the previous summary even if a new variant skips a row.
        cx.write(|m| {
            slots
                .iter()
                .try_for_each(|&widget| m.set_u8(widget + WIDGET_VISIBLE, 0))
        })?;
        self.passing.store(true, Ordering::SeqCst);
        let retail = cx
            .call(
                GuestCall::new(points::LOBBY_VARIANT_PANEL.base)
                    .eax(screen)
                    .args([variant]),
            )
            .await;
        self.passing.store(false, Ordering::SeqCst);
        let retail = retail?;
        if variant == 0 {
            return Ok(Ret::new(retail.eax, 4));
        }

        let quick = cx.write(|m| crate::variant_quick::panel_ids(m, variant))?;
        let mode = cx.write(|m| crate::lobby_mode::panel_ids(m, &self.store))?;
        let ids = panel(mode, self.extra_options(&cx));
        if ids.is_empty() && quick.is_none() {
            return Ok(Ret::new(retail.eax, 4));
        }
        let shown = cx.read(|m| slots.map(|widget| visible(m, widget)));
        for (&widget, slot) in slots.iter().zip(plan(shown, quick.as_deref(), &ids)) {
            match slot {
                Slot::Retail => {}
                Slot::Hidden => cx.write(|m| m.set_u8(widget + WIDGET_VISIBLE, 0))?,
                Slot::Ours(id) => {
                    cx.call(GuestCall::new(points::TEXT_SET.base).edi(widget).args([id]))
                        .await?;
                    cx.write(|m| m.set_u8(widget + WIDGET_VISIBLE, 1))?;
                }
            }
        }
        Ok(Ret::new(retail.eax, 4))
    }
}

/// The feature's id (`lobby-options=off` switches it).
pub const FEATURE: &str = "lobby-options";

impl hooks::Feature for LobbyOptions {
    fn manifest(&self) -> Manifest {
        Manifest::new(FEATURE, "Extra options in the custom game lobby")
            .doc("docs/game/ui-builder.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !b.switches().feature("ui-strings", true) {
            return Ok(());
        }
        // A script variant's own rows (docs/game/rules-variants.md,
        // "Quick Options in the lobby"); off, its formatter rows stay.
        if b.switches().feature(crate::variant_quick::FEATURE, true) {
            crate::variant_quick::build(b)?;
        }
        let this = Arc::new(self.clone());
        b.raw().before(&points::LOBBY_VARIANT_PANEL, "lobby option lines", move |c| {
            if this.passing.load(Ordering::SeqCst) {
                return Ok(Flow::Continue);
            }
            let screen = c.cpu.eax;
            let Some(variant) = c.arg_u32("variant") else {
                return Ok(Flow::Continue);
            };
            let slots = widgets(&*c.ctx.runtime().memory(), screen);
            let Some(slots) = slots else {
                return Ok(Flow::Continue);
            };
            let this = Arc::clone(&this);
            c.answer_async(move |cx| -> EntryFuture {
                Box::pin(async move { this.update(cx, screen, variant, slots).await })
            })
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    fn texts(ids: &[u32]) -> Vec<String> {
        let table = strings();
        ids.iter()
            .map(|&id| table.iter().find(|(key, _)| *key == id).unwrap().1.clone())
            .collect()
    }

    #[test]
    fn only_enabled_options_get_lines_in_panel_order() {
        let bots = Want {
            count: 4,
            ..Want::OFF
        };
        assert!(option_ids(Want::OFF, false, 0).is_empty());
        assert_eq!(texts(&option_ids(bots, false, 0)), ["Clankers: 4, Normal"]);
        assert_eq!(texts(&option_ids(Want::OFF, true, 0)), ["Halo 1 Options: On"]);
        assert_eq!(texts(&option_ids(Want::OFF, false, 1)), ["Skulls: 1"]);
        assert_eq!(
            texts(&option_ids(bots, true, MAX_SKULLS)),
            ["Clankers: 4, Normal", "Halo 1 Options: On", "Skulls: 3"]
        );
        assert_eq!(
            texts(&option_ids(
                Want {
                    count: 15,
                    difficulty: clankers::MIXED,
                    ..Want::OFF
                },
                false,
                0,
            )),
            ["Clankers: 15, Mixed"]
        );
    }

    #[test]
    fn halo1_summary_uses_either_multiplayer_choice_on_a_converted_map() {
        use crate::setting_defs::{HALO1, HALO2};
        use settings::Value;

        let defs = [h1_options::SANDBOX.def, h1_options::HUD.def];
        let store = Store::defaults(&defs);
        assert!(!halo1_active(Some(Category::Halo2), &store));
        assert!(!halo1_active(None, &store));

        for def in defs {
            store.set(def.key, Value::Int(HALO2)).unwrap();
        }
        assert!(!halo1_active(Some(Category::Halo1), &store));
        for def in defs {
            store.set(def.key, Value::Int(HALO1)).unwrap();
            assert!(halo1_active(Some(Category::Halo1), &store), "{}", def.key);
            store.set(def.key, Value::Int(HALO2)).unwrap();
        }
    }

    /// The lobby mode's lines come first and, with the options' lines, never
    /// pass the five slots; every line is the panel's own form and fits its
    /// width.
    #[test]
    fn mode_lines_come_first_and_fit_the_panel() {
        use crate::lobby_mode::{MAX_PANEL_LINES, panel_lines};
        use crate::lobby_start::Mode;
        let mode: Vec<u32> = (0..MAX_PANEL_LINES as u32).map(|i| 0x100 + i).collect();
        let options = option_ids(
            Want {
                count: 4,
                ..Want::OFF
            },
            true,
            MAX_SKULLS,
        );
        let all = panel(mode.clone(), options.clone());
        assert_eq!(all.len(), PANEL_ROWS);
        assert_eq!(all[..mode.len()], mode[..], "the mode's lines first");
        assert_eq!(panel(Vec::new(), options.clone()), options, "Multiplayer: as before");
        let ff = crate::firefight::options::Options {
            lives: Some(30),
            wave_set: 2,
            teams: Some(2),
            difficulty: ::firefight::Difficulty::Legendary,
            ..Default::default()
        };
        let lines = [
            panel_lines(Mode::Forge, None, None),
            panel_lines(Mode::Forge, Some("Castle Keep"), None),
            panel_lines(Mode::Firefight, None, Some(Ok(ff))),
        ];
        // Twelve units a character is a generous bound for the panel's
        // font; the box is 317 units wide.
        let width = (PANEL.right - PANEL.left) as usize;
        for line in lines.iter().flatten() {
            assert!(line.chars().count() * 12 <= width, "{line:?} is too wide");
            assert!(!line.contains(':') && !line.contains("  "), "{line:?}");
        }
        for start in 0..=PANEL_ROWS {
            assert!(first_extra(start, all.len()) + all.len() <= PANEL_ROWS);
        }
    }

    #[test]
    fn option_lines_replace_bottom_rows_when_the_box_is_full() {
        let starts: Vec<_> = (0..=3).map(|extra| first_extra(PANEL_ROWS, extra)).collect();
        assert_eq!(starts, [5, 4, 3, 2]);
        for retail in 0..=PANEL_ROWS {
            for extra in 0..=3 {
                let start = first_extra(retail, extra);
                assert!(start + extra <= PANEL_ROWS);
                assert_eq!(retail - start, (retail + extra).saturating_sub(PANEL_ROWS));
            }
        }
    }

    /// The formatter left its first `n` rows visible.
    fn retail(n: usize) -> [bool; PANEL_ROWS] {
        std::array::from_fn(|i| i < n)
    }

    /// A script variant's marked rows take the slots from the top, in the
    /// script's order, however many rows the formatter wrote; slots past
    /// the last mark are hidden. The lines are the record's own.
    #[test]
    fn marked_rows_replace_the_formatters_in_order() {
        use Slot::{Hidden, Ours};
        let quick = [0x10, 0x11, 0x12];
        for n in 0..=PANEL_ROWS {
            assert_eq!(
                plan(retail(n), Some(&quick), &[]),
                [Ours(0x10), Ours(0x11), Ours(0x12), Hidden, Hidden],
                "{n} retail rows"
            );
        }
        let five = [1, 2, 3, 4, 5];
        assert_eq!(plan(retail(2), Some(&five), &[]), five.map(Ours));

        let ir = ::rules::compile(::rules::builtin::SUPERBOUNCE).unwrap();
        let record = variants::portable::record_for(&ir, &[]).unwrap();
        assert_eq!(record.game_type(), variants::GameType::Script);
        let lines = crate::variant_quick::lines(&record).unwrap();
        assert_eq!(lines.len(), ir.quick.len());
        assert_eq!(lines[0], "Respawn Time: 3 seconds");
        assert_eq!(lines[3], "Bounce points: 1");
    }

    /// Siege's Quick Options are its five marks, read off its record on
    /// Slayer's index (where the lobby builds the panel), and its rows
    /// take the whole panel.
    #[test]
    fn siege_has_its_quick_options() {
        let ir = ::rules::compile(::rules::builtin::SIEGE).unwrap();
        let record = variants::portable::record_for(&ir, &[]).unwrap();
        assert_eq!(record.game_type(), variants::GameType::Slayer);
        let lines = crate::variant_quick::lines(&record).unwrap();
        assert_eq!(lines.len(), 5, "{lines:?}");
        assert!(lines.iter().all(|l| !l.is_empty()), "{lines:?}");
        let ids: Vec<u32> = (0..5).collect();
        assert_eq!(plan(retail(PANEL_ROWS), Some(&ids), &[]), [0, 1, 2, 3, 4].map(Slot::Ours));
    }

    /// The lobby mode's and the options' lines take the bottom of a script
    /// variant's rows by the same rule as of the formatter's.
    #[test]
    fn mode_and_option_lines_still_take_the_bottom() {
        use Slot::{Hidden, Ours};
        let all = retail(PANEL_ROWS);
        assert_eq!(
            plan(all, Some(&[1, 2, 3]), &[9]),
            [Ours(1), Ours(2), Ours(3), Ours(9), Hidden]
        );
        assert_eq!(
            plan(all, Some(&[1, 2, 3, 4, 5]), &[8, 9]),
            [Ours(1), Ours(2), Ours(3), Ours(8), Ours(9)]
        );
        assert_eq!(
            plan(all, Some(&[1, 2, 3, 4]), &[7, 8, 9]),
            [Ours(1), Ours(2), Ours(7), Ours(8), Ours(9)]
        );
        assert_eq!(plan(all, Some(&[]), &[8, 9]), [Ours(8), Ours(9), Hidden, Hidden, Hidden]);
        // The same rule over the formatter's rows.
        for n in 0..=PANEL_ROWS {
            for extra in 0..=3 {
                let ids: Vec<u32> = (0..extra).map(|i| 0x20 + i).collect();
                let from_retail = plan(retail(n), None, &ids);
                let from_quick: Vec<u32> = (0..n as u32).collect();
                let from_marks = plan(retail(0), Some(&from_quick), &ids);
                let start = first_extra(n, ids.len());
                for (i, (r, q)) in from_retail.iter().zip(from_marks).enumerate() {
                    let ours = (start..start + ids.len())
                        .contains(&i)
                        .then(|| Ours(ids[i - start]));
                    assert_eq!(*r, ours.unwrap_or(if i < n { Slot::Retail } else { Hidden }));
                    assert_eq!(q, ours.unwrap_or(if i < n { Ours(i as u32) } else { Hidden }));
                }
            }
        }
    }

    /// A record that names no script, or a script this machine lacks, has
    /// no lines of `variant_quick`; the formatter's rows stay as before.
    #[test]
    fn a_variant_with_no_script_keeps_retails_rows() {
        use Slot::{Hidden, Ours, Retail};
        use variants::ext::{Ext, Mode, Script};
        use variants::record::{GameType, GameVariant};

        let mut record = GameVariant::stock(GameType::Slayer).unwrap();
        assert_eq!(crate::variant_quick::lines(&record), None);
        record
            .set_ext(&Ext {
                script: Script { mode: Mode::Rules, prefix: [9; 8], options: 0 },
                ..Ext::default()
            })
            .unwrap();
        assert_eq!(crate::variant_quick::lines(&record), None);

        assert_eq!(plan(retail(4), None, &[]), [Retail, Retail, Retail, Retail, Hidden]);
        assert_eq!(plan(retail(4), None, &[9]), [Retail, Retail, Retail, Retail, Ours(9)]);
        assert_eq!(plan(retail(5), None, &[9]), [Retail, Retail, Retail, Retail, Ours(9)]);
    }

    /// A script with no `quick` line has no rows: the formatter's are
    /// hidden and none take their place; nothing is picked for it.
    #[test]
    fn a_script_with_no_marks_shows_none() {
        use Slot::{Hidden, Ours};
        let ir = ::rules::compile(::rules::builtin::SLAYER).unwrap();
        assert!(ir.quick.is_empty());
        let record = variants::portable::record_for(&ir, &[]).unwrap();
        assert_eq!(crate::variant_quick::lines(&record), Some(Vec::new()));
        assert!(crate::variant_quick::rows(&record, &ir).is_empty());
        assert_eq!(plan(retail(PANEL_ROWS), Some(&[]), &[]), [Hidden; PANEL_ROWS]);
        assert_eq!(
            plan(retail(PANEL_ROWS), Some(&[]), &[9]),
            [Ours(9), Hidden, Hidden, Hidden, Hidden]
        );
    }

    #[test]
    fn agent_reads_only_visible_option_widgets_of_the_custom_pane() {
        let mut m = FakeGuest::new();
        let (screen, first) = (0x1000, 0x3000);
        m.map(0x0054_d000, 0x1000)
            .map(screen, 0x2000)
            .map(first, 0x3000);
        let channel = crate::ui_kit::lifecycle::window_object(5, crate::ui_watch::SHARED).unwrap();
        m.put_u32(channel + 8, screen)
            .put_u32(channel + 0x34, screen)
            .put_u32(screen + 0x70, SCREEN)
            .put_u32(screen + WIDGET_CHILD, first);
        m.set_u16(screen + 8, 1).unwrap();

        let widget = |i: usize| first + 0x300 * i as u32;
        for i in 0..FIRST_OPTION_TEXT + PANEL_ROWS {
            let at = widget(i);
            m.put_u32(at, TEXT_VTABLE)
                .put_u32(at + WIDGET_KIND, TEXT_KIND)
                .put_u32(at + WIDGET_NEXT, widget(i + 1));
            if let Some(slot) = i.checked_sub(FIRST_OPTION_TEXT) {
                m.set_u16(at + WIDGET_INDEX, (FIRST_OPTION_BLOCK + slot) as u16)
                    .unwrap();
            }
        }
        for (i, shown, line) in [
            (0, 1, "Règle de jeu"),
            (1, 0, "Stale value"),
            (2, 1, ""),
            (4, 1, "Skulls: 1"),
        ] {
            let at = widget(FIRST_OPTION_TEXT + i);
            m.set_u8(at + WIDGET_VISIBLE, shown).unwrap();
            for (offset, unit) in line.encode_utf16().enumerate() {
                m.set_u16(at + TEXT_BUFFER + 2 * offset as u32, unit)
                    .unwrap();
            }
        }
        assert_eq!(inspect(&m).unwrap(), ["Règle de jeu", "Skulls: 1"]);
        m.set_u16(screen + CUSTOM_PANE, 1).unwrap();
        assert_eq!(inspect(&m), None);
        m.set_u16(screen + CUSTOM_PANE, 0).unwrap();
        m.put_u32(screen + 0x70, 9);
        assert_eq!(inspect(&m), None);
    }

    #[test]
    fn the_sixth_option_is_in_the_safe_area_but_outside_the_box() {
        use crate::ui_kit::layout::{Issue, SAFE, bounds_issues};

        let (screen, widget) = (0x1000, 0x3000);
        let mut m = FakeGuest::new();
        m.map(screen, 0x2000).map(widget, 0x1000);
        m.put_u32(widget, TEXT_VTABLE)
            .put_u32(widget + WIDGET_KIND, TEXT_KIND)
            .put_u32(widget + 0x10, screen);
        let bounds = |row: usize| {
            let top = PANEL.top - 19 * row as i16;
            tags::ui::Rect {
                top,
                bottom: top - 20,
                ..PANEL
            }
        };
        for row in 0..=PANEL_ROWS {
            m.set_u16(widget + WIDGET_INDEX, (FIRST_OPTION_BLOCK + row) as u16)
                .unwrap();
            let rect = bounds(row);
            let issues: Vec<_> =
                bounds_issues(|| "option".into(), rect, SAFE, container(&m, screen, widget)).collect();
            if row < PANEL_ROWS {
                assert!(issues.is_empty(), "row {row}: {issues:?}");
            } else {
                assert_eq!(
                    issues,
                    [Issue::OutsideContainer {
                        id: "option".into(),
                        container: "Quick Options".into(),
                    }]
                );
            }
        }
    }
}
