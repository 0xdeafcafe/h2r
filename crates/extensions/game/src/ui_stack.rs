//! A tick-boundary read of the physical screen channels and known Back
//! paths. The channel's +0x34 points into a screen's widget tree.

use std::sync::Arc;

use agent::Access;
use agent_proto::{RpcError, code, method};
use guest_layout::{GuestMemory, points};
use serde_json::{Value, json};

use crate::layout_glue::ReadGm;
use crate::map_select::{Mode, SCREEN_LIST};
use crate::ui_back::{self, Back, GAME_SETUP_ID, Navigation, SCREEN_ID, SELECT_MAP_ID};
use crate::ui_kit::input::Input;
use crate::ui_kit::lifecycle::focused_row_handle;
use crate::ui_watch::{ACTIVE, PENDING, SHARED, channel};

const OUTGOING: u32 = 0x30;
const SCREEN_EVENT_SLOT: u32 = 0x28;
const HISTORY: u32 = 0x38;
const HISTORY_SCREEN_ID: u32 = 0x24;
const CLOSE_WITHOUT_HISTORY: u32 = 0x3c;
const MAX_LINKS: usize = 32;

pub fn install(agent: &agent::Agent, navigation: Arc<Navigation>) {
    agent.add_game_method(method::UI_STACK, Access::Read, move |params, ctx| {
        if !params.is_null() && !params.as_object().is_some_and(|p| p.is_empty()) {
            return Err(RpcError::new(code::INVALID_PARAMS, "ui.stack takes no parameters"));
        }
        ctx.read_guest(|mem| inspect(mem, &navigation))
    });
}

#[derive(Clone, Copy)]
struct Channel {
    window: u32,
    slot: u32,
    address: u32,
    active: u32,
    pending: u32,
    outgoing: u32,
}

impl Channel {
    fn read(m: &dyn GuestMemory, window: u32, slot: u32) -> Option<Self> {
        let address = channel(window, slot)?;
        Some(Self {
            window,
            slot,
            address,
            active: m.u32(address + ACTIVE).ok()?,
            pending: m.u32(address + PENDING).ok()?,
            outgoing: m.u32(address + OUTGOING).ok()?,
        })
    }

    fn occupied(self) -> bool {
        self.active != 0 || self.pending != 0
    }

    /// The reopen requests of the game-shell channel, nearest parent first.
    fn history(self, m: &dyn GuestMemory) -> Vec<u32> {
        let mut parents = Vec::new();
        if self.window == 5 {
            let mut node = m.u32(self.address + HISTORY).unwrap_or(0);
            for _ in 0..MAX_LINKS {
                if node == 0 {
                    break;
                }
                let Some(id) = node.checked_add(HISTORY_SCREEN_ID).and_then(|p| m.u32(p).ok()) else {
                    break;
                };
                parents.push(id);
                node = m.u32(node).unwrap_or(0);
            }
        }
        parents
    }
}

fn screen_back(m: &dyn GuestMemory, ch: Channel, id: u32, active: &Value) -> Value {
    let screen = ch.active;
    let parent = &active["parent"];
    if ch.window == ui_back::LOBBY_DIALOG && (!parent.is_null() || !active["mode"].is_null()) {
        let action = ui_back::back(&ReadGm(m), screen, Some(Input::B)).0;
        if action == Back::Wait {
            return json!({"instance": screen, "id": id, "action": "wait", "parent": parent});
        }
        if action == Back::Reopen && !parent.is_null() {
            return json!({"instance": screen, "id": id, "action": "reopen", "parent": parent});
        }
    }
    let handler = m
        .u32(screen)
        .ok()
        .and_then(|vt| vt.checked_add(SCREEN_EVENT_SLOT))
        .and_then(|p| m.u32(p).ok());
    let generic = handler == Some(points::SCREEN_BACK_EVENT.base)
        || handler == Some(points::GAME_SETUP_EVENT.base);
    if ch.window == crate::ui_kit::dialog_tag::DIALOG_WINDOW && value_picker(active["class"].as_str()) {
        return json!({"instance": screen, "id": id, "action": "close", "parent": parent});
    }
    let closes = match ch.window {
        2 | 3 => true,
        5 => !parent.is_null() || m.u8(ch.address + CLOSE_WITHOUT_HISTORY).is_ok_and(|f| f != 0),
        _ => false,
    };
    if generic && closes {
        json!({"instance": screen, "id": id, "action": "close", "parent": parent})
    } else {
        Value::Null
    }
}

fn value_picker(class: Option<&str>) -> bool {
    matches!(class, Some(crate::ui_kit::picker::NAME | crate::ui_kit::picker::GAME_NAME))
}

fn screen_view(
    m: &dyn GuestMemory,
    nav: &Navigation,
    ch: Channel,
    screen: u32,
    native_parent: Option<u32>,
) -> Value {
    if screen == 0 {
        return Value::Null;
    }
    let id = m.u32(screen.wrapping_add(SCREEN_ID)).ok();
    let list = screen.wrapping_add(SCREEN_LIST);
    let mode = nav.maps.mode_of(list);
    let class = m.u32(screen).ok().and_then(crate::list_class::class_name);
    let setup = nav.setup_parent(screen).map(|_| json!({"id": GAME_SETUP_ID})).or_else(|| {
        nav.category_parent(screen).map(|p| json!({"id": ui_back::CATEGORY_ID,
            "parent": p.setup.map(|_| json!({"id": GAME_SETUP_ID}))}))
    });
    let parent = match mode {
        Some(Mode::Pack(_)) => json!({"id": SELECT_MAP_ID, "mode":"maps", "folder":"Mods",
            "parent":{"id": SELECT_MAP_ID, "mode":"folders", "parent":setup}}),
        Some(Mode::Variants { from, .. }) => json!({"id": SELECT_MAP_ID, "mode":"maps", "folder": from.label(),
            "parent":{"id": SELECT_MAP_ID, "mode":"folders", "parent":setup}}),
        Some(Mode::Maps(_)) if nav.maps.maps_from_setup(list) => setup.unwrap_or(Value::Null),
        Some(Mode::Maps(_)) => json!({"id": SELECT_MAP_ID, "mode": "folders", "parent": setup}),
        _ if value_picker(class) && ch.window == crate::ui_kit::dialog_tag::DIALOG_WINDOW && ch.active == screen => {
            crate::ui_audit::target_beneath(m, screen)
                .map(|target| json!({"id":target.id, "class":m.u32(target.screen).ok().and_then(crate::list_class::class_name)}))
                .unwrap_or(Value::Null)
        }
        _ => setup.or_else(|| native_parent.map(|id| json!({"id": id}))).unwrap_or(Value::Null),
    };
    let pack_name = mode.and_then(|mode| match mode {
        Mode::Pack(id) => crate::map_select::opened_pack_name(id),
        Mode::Variants { map, .. } => crate::map_select::opened_map_name(map),
        _ => None,
    });
    let (mode, folder) = match mode {
        Some(Mode::Categories) => (Some("folders"), None),
        Some(Mode::Maps(category)) => (Some("maps"), Some(category.label())),
        Some(Mode::Pack(_)) => (Some("pack"), pack_name.as_deref()),
        Some(Mode::Variants { .. }) => (Some("variants"), pack_name.as_deref()),
        None => (None, None),
    };
    let variant_list = matches!(id, Some(15 | 208));
    let category = variant_list.then(|| crate::tweakers::screen_row(m, screen)).flatten();
    let picker = mode.is_some() || variant_list || matches!(id, Some(60 | 207));
    let focused_row = (picker && screen == ch.active)
        .then(|| focused_row_handle(m, ch.address, list).map(|handle| handle as u16))
        .flatten();
    json!({
        "id": id,
        "instance": screen,
        "class": class,
        "variant_category": category.map(|row| row.label()),
        "mode": mode,
        "folder": folder,
        "focused_row": focused_row,
        "parent": parent,
    })
}

fn inspect(m: &dyn GuestMemory, nav: &Navigation) -> Value {
    let channels: Vec<_> = (0..=SHARED)
        .flat_map(|slot| (0..6).filter_map(move |window| Channel::read(m, window, slot)))
        .filter(|ch| ch.occupied() || ch.outgoing != 0)
        .collect();
    let target = crate::ui_audit::target(m);
    let mut input = Value::Null;
    let mut back = Value::Null;
    let channels: Vec<_> = channels.into_iter().map(|ch| {
        let history = ch.history(m);
        let active = screen_view(m, nav, ch, ch.active, history.first().copied());
        if target.as_ref().is_some_and(|t| t.screen == ch.active)
            && let Some(id) = m.u32(ch.active.wrapping_add(SCREEN_ID)).ok()
        {
            input = json!({"id": id, "instance": ch.active, "window": ch.window, "slot": ch.slot});
            back = screen_back(m, ch, id, &active);
        }
        json!({
            "window": ch.window,
            "slot": ch.slot,
            "active": active,
            "pending": screen_view(m, nav, ch, ch.pending, None),
            "outgoing": screen_view(m, nav, ch, ch.outgoing, None),
            "history": history,
        })
    }).collect();
    json!({"channels": channels, "input": input, "back": back, "kit": kit()})
}

/// The kit primitive on top, as its screen last said
/// (docs/game/ui-screens.md, "Cards and the card row", "The floating
/// menu"): a card screen's row (`cards`) or a floating menu's anchor
/// (`floating`), each null while none is up.
fn kit() -> Value {
    json!({
        "cards": crate::ui_kit::card::shown(),
        "floating": crate::ui_kit::floating::shown(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_select::Category;
    use crate::ui_kit::lifecycle::{SCREEN_SLOT, SCREEN_STATE, SCREEN_WINDOW};
    use crate::list_class::WIDGET_PARENT;
    const FOCUSED: u32 = 0x34;
    const PLAYER_MASK: u32 = 8;
    use guest_layout::testing::FakeGuest;

    const SCREEN: u32 = 0x10000;
    const ITEM: u32 = 0x11000;
    const VTABLE: u32 = 0x12000;
    const REQUEST: u32 = 0x13000;

    fn focused(window: u32, id: u32) -> (FakeGuest, u32) {
        let mut m = FakeGuest::new();
        m.map(0x0054_d000, 0x1000).map(SCREEN, 0x4000);
        let ch = channel(window, SHARED).unwrap();
        m.put_u32(ch + ACTIVE, SCREEN)
            .put_u32(ch + FOCUSED, ITEM)
            .put_u32(ITEM + WIDGET_PARENT, SCREEN + SCREEN_LIST)
            .put_u32(SCREEN + SCREEN_LIST + WIDGET_PARENT, SCREEN)
            .put_u32(ITEM + SCREEN_ID, 0x8001_0002)
            .put_u32(SCREEN + SCREEN_ID, id)
            .put_u32(SCREEN, VTABLE)
            .put_u32(VTABLE + SCREEN_EVENT_SLOT, points::SCREEN_BACK_EVENT.base);
        m.set_u32(SCREEN + SCREEN_WINDOW, window).unwrap();
        m.set_u32(SCREEN + SCREEN_SLOT, SHARED).unwrap();
        m.set_u8(SCREEN + SCREEN_STATE, 0).unwrap();
        m.set_u16(SCREEN + PLAYER_MASK, 1).unwrap();
        (m, ch)
    }

    #[test]
    fn maps_report_the_folder_parent_and_back_action() {
        let nav = Navigation::default();
        let (m, _) = focused(3, SELECT_MAP_ID);
        nav.maps.built(SCREEN + SCREEN_LIST, Mode::Maps(Category::Halo1), None, false);
        let answer = inspect(&m, &nav);
        let map = &answer["channels"][0]["active"];
        assert_eq!(answer["input"]["id"], SELECT_MAP_ID);
        assert_eq!(map["mode"], "maps");
        assert_eq!(map["folder"], Category::Halo1.label());
        assert_eq!(map["focused_row"], 2);
        assert_eq!(answer["back"]["instance"], SCREEN);
        assert_eq!(answer["back"]["parent"]["mode"], "folders");
        assert_eq!(answer["back"]["action"], "reopen");

        nav.maps.built(SCREEN + SCREEN_LIST, Mode::Maps(Category::Halo2), None, true);
        let direct = inspect(&m, &nav);
        assert!(direct["channels"][0]["active"]["parent"].is_null());
        assert_eq!(direct["back"]["action"], "close");

        nav.maps.built(SCREEN + SCREEN_LIST, Mode::Pack(17), None, false);
        let answer = inspect(&m, &nav);
        assert_eq!(answer["channels"][0]["active"]["mode"], "pack");
        assert_eq!(answer["back"]["action"], "reopen");
        assert_eq!(answer["back"]["parent"]["folder"], "Mods");
        assert_eq!(answer["back"]["parent"]["parent"]["mode"], "folders");

        nav.maps.built(SCREEN + SCREEN_LIST, Mode::Categories, None, false);
        let answer = inspect(&m, &nav);
        assert_eq!(answer["channels"][0]["active"]["mode"], "folders");
        assert_eq!(answer["channels"][0]["active"]["focused_row"], 2);
        assert_eq!(answer["back"]["action"], "close");
        assert!(answer["back"]["parent"].is_null());
    }

    #[test]
    fn a_reused_variant_screen_reports_only_its_current_category() {
        use crate::community_variants::Row;

        let nav = Navigation::default();
        let (mut m, _) = focused(3, 208);
        for row in [Row::Siege, Row::Firefight, Row::PartyGames] {
            m.put_u32(SCREEN + SCREEN_ID, 208)
                .put_u8(SCREEN + crate::tweakers::SCREEN_MARK, row.mark());
            assert_eq!(inspect(&m, &nav)["channels"][0]["active"]["variant_category"], row.label());
            m.put_u32(SCREEN + SCREEN_ID, ui_back::CATEGORY_ID);
            assert!(inspect(&m, &nav)["channels"][0]["active"]["variant_category"].is_null());
        }
    }

    #[test]
    fn native_history_is_the_parent_of_generic_back() {
        let (mut m, ch) = focused(5, 42);
        m.put_u32(ch + HISTORY, REQUEST)
            .put_u32(REQUEST + HISTORY_SCREEN_ID, 19);
        let answer = inspect(&m, &Navigation::default());
        assert_eq!(answer["channels"][0]["history"], json!([19]));
        assert_eq!(answer["back"]["parent"]["id"], 19);
        assert_eq!(answer["back"]["action"], "close");
    }

    #[test]
    fn the_value_picker_closes_to_the_underlying_input_owner() {
        let (mut m, _) = focused(3, GAME_SETUP_ID);
        let modal = channel(1, SHARED).unwrap();
        let picker = 0x26000;
        m.put_u32(modal + ACTIVE, picker).put_u32(modal + FOCUSED, picker)
            .put_u32(picker + SCREEN_ID, 223).put_u16(picker + PLAYER_MASK, 1);
        let under = crate::ui_audit::target_beneath(&m, picker).unwrap();
        assert_eq!(under.screen, SCREEN);
        let view = json!({"class":crate::ui_kit::picker::NAME, "parent":{"id":under.id}});
        let back = screen_back(&m, Channel::read(&m, 1, SHARED).unwrap(), 223, &view);
        assert_eq!(back["action"], "close");
        assert_eq!(back["parent"]["id"], GAME_SETUP_ID);
    }

    #[test]
    fn a_closing_screen_blocks_input_until_the_next_active_screen() {
        let (mut m, ch) = focused(3, SELECT_MAP_ID);
        m.put_u32(ch + PENDING, REQUEST);
        m.set_u8(SCREEN + SCREEN_STATE, 2).unwrap();
        let answer = inspect(&m, &Navigation::default());
        assert_eq!(answer["channels"][0]["pending"]["instance"], REQUEST);
        assert!(answer["input"].is_null());
        assert!(answer["back"].is_null());
    }
}
