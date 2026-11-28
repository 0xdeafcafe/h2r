//! Screen 0xda keeps the retail checkpoint reader, asynchronous completion
//! and co-op player selection. Its four-byte list records hold an action
//! at +2; retail allocates three, and its widgets scroll over the array.
use std::sync::Arc;

use guest_layout::datum::DatumArray;
use guest_layout::entry::{EntryFuture, GuestCall, Ret};
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
use guest_layout::view::ui::{self, Screen};
use hooks::{Builder, Flow};

use super::{ID_BASE, LEVEL_SELECT_LOADER, Picker, State};
use crate::campaigns::{CampaignProgress, RETAIL_CAMPAIGN, SELECTED_CAMPAIGN, SELECTED_MAP};
use crate::ui_kit::ids;
use content::campaign_list::Built;

const IDS: ids::Family = ids::family("campaign actions");
pub(super) const STRINGS: [(u32, &str); 3] = [
    (IDS.id(0), "New Game"),
    (IDS.id(1), "Select Level"),
    (IDS.id(2), "Resume"),
];
pub(super) const RESTORE_CHECKPOINT: u32 = 0x0054_e7cc;
const DIFFICULTY: u32 = 0x0054_e7c8;
const DIFFICULTY_LOADER: u32 = 0x002b_b3ed;
const START_LOADER: u32 = 0x002b_1467;

const CHECKPOINT: u16 = 0;
const COOP: u16 = 2;
const COOP_FRESH: u16 = 4;
const NEW: u16 = 5;
const SELECT: u16 = 6;
const LAST_LEVEL: u16 = 7;

pub fn options_capacity(name: u32, n: u32) -> u32 {
    if name == 0x0045_af70 && n == 3 { 4 } else { n }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Resume {
    map: u32,
    difficulty: u16,
    checkpoint: bool,
}

fn resume(game: &Built, progress: &CampaignProgress) -> Option<Resume> {
    let level = match &progress.last_map {
        Some(name) => game.levels.iter().find(|l| l.map == *name),
        None => game
            .levels
            .iter()
            .find(|l| i64::from(l.map_id) == progress.last),
    }?;
    Some(Resume {
        map: level.map_id,
        difficulty: progress.difficulty.unwrap_or(1).min(3),
        checkpoint: progress.checkpoint,
    })
}

/// Retail accepts a solo checkpoint only when exactly one slot is signed
/// in (0x18fada). Other configurations can still restart a saved level.
fn profile_resume(s: &State, m: &dyn GuestMemory) -> Option<Resume> {
    use crate::skulls::{LOCAL_PLAYERS, SLOT_SIGNED_IN, SLOT_STRIDE, SLOTS};
    let slots = (0..LOCAL_PLAYERS)
        .filter(|i| m.u32(SLOTS + i * SLOT_STRIDE).unwrap_or(0) & SLOT_SIGNED_IN != 0);
    let mut slots = slots.peekable();
    let slot = slots.next();
    let profile = slot.and_then(|i| crate::skulls::slot_name(&crate::layout_glue::ReadGm(m), i));
    let mut saved = selected_resume(
        s,
        profile.as_deref().unwrap_or(crate::skulls::DEFAULT_PROFILE),
    )?;
    saved.checkpoint &= profile.is_some() && slots.peek().is_none();
    Some(saved)
}

fn selected_resume(s: &State, profile: &str) -> Option<Resume> {
    let game = s
        .built
        .iter()
        .find(|b| b.campaign_id == s.selection.campaign())?;
    resume(game, &s.progress.get(profile, &game.key)?)
}

fn actions(retail: bool, retail_checkpoint: bool, saved: Option<Resume>, coop: u16) -> Vec<u16> {
    let first = if retail {
        retail_checkpoint.then_some(CHECKPOINT)
    } else {
        saved.map(|s| if s.checkpoint { CHECKPOINT } else { LAST_LEVEL })
    };
    first.into_iter().chain([NEW, SELECT, coop]).collect()
}

/// Native actions (checkpoint and co-op) continue through 0x2b166d.
fn solo_route(
    action: u16,
    game: u32,
    built: &[Built],
    saved: Option<Resume>,
) -> Option<(u32, Option<u32>, Option<u16>)> {
    match action {
        SELECT => Some((LEVEL_SELECT_LOADER, None, None)),
        NEW => {
            let first = if game == RETAIL_CAMPAIGN {
                1
            } else {
                built
                    .iter()
                    .find(|g| g.campaign_id == game)?
                    .levels
                    .first()?
                    .map_id
            };
            Some((DIFFICULTY_LOADER, Some(first), None))
        }
        LAST_LEVEL => {
            let r = saved?;
            Some((START_LOADER, Some(r.map), Some(r.difficulty)))
        }
        _ => None,
    }
}

pub(super) fn action(m: &dyn GuestMemory, list: u32, row: i16) -> Result<Option<u16>, MemFault> {
    let array = DatumArray::at(m.u32(list + 0x70)?);
    array
        .get_absolute(m, i32::from(row))?
        .map(|at| m.u16(at + 2))
        .transpose()
}

pub(super) fn label(action: u16) -> Option<&'static str> {
    match action {
        CHECKPOINT | LAST_LEVEL => Some("resume"),
        NEW => Some("new_game"),
        SELECT => Some("select_level"),
        COOP | COOP_FRESH => Some("co_op"),
        _ => None,
    }
}

pub(super) fn checkpoint_selection(
    c: &mut hooks::Call,
    s: &State,
) -> Result<(), runtime::GuestFault> {
    let mut m = c.ctx.runtime().memory();
    if let Some(saved) = profile_resume(s, &*m) {
        m.set_u32(SELECTED_CAMPAIGN, s.selection.campaign())?;
        m.set_u32(SELECTED_MAP, saved.map)?;
    }
    Ok(())
}

/// Called before the list is handed to its screen. Retail has just
/// populated consecutive rows, so keep their salts, adding at most two.
fn fill(m: &mut dyn GuestMemory, list: u32, rows: &[u16]) -> Result<(), MemFault> {
    let array = DatumArray::at(m.u32(list + 0x70)?);
    if array.header_field(m, guest_layout::datum::layout::MAXIMUM_COUNT)? < rows.len() as i32 {
        return Ok(());
    }
    for (i, &action) in rows.iter().enumerate() {
        let i = i as i32;
        let at = match array.get_absolute(m, i)? {
            Some(at) => at,
            None => {
                if array.new_datum_at_absolute(m, i)?.is_none() {
                    break;
                }
                array.element_addr(m, i)?
            }
        };
        m.set_u16(at + 2, action)?;
    }
    Ok(())
}

pub(super) fn hooks(b: &mut Builder, state: &Arc<State>) {
    let s = Arc::clone(state);
    b.raw().after(
        &points::CAMPAIGN_OPTIONS_LIST,
        "actions for the selected game",
        move |c| {
            let mut m = c.ctx.runtime().memory();
            let list = c.cpu.eax;
            let retail = s.selection.campaign() == RETAIL_CAMPAIGN;
            let native_checkpoint = action(&*m, list, 0) == Ok(Some(CHECKPOINT));
            let legacy = native_checkpoint && crate::campaigns::legacy_checkpoint::recover(&s, &*m);
            let saved = profile_resume(&s, &*m);
            let coop = if retail {
                action(&*m, list, if native_checkpoint { 2 } else { 1 })
                    .ok()
                    .flatten()
                    .unwrap_or(COOP_FRESH)
            } else if saved.is_some() {
                COOP
            } else {
                COOP_FRESH
            };
            let rows = actions(retail, native_checkpoint && !legacy, saved, coop);
            if let Err(e) = fill(&mut *m, list, &rows) {
                eprintln!("campaigns: could not populate Campaign Options: {e:?}");
            }
            Ok(())
        },
    );

    // Slot 17 (0x24bb60) uses slot 19's result to walk the embedded
    // widgets. Only the separate datum array grows for the Resume row.
    b.raw().after(
        &points::CAMPAIGN_OPTION_ITEM_COUNT,
        "limit linked items to the constructed widgets",
        |c| {
            c.cpu.eax = (c.cpu.eax as i32).min(ui::CAMPAIGN_OPTIONS_ITEMS as i32) as u32;
            Ok(())
        },
    );

    let s = Arc::clone(state);
    let picker = Picker::new(&s.built);
    b.raw().before(
        &points::CAMPAIGN_OPTION_TEXT,
        "game title and action labels",
        move |c| {
            let item = c.arg_u32("item").unwrap_or(0);
            let id = {
                let m = c.ctx.runtime().memory();
                crate::speaker_row::item_row(&*m, item)
                    .and_then(|r| action(&*m, c.cpu.ecx, r).ok().flatten())
            };
            // Use retail's localized labels for the two native actions.
            let label = match id {
                Some(NEW) => IDS.id(0),
                Some(SELECT) => IDS.id(1),
                Some(LAST_LEVEL) => IDS.id(2),
                Some(CHECKPOINT) => 0x1400_0425,
                Some(COOP) => 0x1500_0427,
                Some(COOP_FRESH) => 0x0400_0423,
                _ => return Ok(Flow::Continue),
            };
            let Some(row) = picker
                .rows
                .iter()
                .position(|r| r.0 == s.selection.campaign())
            else {
                return Ok(Flow::Continue);
            };
            let screen = Screen::at(c.cpu.ecx - ui::LIST);
            c.answer_async(move |cx| -> EntryFuture {
                Box::pin(async move {
                    cx.call(
                        GuestCall::new(points::TEXT_SET.base)
                            .edi(screen.header())
                            .esi(screen.addr)
                            .args([ID_BASE + 1 + row as u32]),
                    )
                    .await?;
                    crate::list_class::draw_texts(&cx, item, vec![(0, label)]).await?;
                    Ok(Ret::new(1, 8))
                })
            })
        },
    );

    // 0x2b18df is the only caller. The native description widget chooses
    // its localized hint by action code, independently of the A handler.
    b.raw().after(
        &points::CAMPAIGN_OPTION_HELP,
        "hint for the focused action",
        |c| {
            c.cpu.eax = match c.cpu.eax as u16 {
                NEW => 1,
                SELECT => 3,
                LAST_LEVEL => u32::from(CHECKPOINT),
                _ => return Ok(()),
            };
            Ok(())
        },
    );

    let s = Arc::clone(state);
    b.raw().before(
        &points::CAMPAIGN_OPTION_CHOSEN,
        "open the selected game's action",
        move |c| {
            let game = s.selection.campaign();
            let (loader, map) = {
                let mut m = c.ctx.runtime().memory();
                let Some(choice) = crate::layout_glue::chosen_row(&*m, c.cpu.esp)
                    .and_then(|r| action(&*m, c.cpu.ecx, r).ok().flatten())
                else {
                    return Ok(Flow::Continue);
                };
                let saved = (choice == LAST_LEVEL)
                    .then(|| profile_resume(&s, &*m))
                    .flatten();
                let Some((loader, map, difficulty)) = solo_route(choice, game, &s.built, saved)
                else {
                    return Ok(Flow::Continue);
                };
                if let Some(map) = map {
                    m.set_u32(SELECTED_CAMPAIGN, game)?;
                    m.set_u32(SELECTED_MAP, map)?;
                }
                if let Some(d) = difficulty {
                    m.set_u32(DIFFICULTY, u32::from(d))?;
                    m.set_u8(RESTORE_CHECKPOINT, 0)?;
                }
                (loader, map)
            };
            c.answer_async(move |cx| -> EntryFuture {
                Box::pin(async move {
                    if let Some(map) = map {
                        let record = cx
                            .call(
                                GuestCall::new(points::CAMPAIGN_LEVEL_LOOKUP.base)
                                    .args([game, map]),
                            )
                            .await?
                            .eax;
                        if record == 0 {
                            eprintln!(
                                "campaigns: no start record for campaign {game} map {map:#x}"
                            );
                            return Ok(Ret::new(0, 8));
                        }
                        // The level-select press preloads the scenario at 0x2c514d.
                        cx.call(GuestCall::new(0x0016_3890).args([record + 8, 0]))
                            .await?;
                    }
                    crate::list_class::open_retail(&cx, cx.arg(0)?, &[], loader).await?;
                    Ok(Ret::new(0, 8))
                })
            })
        },
    );

    let s = Arc::clone(state);
    b.raw().before(
        &points::PROFILE_LAST_MAP,
        "continue in this campaign",
        move |c| {
            let mut m = c.ctx.runtime().memory();
            let game = s.selection.campaign();
            if game == RETAIL_CAMPAIGN || !matches!(m.u32(c.cpu.esp)?, 0x0019_9aab | 0x002c_5004) {
                return Ok(Flow::Continue);
            }
            let map = profile_resume(&s, &*m).map(|r| r.map).or_else(|| {
                s.built
                    .iter()
                    .find(|b| b.campaign_id == game)?
                    .levels
                    .first()
                    .map(|l| l.map_id)
            });
            let Some(map) = map else {
                return Ok(Flow::Continue);
            };
            m.set_u32(SELECTED_CAMPAIGN, game)?;
            drop(m);
            c.cpu.eax = map;
            c.ret(0).map(Flow::Return)
        },
    );
    let s = Arc::clone(state);
    b.raw().before(
        &points::LOBBY_SET_MAP,
        "carry the map's campaign into co-op",
        move |c| {
            let mut mem = c.ctx.runtime().memory();
            if mem.u32(c.cpu.esp + 4)? == RETAIL_CAMPAIGN
                && let Some(game) = crate::campaigns::campaign_of(&s.built, c.cpu.esi)
            {
                mem.set_u32(c.cpu.esp + 4, game.campaign_id)?;
            }
            Ok(Flow::Continue)
        },
    );

    let s = Arc::clone(state);
    b.raw().before(
        &points::PROFILE_CHECKPOINT_SET,
        "keep checkpoint presence per game",
        move |c| {
            let Some(key) = s.key_of_campaign(s.selection.campaign()) else {
                return Ok(Flow::Continue);
            };
            let slot = c.arg_u32("slot").unwrap_or(u32::MAX);
            if let Some(name) = crate::skulls::slot_name(&*c.ctx.runtime().memory(), slot) {
                s.progress.checkpoint_saved(&name, key);
            }
            c.ret(4).map(Flow::Return)
        },
    );

    // A legacy header on a removable device can still arrive through the
    // retail filename. Preserve the native error path (0x2b182a) for it.
    let s = Arc::clone(state);
    b.raw().before(
        &points::CAMPAIGN_CHECKPOINT_POLL,
        "validate the checkpoint's game",
        move |c| {
            let list = c.cpu.ecx;
            let mut m = c.ctx.runtime().memory();
            if m.u8(list + 0x220)? != 0 && m.u8(list + 0x224)? != 0 && m.u8(list + 0x226)? != 0 {
                let owner = m.u32(list + 0x330 + 0x144)?;
                if s.selection.campaign() == RETAIL_CAMPAIGN && owner > 1 && owner != u32::MAX {
                    m.set_u8(list + 0x226, 0)?;
                }
            }
            Ok(Flow::Continue)
        },
    );

    // The retail reader/writer has finished constructing a 256-byte path.
    // The queue copies it into its job, before the UI can change games.
    let paths: Vec<_> = state
        .built
        .iter()
        .map(|game| {
            (
                game.campaign_id,
                crate::campaigns::legacy_checkpoint::filename(&game.key),
            )
        })
        .collect();
    let paths = Arc::new(paths);
    for (point, caller) in [
        (&points::CHECKPOINT_QUEUE_READ, 0x0021_7506),
        (&points::CHECKPOINT_QUEUE_WRITE, 0x0021_7576),
    ] {
        let s = Arc::clone(state);
        let paths = Arc::clone(&paths);
        b.raw()
            .before(point, "the selected game's checkpoint file", move |c| {
                let Some((_, name)) = paths.iter().find(|p| p.0 == s.selection.campaign()) else {
                    return Ok(Flow::Continue);
                };
                let mut m = c.ctx.runtime().memory();
                if m.u32(c.cpu.esp)? != caller {
                    return Ok(Flow::Continue);
                }
                let mut path = m.read::<256>(c.cpu.eax)?;
                if scope_checkpoint(&mut path, name) {
                    m.write(c.cpu.eax, path)?;
                    return Ok(Flow::Continue);
                }
                eprintln!("campaigns: invalid profile checkpoint path for {name}");
                drop(m);
                c.cpu.eax = 0;
                c.ret(20).map(Flow::Return)
            });
    }
}

fn scope_checkpoint(path: &mut [u8; 256], name: &str) -> bool {
    let Some(end) = path.iter().position(|&b| b == 0) else {
        return false;
    };
    let Some(dir) = path[..end].strip_suffix(b"auxilary.bin") else {
        return false;
    };
    let start = dir.len();
    let Some(dest) = path.get_mut(start..=start + name.len()) else {
        return false;
    };
    dest[..name.len()].copy_from_slice(name.as_bytes());
    dest[name.len()] = 0;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn new_game_opens_difficulty_and_resume_skips_it() {
        let s = super::super::tests::state();
        let game = &s.built[0];
        let route = |a, g, r| solo_route(a, g, &s.built, r);
        let saved = Resume {
            map: game.levels[1].map_id,
            difficulty: 3,
            checkpoint: false,
        };
        assert_eq!(
            route(SELECT, game.campaign_id, None),
            Some((LEVEL_SELECT_LOADER, None, None))
        );
        assert_eq!(
            route(NEW, game.campaign_id, Some(saved)),
            Some((DIFFICULTY_LOADER, Some(game.levels[0].map_id), None))
        );
        assert_eq!(
            route(NEW, RETAIL_CAMPAIGN, None),
            Some((DIFFICULTY_LOADER, Some(1), None))
        );
        assert_eq!(
            route(LAST_LEVEL, game.campaign_id, Some(saved)),
            Some((START_LOADER, Some(saved.map), Some(3)))
        );
        for native in [CHECKPOINT, COOP, COOP_FRESH] {
            assert_eq!(route(native, RETAIL_CAMPAIGN, None), None);
        }
    }

    #[test]
    fn resume_is_scoped_to_the_game_and_profile_across_launches() {
        let s = super::super::tests::state();
        let game = &s.built[0];
        let first = &game.levels[0];
        let second = &game.levels[1];
        s.selection.set(game.campaign_id);
        assert_eq!(
            actions(false, true, selected_resume(&s, "Chief"), COOP_FRESH),
            [NEW, SELECT, COOP_FRESH]
        );
        s.progress.launched("Chief", &game.key, second, 3, false);
        let saved = selected_resume(&s, "Chief").unwrap();
        assert_eq!(
            saved,
            Resume {
                map: second.map_id,
                difficulty: 3,
                checkpoint: false
            }
        );
        assert_eq!(actions(false, true, Some(saved), COOP)[0], LAST_LEVEL);
        s.progress.checkpoint_saved("Chief", &game.key);
        s.progress.launched("Chief", &game.key, second, 3, true);
        assert!(selected_resume(&s, "Chief").unwrap().checkpoint);
        assert_eq!(selected_resume(&s, "Arbiter"), None);
        s.selection.set(RETAIL_CAMPAIGN);
        assert_eq!(selected_resume(&s, "Chief"), None);
        assert_eq!(
            actions(true, false, Some(saved), COOP_FRESH),
            [NEW, SELECT, COOP_FRESH]
        );
        assert_eq!(
            actions(true, true, None, COOP),
            [CHECKPOINT, NEW, SELECT, COOP]
        );
        s.selection.set(game.campaign_id);
        assert_eq!(
            actions(false, false, selected_resume(&s, "Chief"), COOP)[0],
            CHECKPOINT
        );
        s.progress.launched("Chief", &game.key, first, 1, false);
        assert_eq!(
            selected_resume(&s, "Chief"),
            Some(Resume {
                map: first.map_id,
                difficulty: 1,
                checkpoint: false
            })
        );
    }

    #[test]
    fn old_progress_uses_normal_and_current_progress_follows_the_map_basename() {
        let s = super::super::tests::state();
        let mut game = s.built[0].clone();
        let id = game.levels[0].map_id;
        let legacy: CampaignProgress =
            toml::from_str(&format!("last = {id}\nfinished = [-1,-1,-1,-1]")).unwrap();
        assert_eq!(
            resume(&game, &legacy),
            Some(Resume {
                map: id,
                difficulty: 1,
                checkpoint: false
            })
        );
        let current = CampaignProgress {
            last_map: Some(game.levels[0].map.clone()),
            ..legacy
        };
        game.levels[0].map_id = 9999;
        assert_eq!(resume(&game, &current).unwrap().map, 9999);
        game.levels.remove(0);
        assert_eq!(resume(&game, &current), None);
    }

    #[test]
    fn expanding_the_retail_array_preserves_the_first_handle_and_exposes_four_actions() {
        let mut m = FakeGuest::new();
        m.map(0x1000, 0x2000).put_u32(0x1070, 0x2000);
        let array = DatumArray::at(0x2000);
        array
            .initialize_contiguous(&mut m, 0x1100, 4, 4, 2, 0)
            .unwrap();
        array.make_valid(&mut m).unwrap();
        let first = array.new_datum(&mut m).unwrap().unwrap();
        fill(&mut m, 0x1000, &[CHECKPOINT, NEW, SELECT, COOP]).unwrap();
        assert!(array.try_get(&m, first).unwrap().is_some());
        let rows: Vec<_> = (0..4)
            .map(|i| action(&m, 0x1000, i).unwrap().unwrap())
            .collect();
        assert_eq!(rows, [CHECKPOINT, NEW, SELECT, COOP]);
        assert_eq!(action(&m, 0x1000, 4).unwrap(), None);
    }

    #[test]
    fn a_halo2_checkpoint_links_only_widgets_and_keeps_the_fourth_action() {
        use guest_layout::datum::layout::ACTIVE_COUNT;
        use runtime::{CpuAbi, Dispatch, ExecCtx, Guest, Runtime, guest::Permissions};

        let list = ui::ListHandler::at(0x1000);
        let array = DatumArray::at(0x2000);
        let mut m = Guest::new();
        m.map_zeroed(list.addr, 0x2000, Permissions::RW).unwrap();
        let callback = 0x9000;
        m.map_zeroed(callback, 0x1000, Permissions::RX).unwrap();
        list.set_rows(&mut m, array.addr).unwrap();
        let press = list.press(0x208);
        press.set_target(&mut m, list.addr).unwrap();
        press.set_function(&mut m, callback).unwrap();
        array
            .initialize_contiguous(&mut m, 0x1400, 4, 4, 2, 0)
            .unwrap();
        array.make_valid(&mut m).unwrap();
        // The native constructor returns the handler with three saved-game actions.
        fill(&mut m, list.addr, &[CHECKPOINT, 1, COOP]).unwrap();
        let rt = Runtime::new(m, Dispatch::default());
        let mut ctx = ExecCtx::new(Arc::clone(&rt), 2);
        let mut registry = hooks::Registry::new(hooks::Switches::defaults());
        registry
            .add(&super::super::Campaigns {
                state: Arc::new(super::super::tests::state()),
                picker: Arc::new(|| 0),
            })
            .unwrap();
        let plan = registry.build(&hooks::Base).unwrap();
        let mut after = |point: &'static points::Point, eax| {
            let mut cpu = CpuAbi {
                ecx: list.addr,
                eax,
                ..Default::default()
            };
            if let Some(chain) = plan.chains.get(&point.base) {
                for hook in chain.after.iter().rev() {
                    if let hooks::Run::After(f) = &hook.run {
                        f(&mut hooks::Call {
                            cpu: &mut cpu,
                            ctx: &mut ctx,
                            addr: point.base,
                            point,
                        })
                        .unwrap();
                    }
                }
            }
            cpu.eax
        };
        assert_eq!(after(&points::CAMPAIGN_OPTION_ITEM_COUNT, 2), 2);
        after(&points::CAMPAIGN_OPTIONS_LIST, list.addr);
        // Slot 19's original body reads the active count. Slot 17 walks that
        // many items and propagates the player mask to each one's children.
        let rows = array.header_field(&*rt.memory(), ACTIVE_COUNT).unwrap();
        assert_eq!(rows, 4);
        let widgets = after(&points::CAMPAIGN_OPTION_ITEM_COUNT, rows as u32);
        let mut m = rt.memory();
        for i in 0..widgets {
            let item = list.item(i).addr;
            m.set_u16(item + 8, 1).unwrap();
            let child = m.u32(item + 0x14).unwrap();
            if child != 0 {
                m.set_u16(child + 8, 1)
                    .expect("press callback reached as a widget");
            }
        }
        assert_eq!(widgets, 3);
        assert_eq!(press.function(&*m), Ok(callback));
        assert_eq!(action(&*m, list.addr, 0), Ok(Some(CHECKPOINT)));
        assert_eq!(action(&*m, list.addr, 3), Ok(Some(COOP)));
    }

    #[test]
    fn checkpoint_reads_and_writes_keep_the_profile_directory() {
        let mut buf = [0u8; 256];
        let old = br"T:\profile\auxilary.bin";
        buf[..old.len()].copy_from_slice(old);
        assert!(scope_checkpoint(&mut buf, "cp-example.bin"));
        assert_eq!(
            &buf[..buf.iter().position(|&ch| ch == 0).unwrap()],
            br"T:\profile\cp-example.bin"
        );
        assert!(!scope_checkpoint(&mut buf, "other.bin"));
    }
}
