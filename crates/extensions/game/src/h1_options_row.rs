//! The "Halo 1 Options..." row on a multiplayer custom game's Game Setup
//! dialog, next to its map and game type, while the lobby's map is a
//! converted Halo 1 map; and the page it opens (`crate::h1_options::page`,
//! docs/halo1/multiplayer.md "Halo 1 options"). The row's text and its
//! Game Setup description are the page's caller (`settings.caller` in
//! `content/ui/screens/h1-options.yaml`), written from the page's own text
//! slots, as the Clankers row's are.
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

use guest_layout::entry::{EntryFuture, GuestCall, Ret};
use guest_layout::memory::GuestMemory;
use hooks::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;

use crate::h1_options;
use crate::list_class::{self, Rows};
use crate::ui_kit::screen::KitScreen;

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
/// Retail's "Quick Options..." row. For a variant whose rules script
/// marks Quick Options rows, its press opens ours (`crate::variant_quick::page`,
/// docs/game/rules-variants.md "Quick Options in the lobby"); for a
/// script with no marks it is greyed and does nothing.
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
pub fn setup_actions(m: &dyn GuestMemory, list: u32) -> Result<Vec<i16>, guest_layout::memory::MemFault> {
    let array = guest_layout::datum::DatumArray::at(m.u32(list.wrapping_add(SETUP_ROWS))?);
    let hw = array.header_field(m, guest_layout::datum::layout::HIGH_WATER)?;
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

/// Adds rows to the list at `list`: the actions `order` gives for the
/// list's actions (as many more as it adds), a new datum for each added
/// one, then every action written back in slot order. The new count. A
/// list without room for them all is left as it was.
pub fn insert_row_ordered(
    m: &mut dyn GuestMemory,
    list: u32,
    order: impl FnOnce(&[i16]) -> Vec<i16>,
) -> Result<usize, String> {
    let e = |e: guest_layout::memory::MemFault| format!("{e:?}");
    let actions = setup_actions(m, list).map_err(e)?;
    let order = order(&actions);
    let adds = order
        .len()
        .checked_sub(actions.len())
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("{} rows asked for {} there", order.len(), actions.len()))?;
    let array = guest_layout::datum::DatumArray::at(m.u32(list.wrapping_add(SETUP_ROWS)).map_err(e)?);
    let room = array.header_field(m, guest_layout::datum::layout::MAXIMUM_COUNT).map_err(e)?
        - array.header_field(m, guest_layout::datum::layout::ACTIVE_COUNT).map_err(e)?;
    if (room.max(0) as usize) < adds {
        return Err(format!("the list has room for {room} rows, not {adds}"));
    }
    for _ in 0..adds {
        array.new_datum(m).map_err(e)?.ok_or("the list is full")?;
    }
    let hw = array
        .header_field(m, guest_layout::datum::layout::HIGH_WATER)
        .map_err(e)?;
    let mut elems = Vec::new();
    for i in 0..hw.max(0) {
        if let Some(elem) = array.get_absolute(m, i).map_err(e)? {
            elems.push(elem);
        }
    }
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

/// Gives the list at `list` exactly the rows `order` names, in slot order:
/// datums added or deleted from the end until there are as many, then each
/// one's action written. The new count. A Game Setup row of ours that hides
/// a row (the lobby's mode, `crate::lobby_mode`) passes the list without it.
pub fn rewrite_rows(m: &mut dyn GuestMemory, list: u32, order: &[i16]) -> Result<usize, String> {
    let e = |e: guest_layout::memory::MemFault| format!("{e:?}");
    let array = guest_layout::datum::DatumArray::at(m.u32(list.wrapping_add(SETUP_ROWS)).map_err(e)?);
    let elems = |m: &dyn GuestMemory| -> Result<Vec<(i32, u32)>, String> {
        let hw = array
            .header_field(m, guest_layout::datum::layout::HIGH_WATER)
            .map_err(e)?;
        let mut out = Vec::new();
        for i in 0..hw.max(0) {
            if let Some(elem) = array.get_absolute(m, i).map_err(e)? {
                out.push((i, elem));
            }
        }
        Ok(out)
    };
    let mut have = elems(&*m)?;
    while have.len() < order.len() {
        array.new_datum(m).map_err(e)?.ok_or("the list is full")?;
        have = elems(&*m)?;
    }
    while have.len() > order.len() {
        let (index, _) = have.pop().expect("more rows than wanted");
        let handle = array
            .handle_of(&*m, index)
            .map_err(e)?
            .ok_or_else(|| format!("row {index} has no handle"))?;
        array.delete(m, handle).map_err(e)?;
    }
    for ((_, elem), a) in have.into_iter().zip(order) {
        m.set_u16(elem.wrapping_add(SETUP_ACTION), *a as u16)
            .map_err(e)?;
    }
    Ok(order.len())
}

/// The action of the row whose handle is `row` on the list at `list`.
pub fn row_action(m: &dyn GuestMemory, list: u32, row: u32) -> Option<i16> {
    let array = guest_layout::datum::DatumArray::at(m.u32(list.wrapping_add(SETUP_ROWS)).ok()?);
    let elem = array
        .try_get(m, guest_layout::DatumHandle::from_raw(row)?)
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

/// The row on the Game Setup dialog, and the page its press opens.
pub struct SetupRow {
    pub data_root: PathBuf,
    pub page: Arc<KitScreen>,
}

fn once(seen: &AtomicBool, what: &str) {
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("{what}");
    }
}

fn row_text(c: &mut Call, page: &KitScreen, seen: &AtomicBool) -> Result<Flow, GuestFault> {
    let rt = Arc::clone(c.ctx.runtime());
    let (item, id) = {
        let mut mem = rt.memory();
        let item = mem.u32(c.cpu.esp + 4).ok();
        let action = item
            .and_then(|i| mem.u32(i + 0x70).ok())
            .and_then(|row| row_action(&*mem, c.cpu.ecx, row));
        let Some(item) = item.filter(|_| action == Some(OPTIONS_ACTION)) else {
            return Ok(Flow::Continue);
        };
        // The row's own text ("Halo 1 Options..."), from the page's slot
        // after its footer, written before it is drawn.
        let id = page.row_label_id();
        if let Err(e) = page.write_texts(&mut *mem) {
            eprintln!("h1 options: the row's text: {e:?}");
        }
        (item, id)
    };
    once(seen, "h1 options: Game Setup row text");
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            list_class::draw_texts(&cx, item, vec![(0, id)]).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

/// A press on a Game Setup row (`SQUAD_SETTINGS_ROW_CHOSEN`, ECX the
/// list): the row's action, the pressing player, and the dialog screen's
/// stack (its `+0x74` window and `+0x78` slot).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetupPress {
    pub action: Option<i16>,
    pub player: Option<u32>,
    pub stack: Option<(u32, u32)>,
}

/// The press `c` is on.
pub fn setup_press(c: &mut Call) -> SetupPress {
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
    SetupPress {
        action,
        player,
        stack,
    }
}

/// Answers press `p` by opening our screen `screen` on the dialog's own
/// stack for the pressing player ("Which stack" above), with the request
/// at `request_at`; `who` names it in the log. Without a player, a stack
/// or the screen's class the press does nothing (the original would run
/// another action's press for ours).
pub fn open_from_setup(
    c: &mut Call,
    request_at: u32,
    p: SetupPress,
    screen: &str,
    who: &str,
) -> Result<Flow, GuestFault> {
    let loader = list_class::loader_of(screen);
    let (Some(player), Some((window, slot)), Some(loader)) = (p.player, p.stack, loader) else {
        eprintln!(
            "{who}: {screen} not opened (player {:?}, stack {:?}, class {})",
            p.player,
            p.stack,
            if loader.is_some() { "on" } else { "off" }
        );
        return c.ret(8).map(Flow::Return);
    };
    eprintln!("{who}: opening {screen} (player {player}) on window {window}, slot {slot}");
    let request = crate::skulls_mp::open_request(1 << player, window, slot, loader);
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            cx.write(|m| m.write(request_at, &request))?;
            cx.call(GuestCall::new(loader).args([request_at])).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

fn row_chosen(c: &mut Call, request_at: u32) -> Result<Flow, GuestFault> {
    let p = setup_press(c);
    if p.action != Some(OPTIONS_ACTION) {
        return Ok(Flow::Continue);
    }
    open_from_setup(c, request_at, p, h1_options::SCREEN, "h1 options")
}

/// Why `h1-options-row` (and `clankers-row`, `clankermatch-switch`) can't
/// be snapshotted now: a Game Setup list's constructor is between its
/// hooks, with the handler being built held on the host. The row itself
/// is written into guest memory.
pub fn building_live(building: &std::sync::Mutex<Vec<u32>>) -> Option<String> {
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
        use guest_layout::points::{
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
        let page = Arc::clone(&self.page);
        b.raw().before(
            &SQUAD_SETTINGS_ROW_TEXT,
            "the Halo 1 Options row's text",
            move |c| row_text(c, &page, &seen),
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

/// The Halo 1 Options page over a store at its defaults, for the tests
/// that put the row on a plan.
#[cfg(test)]
pub(crate) fn test_page() -> Arc<KitScreen> {
    let store = Arc::new(settings::Store::defaults(crate::setting_defs::ALL));
    Arc::new(h1_options::page(store))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

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
            page: test_page(),
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
