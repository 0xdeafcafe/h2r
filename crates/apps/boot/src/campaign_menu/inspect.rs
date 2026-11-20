//! The shared player-one menu channel, read at an agent tick boundary.
use engine::GuestMemory;
use engine::view::ui;
use serde_json::{Value, json};

use super::{PREPARED, Picker, SCREEN, State, options};
use crate::layout_glue::ReadGm;
use crate::ui_kit::lifecycle::window_object;

pub(crate) fn inspect(m: &dyn GuestMemory) -> Value {
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
        focused(m, channel, list)
    }
    .filter(|&r| r < rows.len());
    let selected =
        matches!(page, "options" | "levels" | "difficulty").then(|| key(s.selection.campaign()));
    json!({"installed": true, "page": page, "game": selected, "rows": rows,
        "focused_row": focus, "focused": focus.and_then(|i| rows.get(i))})
}

// 0x24c447 returns the datum at focused item +0x70. 0x22ed7a
// walks the channel's focused leaf (+0x34) up its parents (+0x10).
fn focused(m: &dyn GuestMemory, channel: u32, list: u32) -> Option<usize> {
    let mut child = m.u32(channel + 0x34).ok()?;
    for _ in 0..32 {
        if child == 0 {
            return None;
        }
        let parent = m.u32(child.checked_add(0x10)?).ok()?;
        if parent == list {
            return Some(usize::from(m.u16(child.checked_add(0x70)?).ok()?));
        }
        child = parent;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

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
