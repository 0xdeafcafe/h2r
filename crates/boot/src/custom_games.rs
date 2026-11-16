//! Custom games over Live (docs/live-custom-games.md): a "Custom Games" row
//! on the Xbox Live menu that opens the retail System Link browser in its
//! dormant Live mode, and the party privacy the title already has, carried
//! to the server as the session's visibility.
//!
//! **The browser.** The System Link game browser (screen `0x0d`, 0x940
//! bytes, constructor `0x253221`) has two loaders that differ only in the
//! byte at `screen + 0x93c`: `0x253185` (LAN broadcast, what the main menu
//! opens) and `0x2531c9` (Live), which nothing in retail opens. In Live mode
//! the screen searches with `XOnlineMatchSearch` (procedure 1, 50 results,
//! `0x8fa80`), keeps seven-column rows (`0x8fb30`), probes each host with
//! `XNetQosLookup` and decodes the host's QoS listener data into the same
//! game description the LAN reply carries. The HLE already answers all of
//! those from the server's session list, so the screen fills with the
//! sessions the server lists for this player.
//!
//! **The row.** The Xbox Live menu (screen `0xba`, list handler at
//! `screen + 0x818`, constructor `0x230ea2`) builds a four-row datum array
//! named "xbox live menu list" (`0x458c80`) through the shared constructor
//! (`0x1a480d`, count in EAX), draws a row with `0x230f36` (stdcall, item
//! at `[esp+4]`, row at `[item+0x70]`, `ret 8`; past row 3 it sets the null
//! id) and handles a press in `0x230f92` (`ret 8`, a pointer to the row at
//! `[esp+8]`, `[[esp+4]+4]` the player index). We grow the count to five,
//! draw row 4 with our text, and for a press on row 4 open the Live browser
//! with `0x149f1e(1 << player, 0x2531c9)`, the request the System Link path
//! builds at `0x252951`. The handler embeds four item widgets (`+0x88`), so
//! the list is expected to scroll to the fifth row as lists with more rows
//! than items do (likely; unconfirmed in game).
//!
//! **Privacy.** The party privacy screen (screen `0x19`, list "squad privacy
//! setting list") offers 0 open, 1 friends, 2 invite only (the last two only
//! signed in), and its press calls `0x19a148` with the item id in EAX, which
//! stores it in the squad session at `+0x498c` (`0x64230`, `0x5bde0`). The
//! host's join check (`0x62fe0`) refuses everyone above 1; the server's
//! lists do the rest. We follow the press into the `live.visibility`
//! setting (which updates an open lobby through the HLE) and give the HLE
//! a probe that reads the squad session's privacy whenever it advertises.
//!
//! The logic is over [`Gm`] and tested without a guest; the hooks are
//! registered by `layout_glue::install` (the row: it shares the list
//! constructor and the string lookup) and [`install_privacy`].

use std::sync::Arc;

use crate::layout_glue::{Gm, ID_BASE};
use crate::setting_defs::{
    LIVE_VISIBILITY, VISIBILITY_FRIENDS, VISIBILITY_INVITE, VISIBILITY_PUBLIC,
};

/// The Xbox Live menu's list name, retail row count and our row.
pub const LIST_NAME: u32 = 0x0045_8c80;
pub const VANILLA_ROWS: u32 = 4;
pub const ROW: i16 = 4;
/// Its row text (stdcall, `ret 8`) and row chosen (`ret 8`) functions.
pub const ROW_TEXT: u32 = 0x0023_0f36;
pub const ROW_CHOSEN: u32 = 0x0023_0f92;
/// The System Link browser's Live-mode loader (stdcall, request, `ret 4`).
pub const LIVE_BROWSER_LOADER: u32 = 0x0025_31c9;
/// Our row's label.
pub const TEXT_ID: u32 = ID_BASE + 0x32;
pub const TEXT: &str = "Custom Games";

/// The party privacy press (`0x2b533e` calls it, item id in EAX).
pub const PRIVACY_CHOSEN: u32 = 0x0019_a148;
/// The network life cycle's ready byte and the squad session pointer.
const NETWORK_READY: u32 = 0x0052_7330;
const SQUAD_SESSION: u32 = 0x0052_7364;
const SESSION_STATE: u32 = 0x741c;
const SESSION_PRIVACY: u32 = 0x498c;

/// Whether the row is on (`H2_NO_CUSTOM_GAMES_ROW=1` takes it out).
pub fn row_enabled() -> bool {
    std::env::var_os("H2_NO_CUSTOM_GAMES_ROW").is_none()
}

/// The Live menu's count when it is built with `count` rows: one more for
/// our row, only for that list at its retail size.
pub fn list_count(name: u32, count: u32) -> u32 {
    if name == LIST_NAME && count == VANILLA_ROWS {
        count + 1
    } else {
        count
    }
}

/// The `live.visibility` value of a privacy item id, as `0x19a148` maps
/// it: 0 open, 1 friends, anything else invite only.
pub fn visibility_of_privacy(id: u32) -> i64 {
    match id {
        0 => VISIBILITY_PUBLIC,
        1 => VISIBILITY_FRIENDS,
        _ => VISIBILITY_INVITE,
    }
}

/// The squad session's privacy as a `live.visibility` value, when there is
/// a session to read: the same gates as `0x64230`/`0x64260` (life cycle
/// ready, a session, state 3 to 8).
pub fn squad_visibility<M: Gm>(m: &M) -> Option<i64> {
    if m.u8(NETWORK_READY)? == 0 {
        return None;
    }
    let session = m.u32(SQUAD_SESSION)?;
    if session == 0 {
        return None;
    }
    let state = m.u32(session.wrapping_add(SESSION_STATE))? as i32;
    if !(3..=8).contains(&state) {
        return None;
    }
    Some(visibility_of_privacy(
        m.u32(session.wrapping_add(SESSION_PRIVACY))?,
    ))
}

/// Follow the party privacy screen into the `live.visibility` setting (and
/// so the HLE and an open lobby), and let the HLE read the squad session's
/// privacy whenever it advertises. Live off: nothing.
pub fn install_privacy(
    rt: &runtime::Runtime,
    hle: Option<&Arc<live_hle::Hle>>,
    store: Option<&Arc<settings::Store>>,
) -> anyhow::Result<()> {
    let Some(hle) = hle else { return Ok(()) };
    if let Some(store) = store {
        crate::live_glue::bind_visibility(hle, store);
    }
    hle.set_visibility_probe(Some(Arc::new(|g: &runtime::Guest| {
        squad_visibility(g).map(crate::live_glue::visibility)
    })));
    let Some(o) = rt.resolve_original(PRIVACY_CHOSEN) else {
        eprintln!("custom games: no generated entry at {PRIVACY_CHOSEN:#x}; privacy not followed");
        return Ok(());
    };
    let o = Arc::new(o);
    let store = store.cloned();
    let hle = Arc::downgrade(hle);
    rt.edit_dispatch(|d| {
        d.register_override(
            PRIVACY_CHOSEN,
            "custom-games:privacy",
            move |cpu, ctx, _| {
                let v = visibility_of_privacy(cpu.eax);
                // Guest memory is not held here: the store's callbacks reach the
                // HLE, whose lock comes after guest memory.
                match &store {
                    Some(s) => match s.set(LIVE_VISIBILITY.key, settings::Value::Int(v)) {
                        Ok(o) => eprintln!("{}", o.describe(&LIVE_VISIBILITY)),
                        Err(e) => eprintln!("live.visibility: {e}"),
                    },
                    None => {
                        if let Some(h) = hle.upgrade() {
                            h.set_session_visibility(crate::live_glue::visibility(v));
                        }
                    }
                }
                Ok(o.invoke(cpu, ctx, PRIVACY_CHOSEN))
            },
        );
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_glue::{chosen_row, is_ours};
    use crate::video_screen::{player_mask, text_row};
    use std::collections::HashMap;

    #[derive(Default)]
    struct Fake(HashMap<u32, u8>);
    impl Gm for Fake {
        fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
            for (i, b) in out.iter_mut().enumerate() {
                match self.0.get(&(addr + i as u32)) {
                    Some(v) => *b = *v,
                    None => return false,
                }
            }
            true
        }
        fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
            for (i, b) in data.iter().enumerate() {
                self.0.insert(addr + i as u32, *b);
            }
            true
        }
    }

    #[test]
    fn only_the_live_menu_grows_by_one() {
        assert_eq!(list_count(LIST_NAME, 4), 5);
        assert_eq!(list_count(LIST_NAME, 5), 5, "not twice");
        assert_eq!(list_count(LIST_NAME, 3), 3, "not another size");
        assert_eq!(list_count(crate::speaker_row::LIST_NAME, 4), 4);
        assert!(is_ours(TEXT_ID));
    }

    #[test]
    fn the_row_hooks_read_the_retail_arguments() {
        // Row text: the item at [esp+4], its row at +0x70.
        let mut m = Fake::default();
        let esp = 0x1000;
        m.set_u32(esp + 4, 0x2000);
        m.set_u16(0x2000 + 0x70, ROW as u16);
        assert_eq!(text_row(&m, esp), Some(ROW));
        // Row chosen: the row word through [esp+8], the player through
        // [[esp+4]+4], the mask the browser is opened for.
        m.set_u32(esp + 8, 0x3000);
        m.set_u16(0x3000, ROW as u16);
        m.set_u32(esp + 4, 0x4000);
        m.set_u32(0x4000, 0x5000);
        m.set_u32(0x5000 + 4, 2);
        assert_eq!(chosen_row(&m, esp), Some(ROW));
        assert_eq!(player_mask(&m, esp), Some(1 << 2));
    }

    #[test]
    fn privacy_maps_like_the_title_and_reads_only_a_live_squad() {
        assert_eq!(visibility_of_privacy(0), VISIBILITY_PUBLIC);
        assert_eq!(visibility_of_privacy(1), VISIBILITY_FRIENDS);
        assert_eq!(visibility_of_privacy(2), VISIBILITY_INVITE);
        assert_eq!(visibility_of_privacy(7), VISIBILITY_INVITE);
        let mut m = Fake::default();
        assert_eq!(squad_visibility(&m), None, "nothing mapped");
        m.wr(NETWORK_READY, &[0]);
        assert_eq!(squad_visibility(&m), None, "life cycle not ready");
        m.wr(NETWORK_READY, &[1]);
        m.set_u32(SQUAD_SESSION, 0);
        assert_eq!(squad_visibility(&m), None, "no session");
        let s = 0x10_0000;
        m.set_u32(SQUAD_SESSION, s);
        m.set_u32(s + SESSION_PRIVACY, 1);
        for (state, want) in [
            (2, None),
            (3, Some(VISIBILITY_FRIENDS)),
            (5, Some(VISIBILITY_FRIENDS)),
            (8, Some(VISIBILITY_FRIENDS)),
            (9, None),
        ] {
            m.set_u32(s + SESSION_STATE, state);
            assert_eq!(squad_visibility(&m), want, "state {state}");
        }
        m.set_u32(s + SESSION_STATE, 5);
        m.set_u32(s + SESSION_PRIVACY, 2);
        assert_eq!(squad_visibility(&m), Some(VISIBILITY_INVITE));
    }

    #[test]
    fn privacy_install_without_live_or_game_is_harmless() {
        let rt = runtime::Runtime::new(
            runtime::Guest::new(),
            runtime::dispatch::Dispatch::default(),
        );
        install_privacy(&rt, None, None).unwrap();
        let hle = live_hle::Hle::with_address(
            live_hle::LiveConfig {
                enabled: true,
                ..Default::default()
            },
            "127.0.0.1:3074".parse().unwrap(),
        )
        .unwrap();
        let store = Arc::new(settings::Store::defaults(crate::setting_defs::ALL));
        store
            .set(
                LIVE_VISIBILITY.key,
                settings::Value::Int(VISIBILITY_FRIENDS),
            )
            .unwrap();
        install_privacy(&rt, Some(&hle), Some(&store)).unwrap();
        assert_eq!(hle.session_visibility(), live_hle::Visibility::Friends);
    }
}
