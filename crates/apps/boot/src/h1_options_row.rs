//! The "Halo 1 Options" row on a multiplayer custom game's Game Setup
//! dialog, next to its map and game type, while the lobby's map is a
//! converted Halo 1 map; and the screen it opens (`crate::h1_options`,
//! docs/halo1/multiplayer.md "Halo 1 options").
//!
//! **The row.** The Game Setup dialog (screen 23, `squad_settings_dialog`)
//! builds its list in `0x2bb4d2` (ESI the handler): a datum array "squad
//! setting list" at handler `+0x70`, each row's action word at datum `+2`.
//! Retail adds "Change Map" (action 0) only for the multiplayer custom game
//! lobbies (kinds 1, 3 and 5). An `after` hook on the constructor adds one
//! row with our action ([`OPTIONS_ACTION`], past retail's 0 to 9) when the
//! list has Change Map and the lobby's map ([`LOBBY_MAP`]) is a converted
//! Halo 1 map ([`converted_map_name`]). The dialog is built again each time
//! it opens, so changing the map and coming back shows or hides the row.
//! `before` hooks draw its text (`0x2bb74f`; retail leaves an unknown
//! action's text as it was) and take its press (`0x2bb801`, whose jump
//! table has no bounds check, so the original never sees our action).
//!
//! **Which stack.** The press opens the screen on the dialog's own stack
//! (window 3 in a lobby), with a request built from the dialog screen's
//! `+0x74` and `+0x78`, as the variant editor's Skulls row does: the first
//! Skulls row here opened through `0x149f1e`, which always says window 5,
//! and went under the lobby (docs/game/skulls.md "Why the Game Setup row
//! broke").
//!
//! **The lobby's map** is the map id at `0x51098c`: `0x19a6f2` stores it
//! when the lobby's map is set (`0x19a864` sets it with campaign -1 for a
//! multiplayer game; the Change Map list `0x2c9a73` focuses the row whose
//! record names it). A converted map's id is its content package's list id
//! (`content::map_list`), and its package directory's marker
//! (`new_maps::MARKER`) names the map: `h1_<map>`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use engine::entry::{EntryFuture, GuestCall, Ret};
use engine::memory::GuestMemory;
use hooks::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;

use crate::h1_options::{self, OptionsScreen};
use crate::list_class::{self, ListClass};

/// The row's feature (`features.h1-options-row` switches it).
pub const ROW_FEATURE: &str = "h1-options-row";
/// The screen's list class (`features.h1-options-screen`).
pub const SCREEN_FEATURE: &str = "h1-options-screen";

/// The lobby's map id (`0x19a6f2` stores it; -1 for none).
pub const LOBBY_MAP: u32 = 0x0051_098c;
/// The Game Setup list: its rows' datum array at handler `+0x70`, each
/// row's action word at datum `+2`.
pub const SETUP_ROWS: u32 = 0x70;
pub const SETUP_ACTION: u32 = 2;
/// Retail's "Change Map": only a multiplayer custom game's list has it.
pub const CHANGE_MAP: i16 = 0;
/// Retail's "Quick Options..." row.
pub const QUICK_OPTIONS: i16 = 4;
/// Retail's "Switch To:" rows (co-op 6, custom game 7, matchmaking 8; the
/// row text table at `0x2bb74f`). Our rows go above the first of them, so
/// they sit under Quick Options (a custom game's list is Change Map,
/// Change Rules, Quick Options, Switch To: Co-Op Game, Switch To:
/// Matchmaking; ctor `0x2bb5a8`).
pub const SWITCH_TO: [i16; 3] = [6, 7, 8];
/// Our row's action: past retail's (0 to 9; the row-chosen jump table has
/// no bounds check, so the press must never reach the original).
pub const OPTIONS_ACTION: i16 = 10;
/// A widget's type (`+4`; 0 a screen) and parent (`+0x10`).
const WIDGET_TYPE: u32 = 4;
const WIDGET_PARENT: u32 = 0x10;

/// The screen's class: a dialog on the lobby's Quick Options
/// ([`h1_options::DIALOG`], docs/game/ui-builder.md "The Dialog kind"),
/// inside as well as out: Quick Options' tag copied with its own list (its
/// `setting_lists` skin, a name and a choice per row, where Quick Options
/// has it), Quick Options' header (the screen's icon in its game type
/// picture, its line in the help line), and the legend in its footer.
/// Left and right step a row, A opens the value picker; B closes it.
pub fn class(screen: Arc<OptionsScreen>) -> ListClass {
    let base = h1_options::DIALOG;
    let mut c = ListClass::with_rows(SCREEN_FEATURE, h1_options::SCREEN, screen)
        .dialog(base)
        .sideways()
        .footer(h1_options::FOOTER_ID);
    if let Some(help) = base.help() {
        c = c.description(help);
    }
    // Quick Options' header: the screen's icon in its game type picture.
    if let Some(picture) = base.picture() {
        c = c.picture(picture);
    }
    c
}

/// Whether a Game Setup list with these actions, on a lobby whose map is
/// `map` (`Some` name when it is a converted map), gets our row.
pub fn wants_row(actions: &[i16], map: Option<&str>) -> bool {
    actions.contains(&CHANGE_MAP)
        && !actions.contains(&OPTIONS_ACTION)
        && map.is_some_and(is_halo1_map)
}

/// Whether `name` is a converted Halo 1 multiplayer map.
pub fn is_halo1_map(name: &str) -> bool {
    name.to_ascii_lowercase().starts_with("h1_")
}

/// The name of the converted map whose list id is `map_id`, from its
/// content package directory under `content_root` (`new_maps` writes the
/// marker); `None` for any other id.
pub fn converted_map_name(content_root: &Path, map_id: u32) -> Option<String> {
    if map_id == u32::MAX
        || !content::map_list::is_new_map_offer(content::map_list::offer_content(map_id))
    {
        return None;
    }
    let offer = vfs::offer_id(
        crate::new_maps::HALO2_TITLE,
        content::map_list::offer_content(map_id),
    );
    let text = std::fs::read_to_string(
        content_root
            .join(vfs::content_dir_name(offer))
            .join(crate::new_maps::MARKER),
    )
    .ok()?;
    let t: toml::Table = toml::from_str(&text).ok()?;
    t.get("map")?.as_str().map(str::to_owned)
}

/// The actions of the list at `list`, in slot order.
pub fn setup_actions(m: &dyn GuestMemory, list: u32) -> Result<Vec<i16>, engine::memory::MemFault> {
    let array = engine::datum::DatumArray::at(m.u32(list.wrapping_add(SETUP_ROWS))?);
    let hw = array.header_field(m, engine::datum::layout::HIGH_WATER)?;
    let mut out = Vec::new();
    for i in 0..hw.max(0) {
        if let Some(elem) = array.get_absolute(m, i)? {
            out.push(m.u16(elem.wrapping_add(SETUP_ACTION))? as i16);
        }
    }
    Ok(out)
}

/// Adds our row to the list at `list`; the new count.
pub fn add_row(m: &mut dyn GuestMemory, list: u32) -> Result<usize, String> {
    insert_row(m, list, OPTIONS_ACTION)
}

/// The list's actions with `ours` placed above the first "Switch To:" row
/// (at the end when there is none).
pub fn placed(actions: &[i16], ours: i16) -> Vec<i16> {
    let at = actions
        .iter()
        .position(|a| SWITCH_TO.contains(a))
        .unwrap_or(actions.len());
    let mut out = actions.to_vec();
    out.insert(at, ours);
    out
}

/// Adds a row with `action` to the list at `list`, above the "Switch To:"
/// rows ([`placed`]): a new datum, then the rows' actions written back in
/// slot order. The new count.
pub fn insert_row(m: &mut dyn GuestMemory, list: u32, action: i16) -> Result<usize, String> {
    insert_row_ordered(m, list, |actions| placed(actions, action))
}

/// Adds a row to the list at `list`: a new datum, then the actions
/// `order` gives for the list's actions (one more than there were)
/// written back in slot order. The new count.
pub fn insert_row_ordered(
    m: &mut dyn GuestMemory,
    list: u32,
    order: impl FnOnce(&[i16]) -> Vec<i16>,
) -> Result<usize, String> {
    let e = |e: engine::memory::MemFault| format!("{e:?}");
    let actions = setup_actions(m, list).map_err(e)?;
    let array = engine::datum::DatumArray::at(m.u32(list.wrapping_add(SETUP_ROWS)).map_err(e)?);
    array.new_datum(m).map_err(e)?.ok_or("the list is full")?;
    let hw = array
        .header_field(m, engine::datum::layout::HIGH_WATER)
        .map_err(e)?;
    let mut elems = Vec::new();
    for i in 0..hw.max(0) {
        if let Some(elem) = array.get_absolute(m, i).map_err(e)? {
            elems.push(elem);
        }
    }
    let order = order(&actions);
    if elems.len() != order.len() {
        return Err(format!(
            "{} rows after adding, not {}",
            elems.len(),
            order.len()
        ));
    }
    for (elem, a) in elems.into_iter().zip(&order) {
        m.set_u16(elem.wrapping_add(SETUP_ACTION), *a as u16)
            .map_err(e)?;
    }
    Ok(order.len())
}

/// The action of the row whose handle is `row` on the list at `list`.
pub fn row_action(m: &dyn GuestMemory, list: u32, row: u32) -> Option<i16> {
    let array = engine::datum::DatumArray::at(m.u32(list.wrapping_add(SETUP_ROWS)).ok()?);
    let elem = array
        .try_get(m, engine::DatumHandle::from_raw(row)?)
        .ok()??;
    m.u16(elem.wrapping_add(SETUP_ACTION))
        .ok()
        .map(|v| v as i16)
}

/// The screen widget holding `widget`: the first parent of type 0.
pub fn screen_of(m: &dyn GuestMemory, widget: u32) -> Option<u32> {
    let mut w = widget;
    for _ in 0..16 {
        if w == 0 {
            return None;
        }
        if m.u32(w.wrapping_add(WIDGET_TYPE)).ok()? == 0 {
            return Some(w);
        }
        w = m.u32(w.wrapping_add(WIDGET_PARENT)).ok()?;
    }
    None
}

/// The row on the Game Setup dialog.
pub struct SetupRow {
    pub data_root: PathBuf,
}

fn once(seen: &AtomicBool, what: &str) {
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("{what}");
    }
}

fn row_text(c: &mut Call, seen: &AtomicBool) -> Result<Flow, GuestFault> {
    let (item, action) = {
        let mem = c.ctx.runtime().memory();
        let item = mem.u32(c.cpu.esp + 4).ok();
        let action = item
            .and_then(|i| mem.u32(i + 0x70).ok())
            .and_then(|row| row_action(&*mem, c.cpu.ecx, row));
        (item, action)
    };
    let (Some(item), Some(OPTIONS_ACTION)) = (item, action) else {
        return Ok(Flow::Continue);
    };
    once(seen, "h1 options: Game Setup row text");
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            list_class::draw_texts(&cx, item, vec![(0, h1_options::ROW_LABEL_ID)]).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

fn row_chosen(c: &mut Call, request_at: u32) -> Result<Flow, GuestFault> {
    let (action, player, stack) = {
        let mem = c.ctx.runtime().memory();
        let row = mem.u32(c.cpu.esp + 8).ok().and_then(|p| mem.u32(p).ok());
        let action = row.and_then(|row| row_action(&*mem, c.cpu.ecx, row));
        let player = crate::skulls_menu::player_index(&*mem, c.cpu.esp);
        let stack = screen_of(&*mem, c.cpu.ecx).and_then(|s| {
            Some((
                mem.u32(s + list_class_window()).ok()?,
                mem.u32(s + list_class_slot()).ok()?,
            ))
        });
        (action, player, stack)
    };
    if action != Some(OPTIONS_ACTION) {
        return Ok(Flow::Continue);
    }
    let (Some(player), Some((window, slot)), Some(loader)) =
        (player, stack, list_class::loader_of(h1_options::SCREEN))
    else {
        eprintln!(
            "h1 options: not opened (player {player:?}, stack {stack:?}, class {})",
            if list_class::loader_of(h1_options::SCREEN).is_some() {
                "on"
            } else {
                "off"
            }
        );
        return c.ret(8).map(Flow::Return);
    };
    eprintln!(
        "h1 options: opening the Halo 1 Options screen (player {player}) on window {window}, slot {slot}"
    );
    let request = crate::skulls_mp::open_request(1 << player, window, slot, loader);
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            cx.write(|m| m.write(request_at, &request))?;
            cx.call(GuestCall::new(loader).args([request_at])).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

/// Why `h1-options-row` (and `clankers-row`, `clankermatch-switch`) can't
/// be snapshotted now: a Game Setup list's constructor is between its
/// hooks, with the handler being built held on the host. The row itself
/// is written into guest memory.
pub(crate) fn building_live(building: &std::sync::Mutex<Vec<u32>>) -> Option<String> {
    let n = building.lock().unwrap_or_else(|e| e.into_inner()).len();
    (n != 0).then(|| format!("{n} Game Setup lists are being built"))
}

const fn list_class_window() -> u32 {
    crate::ui_kit::lifecycle::SCREEN_WINDOW
}
const fn list_class_slot() -> u32 {
    crate::ui_kit::lifecycle::SCREEN_SLOT
}

impl hooks::Feature for SetupRow {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ROW_FEATURE,
            "The Halo 1 Options row on a custom game's Game Setup",
        )
        .doc("docs/halo1/multiplayer.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use engine::points::{
            SQUAD_SETTINGS_LIST_CTOR as CTOR, SQUAD_SETTINGS_ROW_CHOSEN, SQUAD_SETTINGS_ROW_TEXT,
        };
        let request_at = b.guest_alloc(0x1000, Permissions::RW)?;
        let content_root = vfs::Config::new(&self.data_root, crate::new_maps::HALO2_TITLE)
            .title_data_dir()
            .join(vfs::CONTENT_DIR);
        // The handler being built (ESI at entry); a stack in case of nesting.
        let building: Arc<std::sync::Mutex<Vec<u32>>> = Arc::default();
        let b2 = Arc::clone(&building);
        let b2_live = Arc::clone(&building);
        b.raw().before(&CTOR, "note the Game Setup list", move |c| {
            b2.lock().unwrap_or_else(|e| e.into_inner()).push(c.cpu.esi);
            Ok(Flow::Continue)
        });
        b.raw().after(&CTOR, "add the Halo 1 Options row", move |c| {
            let Some(list) = building.lock().unwrap_or_else(|e| e.into_inner()).pop() else {
                return Ok(());
            };
            let rt = Arc::clone(c.ctx.runtime());
            let (actions, map_id) = {
                let mem = rt.memory();
                (setup_actions(&*mem, list).unwrap_or_default(), mem.u32(LOBBY_MAP).unwrap_or(u32::MAX))
            };
            // The package marker is read with guest memory released.
            let map = converted_map_name(&content_root, map_id);
            if !wants_row(&actions, map.as_deref()) {
                if actions.contains(&CHANGE_MAP) {
                    eprintln!(
                        "h1 options: Game Setup list: map {map_id:#x} ({}) is not a Halo 1 map; no row",
                        map.as_deref().unwrap_or("not a converted map")
                    );
                }
                return Ok(());
            }
            match add_row(&mut *rt.memory(), list) {
                Ok(n) => eprintln!(
                    "h1 options: Game Setup list: Halo 1 Options row added for {} ({n} rows)",
                    map.unwrap_or_default()
                ),
                Err(why) => eprintln!("h1 options: Game Setup list: row not added: {why}"),
            }
            Ok(())
        });
        let seen = Arc::new(AtomicBool::new(false));
        b.raw().before(
            &SQUAD_SETTINGS_ROW_TEXT,
            "the Halo 1 Options row's text",
            move |c| row_text(c, &seen),
        );
        b.raw().before(
            &SQUAD_SETTINGS_ROW_CHOSEN,
            "the Halo 1 Options row's press",
            move |c| row_chosen(c, request_at),
        );
        let live = Arc::clone(&b2_live);
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            building_live(&live)
        }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    #[test]
    fn a_snapshot_waits_while_a_game_setup_list_is_built() {
        let building = std::sync::Mutex::new(Vec::new());
        assert_eq!(building_live(&building), None);
        building.lock().unwrap().push(0x0010_0000);
        assert!(building_live(&building).is_some());
        building.lock().unwrap().pop();
        assert_eq!(building_live(&building), None);
    }

    #[test]
    fn only_a_custom_games_list_on_a_halo1_map_gets_the_row() {
        assert!(wants_row(&[0, 1, 2], Some("h1_bloodgulch")));
        assert!(!wants_row(&[0, 1, 2], Some("lockout")), "not a Halo 1 map");
        assert!(!wants_row(&[0, 1, 2], None), "a disc map");
        assert!(
            !wants_row(&[1, 2], Some("h1_bloodgulch")),
            "no Change Map: not a custom game"
        );
        assert!(!wants_row(&[0, 10], Some("h1_bloodgulch")), "already there");
    }

    #[test]
    fn our_rows_sit_under_quick_options_above_switch_to() {
        // A custom game's list: Change Map, Change Rules, Quick Options,
        // Switch To: Co-Op Game, Switch To: Matchmaking.
        let custom = [CHANGE_MAP, 1, QUICK_OPTIONS, 6, 8];
        let h1 = placed(&custom, OPTIONS_ACTION);
        assert_eq!(h1, [0, 1, 4, OPTIONS_ACTION, 6, 8]);
        assert_eq!(
            placed(&h1, 11),
            [0, 1, 4, OPTIONS_ACTION, 11, 6, 8],
            "Clankers after it"
        );
        assert_eq!(placed(&[0, 1, 4], 11), [0, 1, 4, 11], "no Switch To: last");
        assert_eq!(placed(&[2, 3, 7, 8], 11), [2, 3, 11, 7, 8]);
    }

    #[test]
    fn the_lobby_map_is_named_by_its_package_marker() {
        let root = std::env::temp_dir().join(format!("h2-h1row-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let id = content::map_list::LIST_ID_FIRST + 3;
        let offer = vfs::offer_id(
            crate::new_maps::HALO2_TITLE,
            content::map_list::offer_content(id),
        );
        let dir = root.join(vfs::content_dir_name(offer));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(crate::new_maps::MARKER),
            "source = \"h1_bloodgulch\"\nmap = \"h1_bloodgulch\"\nmap_id = \"00\"\n",
        )
        .unwrap();
        assert_eq!(
            converted_map_name(&root, id).as_deref(),
            Some("h1_bloodgulch")
        );
        assert_eq!(converted_map_name(&root, id + 1), None, "no package");
        assert_eq!(converted_map_name(&root, 3), None, "a retail id");
        assert_eq!(converted_map_name(&root, u32::MAX), None, "no map");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_dialogs_screen_is_the_lists_first_screen_parent() {
        let mut g = FakeGuest::new();
        g.map(0x1000, 0x100).map(0x2000, 0x100).map(0x3000, 0x100);
        g.put_u32(0x3000 + 4, 1).put_u32(0x3000 + 0x10, 0x2000);
        g.put_u32(0x2000 + 4, 5).put_u32(0x2000 + 0x10, 0x1000);
        g.put_u32(0x1000 + 4, 0);
        assert_eq!(screen_of(&g, 0x3000), Some(0x1000));
        assert_eq!(screen_of(&g, 0), None);
    }

    #[test]
    fn the_hooks_are_on_the_plan() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        r.add(&SetupRow {
            data_root: std::env::temp_dir(),
        })
        .unwrap();
        let text = r.build(&hooks::Base).unwrap().text();
        assert!(text.contains("0x002bb4d2 SQUAD_SETTINGS_LIST_CTOR: before [h1-options-row:note the Game Setup list] -> original -> after [h1-options-row:add the Halo 1 Options row]"), "{text}");
        assert!(
            text.contains("SQUAD_SETTINGS_ROW_TEXT: before [h1-options-row:"),
            "{text}"
        );
        assert!(
            text.contains("SQUAD_SETTINGS_ROW_CHOSEN: before [h1-options-row:"),
            "{text}"
        );
    }
}
