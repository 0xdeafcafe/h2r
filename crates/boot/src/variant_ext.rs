//! The game variant's h2recomp options in a running multiplayer game: the
//! extension block in the active record's spare bytes
//! (`G+0x13c + 0x10c`; [`variants::ext`], docs/game/game-options.md
//! section 3). Slice 3 applies gravity, through the Low Gravity code in
//! [`crate::skulls_custom`]; the other options arrive with their slices.
//!
//! **Who applies it.** A machine alone, the host of a Live game, and a
//! Live guest whose block came from its host. Never on System Link
//! (decided 6a: extras are off there, so every machine plays the game's
//! own rules).
//!
//! **How a guest gets it** ([`Transport`]). The game's wire drops the
//! spare bytes, so a guest's record has none of its own. At each
//! multiplayer start (`GAME_ACTIVATE`, the options in EAX, before the game
//! copies them into the globals) a Live host puts its record's block and
//! the variant's [`key`] on its session (`NewSession.variant_ext`), and a
//! Live guest asks the server for its host's session until the key there
//! is its own record's (the host may publish a moment after the guest
//! starts; at most [`HOST_WAIT`]), then writes the block into the options
//! the game is about to copy. A host whose build sends nothing (an older
//! one) leaves the field out: the guest says so and plays the game's own
//! rules.
//!
//! **Older guests.** Each console's presence carries its build and its
//! capabilities ([`live_proto::capability::VARIANT_EXT`] for this one). A
//! Live host starting a game with options asks the server for its guests'
//! presence ([`live_hle::Hle::players_lacking`], at most [`OLDER_WAIT`])
//! and names each guest whose build lacks it: that guest plays the
//! game's own rules against a host with extras. The guests' XUIDs come
//! from the squad session's players ([`session_guests`]) until the Live
//! session's roster names them.
//!
//! The decision is a pure function over guest memory ([`Gm`]), read each
//! tick after the game's (cheap: the session is read only when the block
//! asks for something).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use feature::{BuildError, Builder, Flow, Manifest};
use live_hle::HostVariantExt;
use live_proto::session::VariantExt;
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
/// The active options' variant record (`O + 0x134`; the options pointer is
/// EAX at `GAME_ACTIVATE`).
pub const OPTIONS_VARIANT: u32 = 0x134;
/// How long a guest's start waits for its host's options.
pub const HOST_WAIT: Duration = Duration::from_millis(1500);
/// How long a host's start waits for its guests' presence.
pub const OLDER_WAIT: Duration = Duration::from_millis(500);

/// The squad session's players (docs/game/bots.md, "Session S"): a mask of
/// the 16 slots in use, then the slots, each with a 12-byte id (the XUID,
/// then the machine id's last four) and its peer.
pub mod players {
    pub const MASK: u32 = 0x111c;
    pub const AT: u32 = 0x1120;
    pub const STRIDE: u32 = 0x13c;
    pub const XUID: u32 = 0;
    pub const PEER: u32 = 0xc;
    pub const SLOTS: u32 = 16;
}

/// The XUIDs of the session's players on other machines than this one
/// (the host's guests), in slot order. Zero XUIDs (no account) are left
/// out.
pub fn session_guests<M: Gm + ?Sized>(m: &M, s: &Session) -> Vec<live_proto::Xuid> {
    let Some(mask) = m.u32(s.addr.wrapping_add(players::MASK)) else {
        return Vec::new();
    };
    (0..players::SLOTS)
        .filter(|i| mask & (1u32 << i) != 0)
        .filter_map(|i| {
            let at = s.addr.wrapping_add(players::AT + i * players::STRIDE);
            let peer = m.u32(at.wrapping_add(players::PEER))?;
            let mut x = [0u8; 8];
            m.rd(at.wrapping_add(players::XUID), &mut x).then_some(())?;
            let xuid = u64::from_le_bytes(x);
            (peer != s.local_peer && xuid != 0).then_some(live_proto::Xuid(xuid))
        })
        .collect()
}

/// The host's lines about guests whose build can't apply its options.
pub fn older_lines(lacking: &live_hle::Lacking) -> Vec<String> {
    match lacking {
        live_hle::Lacking::Known(tags) => tags
            .iter()
            .map(|t| {
                format!(
                    "variant options: {t} plays this variant with the game's own rules (older build)"
                )
            })
            .collect(),
        live_hle::Lacking::Unknown(why) => {
            vec![format!("variant options: the guests' builds are not known: {why}")]
        }
    }
}

/// The block this machine took from its host for the game running now
/// (a guest), so [`in_game`] applies it.
static FROM_HOST: Mutex<Option<[u8; ext::LEN]>> = Mutex::new(None);

fn from_host() -> Option<[u8; ext::LEN]> {
    *FROM_HOST.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_from_host(b: Option<[u8; ext::LEN]>) {
    *FROM_HOST.lock().unwrap_or_else(|e| e.into_inner()) = b;
}

/// Which variant a record is, the same on the host and its guests: FNV-1a
/// over the name (`+4`, 0x40 bytes) and the engine (`+0x44`), which the
/// game's wire carries whole. Two variants alike in both share a key.
pub fn key(record: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in record.get(0x04..0x48).unwrap_or(&[]) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

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
/// network class at `+0x18`); `host_sent` when a guest's block came from
/// its host.
pub fn decide(e: Ext, session: Option<(&Session, u32)>, host_sent: bool) -> Decision {
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
            } else if host_sent {
                Decision::Apply {
                    ext: e,
                    who: "guest, the host's",
                }
            } else {
                Decision::Off {
                    ext: e,
                    why: "a guest applies only what its host sent through Live",
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
    decide(e, s.as_ref().zip(class), from_host() == Some(b))
}

/// What a guest does with what it heard of its host's options, for a
/// record whose key is `own`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    /// Write this block (all zero: the host's variant has none).
    Take([u8; ext::LEN]),
    /// Not yet: the host's session names another variant (its last game's).
    Wait,
    /// Nothing will come: why.
    Nothing(String),
}

pub fn heard(h: &HostVariantExt, own: u64) -> Heard {
    match h {
        HostVariantExt::NotJoined => Heard::Nothing("no joined Live session".into()),
        HostVariantExt::Unknown { session } => {
            Heard::Nothing(format!("session {session} was never read from the server"))
        }
        HostVariantExt::Known { ext: None, .. } => Heard::Nothing(
            "the host's build sends no variant options (an older build): the game's own rules"
                .into(),
        ),
        HostVariantExt::Known { ext: Some(v), .. } => {
            match (v.bytes(), u64::from_str_radix(&v.key, 16)) {
                (Some(b), Ok(k)) if k == own => Heard::Take(b),
                (Some(_), Ok(_)) => Heard::Wait,
                _ => Heard::Nothing("the host's variant options are malformed".into()),
            }
        }
    }
}

/// This machine's part at a multiplayer start, from the squad session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// No networked game: nothing to send or take.
    Alone,
    /// System Link: extras off.
    SystemLink,
    LiveHost,
    LiveGuest,
}

pub fn part(session: Option<(&Session, u32)>) -> Part {
    match session {
        Some((s, class)) if s.live() && s.peers > 1 => {
            if class == SYSTEM_LINK {
                Part::SystemLink
            } else if s.is_host() {
                Part::LiveHost
            } else {
                Part::LiveGuest
            }
        }
        _ => Part::Alone,
    }
}

/// Asks `ask` (the HLE, waiting at most its argument) until the host's
/// options are for this variant, for at most `budget`.
pub fn wait_for_host(
    own: u64,
    budget: Duration,
    ask: impl Fn(Duration) -> HostVariantExt,
) -> Heard {
    let until = Instant::now() + budget;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        match heard(&ask(left.min(Duration::from_millis(500))), own) {
            Heard::Wait if !left.is_zero() => std::thread::sleep(Duration::from_millis(100)),
            Heard::Wait => {
                return Heard::Nothing(format!(
                    "the host's session named another variant for {} ms",
                    budget.as_millis()
                ));
            }
            h => return h,
        }
    }
}

/// The feature id (`features.variant-ext-live` switches it).
pub const FEATURE: &str = "variant-ext-live";

/// Sends a Live host's options to its guests and takes them on a guest,
/// at each multiplayer start.
pub struct Transport {
    pub hle: Option<Arc<live_hle::Hle>>,
}

impl feature::Feature for Transport {
    fn manifest(&self) -> Manifest {
        Manifest::new(FEATURE, "Game variant options sent to Live guests")
            .doc("docs/game/game-options.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use feature::points::GAME_ACTIVATE;
        let hle = self.hle.clone();
        b.raw()
            .before(&GAME_ACTIVATE, "the host's variant options", move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let options = c.cpu.eax;
                let (mode, record, session, guests) = {
                    let mem = rt.memory();
                    let mut record = [0u8; 0x130];
                    let ok = Gm::rd(&*mem, options.wrapping_add(OPTIONS_VARIANT), &mut record);
                    let s = coop_probe::read_session(&*mem);
                    let class = s
                        .as_ref()
                        .and_then(|s| Gm::u32(&*mem, s.addr.wrapping_add(SESSION_NETWORK_CLASS)));
                    let guests = s
                        .as_ref()
                        .map(|s| session_guests(&*mem, s))
                        .unwrap_or_default();
                    (
                        crate::skulls::options_mode(&*mem, options),
                        ok.then_some(record),
                        s.zip(class),
                        guests,
                    )
                };
                set_from_host(None);
                let (Some(crate::skulls_mp::MODE_MULTIPLAYER), Some(record)) = (mode, record)
                else {
                    return Ok(Flow::Continue);
                };
                let mut block = [0u8; ext::LEN];
                block.copy_from_slice(&record[ext::AT..ext::AT + ext::LEN]);
                let own = key(&record);
                let mine = Ext::in_effect(&block);
                match part(session.as_ref().map(|(s, c)| (s, *c))) {
                    Part::Alone => {}
                    Part::SystemLink => {
                        if !mine.is_default() {
                            eprintln!(
                                "variant options: {} off: System Link plays the game's own rules",
                                mine.summary()
                            );
                        }
                    }
                    Part::LiveHost => {
                        if let Some(h) = &hle {
                            // Every start, extras or not: a guest waits for this
                            // game's key, so a stale one must not linger.
                            h.set_session_variant_ext(Some(VariantExt::new(&block, own)));
                            eprintln!(
                                "variant options: sent to guests: {} (key {own:016x})",
                                mine.summary()
                            );
                            if !mine.is_default() {
                                let lacking = h.players_lacking(
                                    live_proto::capability::VARIANT_EXT,
                                    &guests,
                                    OLDER_WAIT,
                                );
                                for line in older_lines(&lacking) {
                                    eprintln!("{line}");
                                }
                            }
                        }
                    }
                    Part::LiveGuest => {
                        let Some(h) = &hle else {
                            return Ok(Flow::Continue);
                        };
                        match wait_for_host(own, HOST_WAIT, |w| h.host_variant_ext(w)) {
                            Heard::Take(b) => {
                                if b != block {
                                    let mut mem = rt.memory();
                                    let at = options
                                        .wrapping_add(OPTIONS_VARIANT)
                                        .wrapping_add(ext::AT as u32);
                                    if !Gm::wr(&mut *mem, at, &b) {
                                        eprintln!(
                                            "variant options: the host's could not be written"
                                        );
                                        return Ok(Flow::Continue);
                                    }
                                }
                                set_from_host(Some(b));
                                eprintln!(
                                    "variant options: the host's for this game: {}",
                                    Ext::in_effect(&b).summary()
                                );
                            }
                            Heard::Wait => {}
                            Heard::Nothing(why) => {
                                eprintln!("variant options: none from the host: {why}")
                            }
                        }
                    }
                }
                Ok(Flow::Continue)
            });
        Ok(())
    }
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
        assert_eq!(decide(Ext::default(), None, false), Decision::Retail);
        assert_eq!(decide(e, None, false).gravity(), 0.5, "no session: local");
        let solo = session(1, true);
        assert_eq!(
            decide(e, Some((&solo, 2)), false).gravity(),
            0.5,
            "one machine"
        );
        let host = session(3, true);
        assert!(matches!(
            decide(e, Some((&host, 2)), false),
            Decision::Apply { who: "host", .. }
        ));
        assert!(matches!(
            decide(e, Some((&host, SYSTEM_LINK)), false),
            Decision::Off { .. }
        ));
        let guest = session(3, false);
        assert!(matches!(
            decide(e, Some((&guest, 2)), false),
            Decision::Off { .. }
        ));
        assert_eq!(decide(e, Some((&guest, 2)), false).gravity(), 1.0);
        assert!(
            decide(e, Some((&host, SYSTEM_LINK)), false)
                .line()
                .contains("System Link")
        );
    }

    #[test]
    fn a_live_guest_applies_only_what_its_host_sent() {
        let e = half();
        let guest = session(3, false);
        assert!(matches!(
            decide(e, Some((&guest, 2)), true),
            Decision::Apply {
                who: "guest, the host's",
                ..
            }
        ));
        // System Link never, whatever was heard.
        assert!(matches!(
            decide(e, Some((&guest, SYSTEM_LINK)), true),
            Decision::Off { .. }
        ));
        assert_eq!(part(None), Part::Alone);
        assert_eq!(part(Some((&session(1, true), 2))), Part::Alone);
        assert_eq!(part(Some((&session(3, true), 2))), Part::LiveHost);
        assert_eq!(part(Some((&guest, 2))), Part::LiveGuest);
        assert_eq!(part(Some((&guest, SYSTEM_LINK))), Part::SystemLink);
    }

    #[test]
    fn the_key_is_the_name_and_engine_only() {
        let mut a = [0u8; 0x130];
        a[4..8].copy_from_slice(&[b'S', 0, b'l', 0]);
        a[0x44] = 2;
        let mut b = a;
        b[0x48] = 0xff; // a field past the engine
        b[0x10c..0x130].fill(7); // the block itself
        assert_eq!(key(&a), key(&b));
        b[0x44] = 3;
        assert_ne!(key(&a), key(&b));
        let mut c = a;
        c[6] = b'L';
        assert_ne!(key(&a), key(&c));
    }

    #[test]
    fn a_guest_waits_for_its_hosts_key_and_says_why_not() {
        let block = half().encode();
        let known = |k: u64| HostVariantExt::Known {
            session: "s".into(),
            ext: Some(VariantExt::new(&block, k)),
            refreshed: true,
        };
        assert_eq!(heard(&known(7), 7), Heard::Take(block));
        assert_eq!(heard(&known(8), 7), Heard::Wait);
        let older = HostVariantExt::Known {
            session: "s".into(),
            ext: None,
            refreshed: true,
        };
        assert!(matches!(heard(&older, 7), Heard::Nothing(w) if w.contains("older build")));
        assert!(matches!(
            heard(&HostVariantExt::NotJoined, 7),
            Heard::Nothing(_)
        ));
        let bad = HostVariantExt::Known {
            session: "s".into(),
            ext: Some(VariantExt {
                block: "00".into(),
                key: "7".into(),
            }),
            refreshed: true,
        };
        assert!(matches!(heard(&bad, 7), Heard::Nothing(w) if w.contains("malformed")));
        // The host publishes on its third answer: the guest waits for it.
        let asked = std::sync::atomic::AtomicU32::new(0);
        let got = wait_for_host(7, Duration::from_secs(5), |_| {
            let n = asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            known(if n < 2 { 6 } else { 7 })
        });
        assert_eq!(got, Heard::Take(block));
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 3);
        // A host stuck on another variant: the guest gives up.
        let got = wait_for_host(7, Duration::from_millis(250), |_| known(6));
        assert!(matches!(got, Heard::Nothing(w) if w.contains("another variant")));
    }

    #[test]
    fn a_guest_with_its_hosts_block_applies_it_in_game() {
        use coop_probe::layout as l;
        use engine::points::{NETWORK_READY, SQUAD_SESSION};
        const S: u32 = 0x9000_0000;
        let mut f = game(crate::skulls_mp::MODE_MULTIPLAYER, &half());
        f.wr(S, &vec![0; 0x7420]);
        f.wr(NETWORK_READY.base, &[1]);
        f.set_u32(SQUAD_SESSION.base, S);
        f.set_u32(S + l::STATE, 5);
        f.set_u32(S + l::PEERS, 2);
        f.set_u32(S + l::HOST_PEER, 1);
        f.set_u32(S + SESSION_NETWORK_CLASS, 2);
        set_from_host(None);
        assert_eq!(in_game(&f).gravity(), 1.0, "not from the host");
        set_from_host(Some(half().encode()));
        assert_eq!(in_game(&f).gravity(), 0.5);
        set_from_host(None);
    }

    #[test]
    fn the_host_names_its_guests_from_the_sessions_players() {
        const S: u32 = 0x9000_0000;
        let mut f = Fake::default();
        f.wr(S, &vec![0; 0x7420]);
        let s = Session {
            addr: S,
            state: 5,
            peers: 3,
            local_peer: 0,
            host_peer: 0,
            ..Session::default()
        };
        let player = |f: &mut Fake, slot: u32, xuid: u64, peer: u32| {
            let at = S + players::AT + slot * players::STRIDE;
            f.wr(at + players::XUID, &xuid.to_le_bytes());
            f.wr(at + players::XUID + 8, &[0xaa; 4]); // the machine id's end
            f.set_u32(at + players::PEER, peer);
        };
        player(&mut f, 0, 0x0009_0000_0000_0001, 0); // the host
        player(&mut f, 1, 0x0009_0000_0000_0002, 1);
        player(&mut f, 3, 0x0009_0000_0000_0003, 2);
        player(&mut f, 4, 0, 2); // no account
        player(&mut f, 5, 0x0009_0000_0000_0005, 1); // a slot not in use
        f.set_u32(S + players::MASK, 0b1_1011);
        assert_eq!(
            session_guests(&f, &s),
            [
                live_proto::Xuid(0x0009_0000_0000_0002),
                live_proto::Xuid(0x0009_0000_0000_0003)
            ]
        );
        // The last slot's end is inside the session.
        assert!(players::AT + players::SLOTS * players::STRIDE <= 0x4980);

        let lines = older_lines(&live_hle::Lacking::Known(vec!["Oldie".into()]));
        assert_eq!(
            lines,
            ["variant options: Oldie plays this variant with the game's own rules (older build)"]
        );
        assert!(older_lines(&live_hle::Lacking::Known(Vec::new())).is_empty());
        assert!(older_lines(&live_hle::Lacking::Unknown("no answer"))[0].contains("no answer"));
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
