//! Lobby steps: presses on player one's pad (`input.pad`), each checked
//! against the screen `ui.screen` reads (`H2_UI_WATCH=1`), with a timeout.
//! They run on the client, since a step takes many ticks and screens.
//!
//! The screen ids and rows are the ones `h2 coop test --drive` presses on
//! (docs/live/live-coop.md 12.1 and 12.6, `boot::ui_watch`), kept here so
//! both use one table. The recipes assume what the driver assumes: a menu
//! opens with its cursor on row 0. A wrong row shows as the wrong screen,
//! and the step fails saying what it saw.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Rpc;
use crate::{RpcError, code, method};

/// Screen ids (docs/live/live-coop.md 12.1, `custom_games::SCREEN_ID`,
/// the System Link browser in Live mode).
pub const MAIN_MENU: u32 = 0x09;
pub const LIVE_MENU: u32 = 0xba;
pub const BROWSER: u32 = 0x0d;
/// The players' screen (`boot::ui_watch::JOIN_SCREEN`) and its modes.
pub const JOIN_SCREEN: u32 = 0x1e;
pub const MODE_SPLIT: u32 = 1;
pub const MODE_SYSTEM_LINK: u32 = 2;
pub const MODE_LIVE: u32 = 3;
pub const MODE_INVITE: u32 = 4;
/// The campaign's screens: A on Campaign, and the one after it.
pub const CAMPAIGN_SCREENS: [u32; 2] = [0xee, 0xd1];
/// Live menu rows (`custom_games`).
pub const BROWSER_ROW: u32 = 3;
pub const COOP_ROW: u32 = 4;
/// Main menu rows (`boot::ui_watch`: `0x230827`): 0 Campaign, 1 Xbox Live,
/// 2 Split Screen, 3 System Link, 4 Settings.
pub const MAIN_LIVE_ROW: u32 = 1;
pub const MAIN_SPLIT_ROW: u32 = 2;
pub const MAIN_SYSTEM_LINK_ROW: u32 = 3;

/// One press: held 150 ms and two reads, so a slow frame still sees it.
pub const PRESS_MS: u64 = 150;
pub const PRESS_READS: u32 = 2;

/// The `input.pad` step for one press of `button`.
pub fn press_step(button: &str) -> Value {
    json!({ "buttons": [button], "ms": PRESS_MS, "reads": PRESS_READS })
}

/// A step's screen check polls every this many ticks…
const POLL_TICKS: u64 = 5;
/// …waiting at most this long for them (the menus may tick slowly).
const POLL_MS: u64 = 1000;
/// A step's default timeout.
pub const STEP_MS: u64 = 15_000;

/// What `ui.screen` said.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    /// None on the main menu (it reads as no screen).
    #[serde(default)]
    pub screen: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_menu_row: Option<u32>,
}

/// What a press should lead to. Every field given must hold.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// This screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<u32>,
    /// No screen: the main menu.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub main_menu: bool,
    /// The players' screen's mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    /// Any screen but this one (the press left it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not: Option<Option<u32>>,
}

impl Expect {
    pub fn holds(&self, s: &Screen) -> bool {
        self.screen.is_none_or(|id| s.screen == Some(id))
            && (!self.main_menu || s.screen.is_none())
            && self.mode.is_none_or(|m| s.mode == Some(m))
            && self.not.is_none_or(|n| s.screen != n)
    }
}

/// The main menu's modes (`lobby` `mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Campaign,
    XboxLive,
    SplitScreen,
    SystemLink,
}

impl Mode {
    fn row(self) -> u32 {
        match self {
            Mode::Campaign => 0,
            Mode::XboxLive => MAIN_LIVE_ROW,
            Mode::SplitScreen => MAIN_SPLIT_ROW,
            Mode::SystemLink => MAIN_SYSTEM_LINK_ROW,
        }
    }

    /// Where A on its row goes.
    fn opens(self) -> Expect {
        match self {
            Mode::Campaign => Expect {
                screen: Some(CAMPAIGN_SCREENS[0]),
                ..Default::default()
            },
            Mode::XboxLive => players(MODE_LIVE),
            Mode::SplitScreen => players(MODE_SPLIT),
            Mode::SystemLink => players(MODE_SYSTEM_LINK),
        }
    }
}

fn players(mode: u32) -> Expect {
    Expect {
        screen: Some(JOIN_SCREEN),
        mode: Some(mode),
        ..Default::default()
    }
}

/// What a host hosts (`lobby` `host`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Host {
    /// The Xbox Live menu's Co-op Campaign row (`boot::coop_live`).
    LiveCoop,
}

/// One lobby step (the `lobby` script step's `action`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Lobby {
    /// Presses, then wait for `expect` (if given).
    Press {
        buttons: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<Expect>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
    /// On the main menu: pick a mode's row and open it.
    Mode { mode: Mode },
    /// From the players' screen in Xbox Live mode (or the Live menu):
    /// host it.
    Host { via: Host },
    /// In a lobby: Y for the friends list, then A on friend row `friend`
    /// (default 0), sending the invitation.
    Invite {
        #[serde(default)]
        friend: u32,
    },
    /// In a lobby: A on Start Game (row 0), and wait for the screen to go.
    Start,
}

/// Reads `ui.screen`.
pub fn screen(rpc: &mut dyn Rpc) -> Result<Screen, RpcError> {
    let v = rpc.call(method::UI_SCREEN, Value::Null)?;
    serde_json::from_value(v.clone())
        .map_err(|e| RpcError::new(code::UNAVAILABLE, format!("ui.screen answered {v} ({e})")))
}

fn fail(why: String, seen: &Screen) -> RpcError {
    let mut e = RpcError::new(code::TIMEOUT, why);
    e.data = Some(json!({ "screen": seen }));
    e
}

/// Sends `buttons` as one `input.pad` (a release between presses).
fn press(rpc: &mut dyn Rpc, buttons: &[&str]) -> Result<(), RpcError> {
    let mut steps = Vec::new();
    for b in buttons {
        steps.push(press_step(b));
        steps.push(json!({ "ms": PRESS_MS, "reads": PRESS_READS }));
    }
    rpc.call(method::INPUT_PAD, json!({ "player": 0, "steps": steps }))?;
    Ok(())
}

/// Polls until `want` holds or `timeout_ms` has passed (in polls of
/// [`POLL_TICKS`] ticks).
fn until(
    rpc: &mut dyn Rpc,
    want: &Expect,
    timeout_ms: u64,
    what: &str,
) -> Result<Screen, RpcError> {
    let polls = (timeout_ms / POLL_MS).max(1);
    let mut seen = screen(rpc)?;
    for _ in 0..polls {
        if want.holds(&seen) {
            return Ok(seen);
        }
        let r = rpc.call(
            method::WAIT,
            json!({ "until": { "ticks": POLL_TICKS }, "timeout_ms": POLL_MS }),
        );
        match r {
            Ok(_) => {}
            Err(e) if e.code == code::TIMEOUT => {}
            Err(e) => return Err(e),
        }
        seen = screen(rpc)?;
    }
    if want.holds(&seen) {
        return Ok(seen);
    }
    Err(fail(
        format!(
            "{what}: wanted {} within {timeout_ms} ms, saw {}",
            serde_json::to_string(want).unwrap_or_default(),
            show(&seen)
        ),
        &seen,
    ))
}

fn show(s: &Screen) -> String {
    match s.screen {
        None => "the main menu (no screen)".into(),
        Some(id) => {
            let mut t = format!("screen 0x{id:02x}");
            if let Some(m) = s.mode {
                t.push_str(&format!(" mode {m}"));
            }
            t
        }
    }
}

fn downs(n: u32) -> Vec<&'static str> {
    vec!["down"; n as usize]
}

/// Runs one lobby step; the screen it ended on.
pub fn run(rpc: &mut dyn Rpc, step: &Lobby) -> Result<Value, RpcError> {
    let done = |s: Screen| Ok(json!({ "screen": s }));
    match step {
        Lobby::Press {
            buttons,
            expect,
            timeout_ms,
        } => {
            if buttons.is_empty() {
                return Err(RpcError::new(code::INVALID_PARAMS, "press needs buttons"));
            }
            for b in buttons {
                if !crate::PAD_BUTTONS.contains(&b.as_str()) {
                    return Err(RpcError::new(
                        code::INVALID_PARAMS,
                        format!("no button {b}"),
                    ));
                }
            }
            let b: Vec<&str> = buttons.iter().map(String::as_str).collect();
            press(rpc, &b)?;
            match expect {
                Some(want) => done(until(rpc, want, timeout_ms.unwrap_or(STEP_MS), "press")?),
                None => done(screen(rpc)?),
            }
        }
        Lobby::Mode { mode } => {
            let main = Expect {
                main_menu: true,
                ..Default::default()
            };
            until(rpc, &main, STEP_MS, "mode: on the main menu")?;
            let mut b = downs(mode.row());
            b.push("a");
            press(rpc, &b)?;
            done(until(rpc, &mode.opens(), STEP_MS, "mode")?)
        }
        Lobby::Host {
            via: Host::LiveCoop,
        } => {
            let mut s = screen(rpc)?;
            if s.screen == Some(JOIN_SCREEN) && s.mode == Some(MODE_LIVE) {
                press(rpc, &["a"])?;
                let menu = Expect {
                    screen: Some(LIVE_MENU),
                    ..Default::default()
                };
                s = until(rpc, &menu, STEP_MS, "host: to the Xbox Live menu")?;
            }
            if s.screen != Some(LIVE_MENU) {
                return Err(fail(
                    format!(
                        "host: start on the players' screen in Xbox Live mode or the Xbox Live menu, not {}",
                        show(&s)
                    ),
                    &s,
                ));
            }
            // The menu restores its cursor to a row it reports when 0 to 3.
            let from = s.live_menu_row.unwrap_or(0).min(COOP_ROW);
            let mut b = downs(COOP_ROW - from);
            b.push("a");
            press(rpc, &b)?;
            let left = Expect {
                not: Some(Some(LIVE_MENU)),
                ..Default::default()
            };
            done(until(rpc, &left, STEP_MS, "host: Co-op Campaign")?)
        }
        Lobby::Invite { friend } => {
            let at = screen(rpc)?;
            press(rpc, &["y"])?;
            let left = Expect {
                not: Some(at.screen),
                ..Default::default()
            };
            let list = until(rpc, &left, STEP_MS, "invite: the friends list")?;
            let mut b = downs(*friend);
            b.push("a");
            press(rpc, &b)?;
            // The invitation sends from the row; the list may stay open.
            let _ = list;
            done(screen(rpc)?)
        }
        Lobby::Start => {
            let at = screen(rpc)?;
            press(rpc, &["a"])?;
            let left = Expect {
                not: Some(at.screen),
                ..Default::default()
            };
            done(until(rpc, &left, STEP_MS, "start")?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A menu: each `input.pad` call moves to the next screen of `after`.
    struct Menu {
        now: Screen,
        after: VecDeque<Screen>,
        pads: Vec<Value>,
    }

    impl Rpc for Menu {
        fn call(&mut self, m: &str, p: Value) -> Result<Value, RpcError> {
            match m {
                method::UI_SCREEN => Ok(serde_json::to_value(self.now).unwrap()),
                method::INPUT_PAD => {
                    self.pads.push(p);
                    if let Some(s) = self.after.pop_front() {
                        self.now = s;
                    }
                    Ok(json!({"queued": 1}))
                }
                method::WAIT => Ok(json!({"ticks": 5})),
                _ => Err(RpcError::new(code::METHOD_NOT_FOUND, m)),
            }
        }
    }

    fn at(screen: Option<u32>, mode: Option<u32>, row: Option<u32>) -> Screen {
        Screen {
            screen,
            mode,
            live_menu_row: row,
        }
    }

    fn buttons(p: &Value) -> Vec<String> {
        p["steps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s["buttons"][0].as_str().map(String::from))
            .collect()
    }

    #[test]
    fn mode_presses_down_to_the_row_and_checks_the_screen() {
        let mut m = Menu {
            now: Screen::default(),
            after: [at(Some(JOIN_SCREEN), Some(MODE_SYSTEM_LINK), None)].into(),
            pads: vec![],
        };
        let r = run(
            &mut m,
            &Lobby::Mode {
                mode: Mode::SystemLink,
            },
        )
        .unwrap();
        assert_eq!(buttons(&m.pads[0]), ["down", "down", "down", "a"]);
        assert_eq!(r["screen"]["mode"], MODE_SYSTEM_LINK);
    }

    #[test]
    fn a_wrong_screen_fails_saying_what_it_saw() {
        let mut m = Menu {
            now: Screen::default(),
            after: [at(Some(JOIN_SCREEN), Some(MODE_SPLIT), None)].into(),
            pads: vec![],
        };
        let e = run(
            &mut m,
            &Lobby::Mode {
                mode: Mode::SystemLink,
            },
        )
        .unwrap_err();
        assert_eq!(e.code, code::TIMEOUT);
        assert!(e.message.contains("screen 0x1e mode 1"), "{}", e.message);
    }

    #[test]
    fn host_goes_through_the_live_menu_to_the_coop_row() {
        let mut m = Menu {
            now: at(Some(JOIN_SCREEN), Some(MODE_LIVE), None),
            after: [
                at(Some(LIVE_MENU), None, Some(1)),
                at(Some(0x200), None, None),
            ]
            .into(),
            pads: vec![],
        };
        run(
            &mut m,
            &Lobby::Host {
                via: Host::LiveCoop,
            },
        )
        .unwrap();
        assert_eq!(buttons(&m.pads[0]), ["a"]);
        assert_eq!(buttons(&m.pads[1]), ["down", "down", "down", "a"]);
    }

    #[test]
    fn steps_parse_by_action() {
        let s: Lobby =
            serde_json::from_value(json!({"action": "mode", "mode": "xbox_live"})).unwrap();
        assert_eq!(
            s,
            Lobby::Mode {
                mode: Mode::XboxLive
            }
        );
        let s: Lobby = serde_json::from_value(json!({"action": "start"})).unwrap();
        assert_eq!(s, Lobby::Start);
        let s: Lobby = serde_json::from_value(
            json!({"action": "press", "buttons": ["b"], "expect": {"main_menu": true}}),
        )
        .unwrap();
        assert!(matches!(s, Lobby::Press { .. }));
        assert!(serde_json::from_value::<Lobby>(json!({"action": "teleport"})).is_err());
    }

    #[test]
    fn start_waits_for_the_lobby_to_go() {
        let mut m = Menu {
            now: at(Some(0x40), None, None),
            after: [at(None, None, None)].into(),
            pads: vec![],
        };
        // The main menu counts as having left the lobby too.
        run(&mut m, &Lobby::Start).unwrap();
        let mut stuck = Menu {
            now: at(Some(0x40), None, None),
            after: VecDeque::new(),
            pads: vec![],
        };
        assert!(run(&mut stuck, &Lobby::Start).is_err());
    }
}
