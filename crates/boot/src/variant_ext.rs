//! The game variant's h2recomp options in a running multiplayer game: the
//! extension block in the active record's spare bytes
//! (`G+0x13c + 0x10c`; [`variants::ext`], docs/game/game-options.md
//! section 3). Slice 3 applies gravity, through the Low Gravity code in
//! [`crate::skulls_custom`]; the other options arrive with their slices.
//!
//! **Who applies it.** A machine alone, or the host of a Live game. Never
//! on System Link (decided 6a: extras are off there, so every machine
//! plays the game's own rules), and never as a Live guest until the host's
//! block reaches guests through the Live session (slice 4): the game's
//! wire drops the spare bytes, so a guest's record has none of its own.
//!
//! The decision is a pure function over guest memory ([`Gm`]), read each
//! tick after the game's (cheap: the session is read only when the block
//! asks for something).

use variants::ext::{self, Ext};

use crate::coop_probe::{self, Session};
use crate::layout_glue::Gm;

/// The game globals pointer ([`crate::skulls_custom::GAME_GLOBALS`]).
const GAME_GLOBALS: u32 = crate::skulls_custom::GAME_GLOBALS;
/// The options mode in the globals (2 multiplayer).
const GLOBALS_MODE: u32 = crate::skulls_custom::GLOBALS_MODE;
/// The active variant record in the globals (docs/live/live-gamestate.md).
pub const ACTIVE_VARIANT: u32 = 0x13c;
/// The block in the globals.
pub const BLOCK: u32 = ACTIVE_VARIANT + ext::AT as u32;
/// The squad session's network class (`[session+0x18]`: 0 offline,
/// 1 System Link, 2 Live; `0x59840` answers LAN searches only for 1).
/// Not the `+0x4980` byte the lobby setters check (`0x63f00`).
pub const SESSION_NETWORK_CLASS: u32 = 0x18;
pub const SYSTEM_LINK: u32 = 1;

/// What this machine does with the active variant's block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Not a multiplayer game, or the block holds nothing (or is ignored).
    Retail,
    /// Apply these options; `who` says why this machine may.
    Apply { ext: Ext, who: &'static str },
    /// The block asks for options this machine doesn't apply; why.
    Off { ext: Ext, why: &'static str },
}

impl Decision {
    /// The gravity factor to apply (1.0 for none).
    pub fn gravity(&self) -> f32 {
        match self {
            Decision::Apply { ext, .. } => ext.gravity_scale(),
            _ => 1.0,
        }
    }

    /// The line the log prints when the decision changes.
    pub fn line(&self) -> String {
        match self {
            Decision::Retail => "variant options: none in this game".into(),
            Decision::Apply { ext, who } => {
                format!("variant options: {} ({who})", ext.summary())
            }
            Decision::Off { ext, why } => {
                format!("variant options: {} not applied: {why}", ext.summary())
            }
        }
    }
}

/// Decides for a variant's options and the squad session (`class` its
/// network class at `+0x18`).
pub fn decide(e: Ext, session: Option<(&Session, u32)>) -> Decision {
    if e.is_default() {
        return Decision::Retail;
    }
    match session {
        Some((s, class)) if s.live() && s.peers > 1 => {
            if class == SYSTEM_LINK {
                Decision::Off {
                    ext: e,
                    why: "System Link plays the game's own rules",
                }
            } else if s.is_host() {
                Decision::Apply {
                    ext: e,
                    who: "host",
                }
            } else {
                Decision::Off {
                    ext: e,
                    why: "a guest gets the host's options through Live, not built yet",
                }
            }
        }
        _ => Decision::Apply {
            ext: e,
            who: "local game",
        },
    }
}

/// The decision for the game running now.
pub fn in_game<M: Gm + ?Sized>(m: &M) -> Decision {
    let Some(g) = m.u32(GAME_GLOBALS).filter(|&g| g != 0) else {
        return Decision::Retail;
    };
    if m.u32(g.wrapping_add(GLOBALS_MODE)) != Some(crate::skulls_mp::MODE_MULTIPLAYER) {
        return Decision::Retail;
    }
    let mut b = [0u8; ext::LEN];
    if !m.rd(g.wrapping_add(BLOCK), &mut b) {
        return Decision::Retail;
    }
    let e = Ext::in_effect(&b);
    if e.is_default() {
        return Decision::Retail;
    }
    let s = coop_probe::read_session(m);
    let class = s
        .as_ref()
        .and_then(|s| m.u32(s.addr.wrapping_add(SESSION_NETWORK_CLASS)));
    decide(e, s.as_ref().zip(class))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake(BTreeMap<u32, u8>);
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

    const G: u32 = 0x8000_0000;

    fn game(mode: u32, e: &Ext) -> Fake {
        let mut f = Fake::default();
        f.set_u32(GAME_GLOBALS, G);
        f.set_u32(G + GLOBALS_MODE, mode);
        f.wr(G + ACTIVE_VARIANT, &[0; 0x130]);
        f.wr(G + BLOCK, &e.encode());
        f
    }

    fn half() -> Ext {
        Ext {
            gravity: 50,
            ..Ext::default()
        }
    }

    fn session(peers: u32, host: bool) -> Session {
        Session {
            state: 5,
            peers,
            host_peer: 0,
            local_peer: if host { 0 } else { 1 },
            ..Session::default()
        }
    }

    #[test]
    fn alone_or_hosting_applies_system_link_and_guests_dont() {
        let e = half();
        assert_eq!(decide(Ext::default(), None), Decision::Retail);
        assert_eq!(decide(e, None).gravity(), 0.5, "no session: local");
        let solo = session(1, true);
        assert_eq!(decide(e, Some((&solo, 2))).gravity(), 0.5, "one machine");
        let host = session(3, true);
        assert!(matches!(
            decide(e, Some((&host, 2))),
            Decision::Apply { who: "host", .. }
        ));
        assert!(matches!(
            decide(e, Some((&host, SYSTEM_LINK))),
            Decision::Off { .. }
        ));
        let guest = session(3, false);
        assert!(matches!(decide(e, Some((&guest, 2))), Decision::Off { .. }));
        assert_eq!(decide(e, Some((&guest, 2))).gravity(), 1.0);
        assert!(
            decide(e, Some((&host, SYSTEM_LINK)))
                .line()
                .contains("System Link")
        );
    }

    #[test]
    fn reads_the_active_records_block_in_multiplayer_only() {
        assert_eq!(in_game(&Fake::default()), Decision::Retail, "no globals");
        let f = game(crate::skulls_mp::MODE_MULTIPLAYER, &half());
        assert_eq!(
            in_game(&f),
            Decision::Apply {
                ext: half(),
                who: "local game"
            }
        );
        let f = game(1, &half());
        assert_eq!(in_game(&f), Decision::Retail, "campaign");
        let f = game(crate::skulls_mp::MODE_MULTIPLAYER, &Ext::default());
        assert_eq!(in_game(&f), Decision::Retail, "a retail record");
        // A System Link host and a Live host, read from the session.
        use coop_probe::layout as l;
        use engine::points::{NETWORK_READY, SQUAD_SESSION};
        const S: u32 = 0x9000_0000;
        for (class, applied) in [(SYSTEM_LINK, false), (2, true)] {
            let mut f = game(crate::skulls_mp::MODE_MULTIPLAYER, &half());
            f.wr(S, &vec![0; 0x7420]);
            f.wr(NETWORK_READY.base, &[1]);
            f.set_u32(SQUAD_SESSION.base, S);
            f.set_u32(S + l::STATE, 5);
            f.set_u32(S + l::PEERS, 2);
            f.set_u32(S + SESSION_NETWORK_CLASS, class);
            assert_eq!(in_game(&f).gravity() == 0.5, applied, "class {class}");
            // The guest of the Live game.
            f.set_u32(S + l::HOST_PEER, 1);
            assert_eq!(in_game(&f).gravity(), 1.0);
        }
        // A damaged block is ignored.
        let mut f = game(crate::skulls_mp::MODE_MULTIPLAYER, &half());
        f.wr(G + BLOCK + 0x20, &[1, 2, 3, 4]);
        assert_eq!(in_game(&f), Decision::Retail);
    }
}
