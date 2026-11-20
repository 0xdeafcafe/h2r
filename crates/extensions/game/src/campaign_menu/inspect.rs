//! The shared player-one menu channel, read at an agent tick boundary.
use engine::GuestMemory;
use engine::datum::{DatumArray, layout::ACTIVE_COUNT};
use engine::view::ui;
use serde_json::{Value, json};

use super::{PREPARED, Picker, SCREEN, State, options};
use crate::campaigns::OptionsRequest;
use crate::layout_glue::ReadGm;
use crate::ui_kit::lifecycle::{focused_row_handle, window_object};

pub fn inspect(m: &dyn GuestMemory) -> Value {
    let Some(s) = PREPARED
        .get()
        .filter(|_| super::REGION.load(super::Ordering::SeqCst) != 0)
    else {
        return json!({"installed": false});
    };
    let channel = window_object(5, crate::ui_watch::SHARED).unwrap();
    let screen = m.u32(channel + 8).unwrap_or(0);
    let vt = (screen != 0).then(|| m.u32(screen).ok()).flatten();
    let picker = vt.and_then(crate::list_class::layout_screen) == Some(SCREEN);
    inspect_menu(m, s, channel, screen, picker)
}

/// The options request actually constructed after the pick. Store only scalar
/// arguments: the request itself lives on the guest stack.
pub(super) fn observe_options_request(m: &dyn GuestMemory, s: &State, esp: u32) {
    let mut pick = s.selection.pick.lock().unwrap_or_else(|e| e.into_inner());
    let Some(pick) = pick.as_mut().filter(|p| p.request.is_none()) else {
        return;
    };
    let arg = |n: u32| m.u32(esp.wrapping_add(4 + n * 4));
    if let (Ok(mask), Ok(window), Ok(slot), Ok(loader)) = (arg(0), arg(1), arg(2), arg(3))
        && loader == super::OPTIONS_LOADER
    {
        pick.request = Some(OptionsRequest {
            mask: mask as u16,
            window,
            slot,
            loader,
        });
    }
}

fn inspect_menu(m: &dyn GuestMemory, s: &State, channel: u32, screen: u32, picker: bool) -> Value {
    let dialog = crate::ui_watch::dialog(&ReadGm(m));
    let id = (screen != 0)
        .then(|| m.u16(screen.wrapping_add(0x70)).ok())
        .flatten();
    let page = match (dialog, picker, screen, id) {
        (Some(_), _, _, _) => "dialog",
        (None, true, _, _) => "picker",
        (None, _, 0, _) => "other",
        (None, _, _, Some(0xda)) => "options",
        (None, _, _, Some(0x0b)) => "levels",
        (None, _, _, Some(0x0c)) => "difficulty",
        _ => "other",
    };
    let key = |id| s.key_of_campaign(id).unwrap_or("halo2");
    let list = screen.wrapping_add(ui::LIST);
    let mut rows = Vec::new();
    match page {
        "picker" => rows.extend(
            Picker::new(&s.built)
                .rows
                .into_iter()
                .take(super::MAX_ROWS)
                .map(|(id, _)| key(id)),
        ),
        "options" => rows
            .extend((0..4).map_while(|row| options::label(options::action(m, list, row).ok()??))),
        _ => {}
    }
    let focus = if rows.is_empty() {
        None
    } else {
        focused_row_handle(m, channel, list).map(|row| usize::from(row as u16))
    }
    .filter(|&r| r < rows.len());
    let selected =
        matches!(page, "options" | "levels" | "difficulty").then(|| key(s.selection.campaign()));
    let pick = matches!(page, "picker" | "options")
        .then(|| *s.selection.pick.lock().unwrap_or_else(|e| e.into_inner()))
        .flatten();
    let list_state = match page {
        "options" => Some(list_state(m, screen, ui::CAMPAIGN_OPTIONS_PRESS)),
        "picker" => Some(list_state(m, screen, ui::CONTROLS_PRESS)),
        _ => None,
    };
    json!({"installed": true, "page": page, "game": selected, "rows": rows,
        "focused_row": focus, "focused": focus.and_then(|i| rows.get(i)),
        "pick": pick, "list": list_state})
}

fn list_state(m: &dyn GuestMemory, screen: u32, press_offset: u32) -> Value {
    let list = ui::Screen::at(screen).list();
    let array = list.rows(m).ok().filter(|&p| p != 0);
    let row_count = array.and_then(|p| DatumArray::at(p).header_field(m, ACTIVE_COUNT).ok());
    let items = crate::speaker_row::children(&ReadGm(m), list.addr);
    let press = list.press(press_offset);
    json!({
        "screen": screen, "handler": list.addr, "row_array": array, "row_count": row_count,
        "children": items, "press_record": press.addr, "press_target": press.target(m).ok(),
        "press_function": press.function(m).ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    #[test]
    fn shows_the_pick_request_and_the_options_list_as_built() {
        use crate::campaign_menu::PickerRows;
        use crate::list_class::Rows;
        use std::sync::Arc;

        let state = Arc::new(super::super::tests::state());
        let picker = PickerRows::new(Arc::clone(&state), None);
        let mut m = FakeGuest::new();
        let (channel, screen, stack) = (0x1000, 0x2000, 0x3000);
        m.map(screen, 0x1000);
        let before = inspect_menu(&m, &state, channel, screen, true);
        assert!(before["pick"].is_null());
        assert_eq!(
            before["list"]["press_record"],
            screen + ui::LIST + ui::CONTROLS_PRESS
        );
        let _ = picker.press(1);
        let pick = inspect_menu(&m, &state, channel, screen, true);
        assert_eq!(pick["pick"], json!({"campaign": 1, "request": null}));

        let loader = super::super::OPTIONS_LOADER;
        m.put_u32(stack + 4, 1)
            .put_u32(stack + 8, 5)
            .put_u32(stack + 12, 4)
            .put_u32(stack + 16, 0x5000);
        observe_options_request(&m, &state, stack);
        assert!(inspect_menu(&m, &state, channel, screen, true)["pick"]["request"].is_null());
        m.put_u32(stack + 16, loader);
        observe_options_request(&m, &state, stack);

        m.put_u32(screen + 0x70, 0xda);
        let list = ui::Screen::at(screen).list();
        let array = DatumArray::at(0x4000);
        let press = list.press(ui::CAMPAIGN_OPTIONS_PRESS);
        let [a, b, c] = [0, 1, 2].map(|i| list.item(i).addr);
        m.put_u32(list.addr + 0x70, array.addr)
            .put_u32(array.addr + ACTIVE_COUNT, 4)
            .put_u32(list.addr + 0x14, a)
            .put_u32(a + 0x18, b)
            .put_u32(b + 0x18, c)
            .put_u32(press.addr + 0x10, list.addr)
            .put_u32(
                press.addr + 0x14,
                engine::points::CAMPAIGN_OPTION_CHOSEN.base,
            );
        let menu = inspect_menu(&m, &state, channel, screen, false);
        assert_eq!(
            menu["pick"]["request"],
            json!({"mask": 1, "window": 5, "slot": 4, "loader": loader})
        );
        assert_eq!(menu["list"]["row_count"], 4);
        assert_eq!(menu["list"]["children"], json!([a, b, c]));
        assert_eq!(menu["list"]["press_record"], press.addr);
        assert_eq!(menu["list"]["press_target"], list.addr);
        assert_eq!(
            menu["list"]["press_function"],
            engine::points::CAMPAIGN_OPTION_CHOSEN.base
        );

        state.selection.set_seen(Default::default());
        assert!(inspect_menu(&m, &state, channel, screen, true)["pick"].is_null());
    }

    #[test]
    fn reports_focused_game_then_the_chosen_games_actions() {
        let s = super::super::tests::state();
        let mut m = FakeGuest::new();
        let (ch, screen, item) = (0x1000, 0x2000, 0x3000);
        m.map(ch, 0x1000).map(screen, 0x1000).map(item, 0x1000);
        m.put_u32(ch + 0x34, item)
            .put_u32(item + 0x10, screen + ui::LIST);
        m.set_u16(item + 0x70, 1).unwrap();
        let picker = inspect_menu(&m, &s, ch, screen, true);
        assert_eq!(picker["focused"], "halo2");
        assert!(picker["game"].is_null());
        s.selection.set(2);
        m.put_u32(screen + 0x70, 0xda);
        let array = engine::datum::DatumArray::at(0x4000);
        m.map(array.addr, 0x1000)
            .put_u32(screen + ui::LIST + 0x70, array.addr);
        array
            .initialize_contiguous(&mut m, 0x4100, 4, 4, 2, 0)
            .unwrap();
        array.make_valid(&mut m).unwrap();
        for id in [5, 6, 4] {
            let row = array.new_datum(&mut m).unwrap().unwrap();
            let at = array.element_addr(&m, i32::from(row.index())).unwrap();
            m.set_u16(at + 2, id).unwrap();
        }
        let menu = inspect_menu(&m, &s, ch, screen, false);
        assert_eq!(menu["page"], "options");
        assert_eq!(menu["game"], "halo1");
        assert_eq!(menu["rows"], json!(["new_game", "select_level", "co_op"]));
        assert_eq!(menu["focused"], "select_level");
    }
}
