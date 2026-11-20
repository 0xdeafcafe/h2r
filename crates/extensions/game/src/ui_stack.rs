//! A tick-boundary read of the physical screen channels and known Back
//! paths. The channel's +0x34 points into a screen's widget tree.

use std::sync::Arc;

use agent::Access;
use agent_proto::{RpcError, code, method};
use engine::{GuestMemory, points};
use serde_json::{Value, json};

use crate::layout_glue::ReadGm;
use crate::map_select::{Mode, SCREEN_LIST};
use crate::ui_back::{self, Back, GAME_SETUP_ID, Navigation, SCREEN_ID, SELECT_MAP_ID};
use crate::ui_kit::input::Input;
use crate::ui_kit::lifecycle::{SCREEN_STATE, focused_row_handle};
use crate::ui_watch::{ACTIVE, PENDING, SHARED, channel};

const OUTGOING: u32 = 0x30;
const FOCUSED: u32 = 0x34;
const WIDGET_PARENT: u32 = 0x10;
const PLAYER_MASK: u32 = 0x8;
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

/// The first occupied channel in dispatcher order on the shared and
/// player-one slots. A channel that is transitioning consumes the event.
fn input_channel(m: &dyn GuestMemory, channels: &[Channel]) -> Option<Channel> {
    for slot in [SHARED, 0] {
        let Some(ch) = channels.iter().copied().find(|c| c.slot == slot && c.occupied()) else {
            continue;
        };
        let mut widget = m.u32(ch.address + FOCUSED).ok()?;
        let mut player_one = false;
        let mut root = 0;
        for _ in 0..MAX_LINKS {
            if widget == 0 {
                break;
            }
            player_one |= m.u16(widget.checked_add(PLAYER_MASK)?).ok()? & 1 != 0;
            root = widget;
            widget = m.u32(widget.checked_add(WIDGET_PARENT)?).ok()?;
        }
        if widget != 0
            || root == 0
            || root != ch.active
            || m.u8(root.wrapping_add(SCREEN_STATE)).ok()? & 3 != 1
        {
            return None;
        }
        if player_one {
            return Some(ch);
        }
    }
    None
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
    let setup = nav.setup_parent(screen).map(|_| json!({"id": GAME_SETUP_ID}));
    let parent = match mode {
        Some(Mode::Maps(_)) => json!({"id": SELECT_MAP_ID, "mode": "folders", "parent": setup}),
        _ => setup.or_else(|| native_parent.map(|id| json!({"id": id}))).unwrap_or(Value::Null),
    };
    let (mode, folder) = match mode {
        Some(Mode::Categories) => (Some("folders"), None),
        Some(Mode::Maps(category)) => (Some("maps"), Some(category.label())),
        None => (None, None),
    };
    let focused_row = (mode.is_some() && screen == ch.active)
        .then(|| focused_row_handle(m, ch.address, list).map(|handle| handle as u16))
        .flatten();
    json!({
        "id": id,
        "instance": screen,
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
    let input_channel = input_channel(m, &channels);
    let mut input = Value::Null;
    let mut back = Value::Null;
    let channels: Vec<_> = channels.into_iter().map(|ch| {
        let history = ch.history(m);
        let active = screen_view(m, nav, ch, ch.active, history.first().copied());
        if input_channel.is_some_and(|selected| selected.address == ch.address)
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
    json!({"channels": channels, "input": input, "back": back})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_select::Category;
    use crate::ui_kit::lifecycle::{SCREEN_SLOT, SCREEN_WINDOW};
    use engine::testing::FakeGuest;

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
        m.set_u8(SCREEN + SCREEN_STATE, 1).unwrap();
        m.set_u16(SCREEN + PLAYER_MASK, 1).unwrap();
        (m, ch)
    }

    #[test]
    fn maps_report_the_folder_parent_and_back_action() {
        let nav = Navigation::default();
        let (m, _) = focused(3, SELECT_MAP_ID);
        nav.maps.built(SCREEN + SCREEN_LIST, Mode::Maps(Category::Halo1), None);
        let answer = inspect(&m, &nav);
        let map = &answer["channels"][0]["active"];
        assert_eq!(answer["input"]["id"], SELECT_MAP_ID);
        assert_eq!(map["mode"], "maps");
        assert_eq!(map["folder"], Category::Halo1.label());
        assert_eq!(map["focused_row"], 2);
        assert_eq!(answer["back"]["instance"], SCREEN);
        assert_eq!(answer["back"]["parent"]["mode"], "folders");
        assert_eq!(answer["back"]["action"], "reopen");

        nav.maps.built(SCREEN + SCREEN_LIST, Mode::Categories, None);
        let answer = inspect(&m, &nav);
        assert_eq!(answer["channels"][0]["active"]["mode"], "folders");
        assert_eq!(answer["channels"][0]["active"]["focused_row"], 2);
        assert_eq!(answer["back"]["action"], "close");
        assert!(answer["back"]["parent"].is_null());
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
