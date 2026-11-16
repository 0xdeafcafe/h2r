//! Our text on the game's own "cannot connect to Xbox Live" dialog.
//!
//! When Xbox Live is picked from the main menu with no network transport, the
//! menu's row handler (`0x002308e0`, transport check at `0x00230928`) shows UI
//! error `0x23`, the dialog that offers the troubleshooter. A sign-in whose
//! task result is CANNOT_ACCESS_SERVICE reaches the same error through the
//! connection-result handler (`0x001486fa`). With Live off (`h2 play`
//! without a Live flag) the console reports no cable, so transport never
//! starts and the first path is taken; with Live on and a server that cannot
//! be reached, the second.
//!
//! A dialog widget keeps its error ID at `+0x610` and four 256-unit UTF-16
//! buffers: title `+0x614`, message `+0x814`, then the two button labels.
//! `0x0019b72a` (EAX = error ID, EBX = dialog, one stack word, `ret 4`)
//! resolves the error's strings into them, and every dialog update copies the
//! title and message buffers into its text widgets. This module runs that
//! fill and then, for error `0x23` only, writes our message over the message
//! buffer, so the game draws our text in its own dialog and font. No tag or
//! localized string is changed. `H2_NO_LIVE_NOTICE=1` skips it.
//! See docs/live/live.md, "Client HLE: Live not set up".

use std::sync::Arc;

use runtime::guest::Permissions;
use runtime::{Guest, GuestFault, Runtime, TransferAbi, abi};

use crate::LiveConfig;

/// The dialog text fill the hook wraps.
pub const DIALOG_TEXT_FILL: u32 = 0x0019_b72a;
/// The UI error for "cannot connect to Xbox Live / start the troubleshooter".
pub const CANNOT_CONNECT_ERROR: u32 = 0x23;
/// Dialog widget fields.
pub const DIALOG_ERROR_ID: u32 = 0x610;
pub const DIALOG_MESSAGE: u32 = 0x814;
/// Units in each dialog text buffer, terminator included.
pub const MESSAGE_UNITS: usize = 256;
/// Shadow entry that runs the original fill (see the page list in lib.rs).
const SHADOW: u32 = 0xa0ff_6000;
/// The longest server URL shown before it is shortened.
const URL_CHARS: usize = 40;

/// What the dialog should say instead of the stock text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// Live is off: how to start the game with it on.
    NotSetUp,
    /// Live is on with this server, and sign-in could not reach it.
    Unreachable(String),
    /// Live is on with this server, but this machine holds no account key
    /// for it: how to make or recover one (docs/live/live.md, "In-game
    /// account setup").
    NoAccount(String),
}

// The message box on the stock dialog is about 244 by 91 pixels, five lines
// of its body font; each line below measures under 240 pixels. Lines break on
// `\r`, as the game's own strings do; `|` would start a markup escape.
const START_AGAIN: &str = "Quit and start again with one of:\r\n\
                           h2 play --live  (live.forbes.red)\r\n\
                           h2 play --live-local  (this machine)";

impl Notice {
    /// The notice for a boot configuration. Live on without a server signs
    /// in locally and never fails to connect, so it keeps the stock text.
    pub fn for_config(config: &LiveConfig) -> Option<Self> {
        match (config.enabled, &config.server_url) {
            (false, _) => Some(Self::NotSetUp),
            (true, Some(url)) if no_accounts(config) => Some(Self::NoAccount(url.clone())),
            (true, Some(url)) => Some(Self::Unreachable(url.clone())),
            (true, None) => None,
        }
    }

    pub fn text(&self) -> String {
        match self {
            Self::NotSetUp => format!(
                "Xbox Live isn't set up in h2recomp.\r\n{START_AGAIN}\r\n\
                 Accounts: h2 live account create <tag>"
            ),
            Self::Unreachable(url) => format!(
                "Can't reach the h2 live server at\r\n{}.\r\n{START_AGAIN}",
                display_url(url)
            ),
            Self::NoAccount(url) => format!(
                "No Live account on this machine yet.\r\n\
                 Make one: h2 live account create <tag>\r\n\
                 or recover one in a browser at\r\n{}\r\n\
                 /dash/signup/recover, then start again.",
                display_url(url)
            ),
        }
    }

    /// The text as the dialog buffer holds it: UTF-16, at most 255 units,
    /// then a zero.
    pub fn units(&self) -> Vec<u16> {
        let mut units: Vec<u16> = self.text().encode_utf16().take(MESSAGE_UNITS - 1).collect();
        units.push(0);
        units
    }
}

/// Whether the boot's accounts file holds no account with a usable key
/// (a missing file or no path holds none). An unreadable file is not "no
/// accounts": sign-in reports it.
fn no_accounts(config: &LiveConfig) -> bool {
    match &config.accounts_path {
        None => true,
        Some(path) => live_hle::account_setup::load(path).is_ok_and(|a| a.is_empty()),
    }
}

/// A server URL as one short line: printable ASCII without `|`, no trailing
/// slash, and at most [`URL_CHARS`] characters.
fn display_url(url: &str) -> String {
    let clean: String = url
        .trim()
        .trim_end_matches('/')
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '|' {
                c
            } else {
                '?'
            }
        })
        .collect();
    if clean.chars().count() <= URL_CHARS {
        clean
    } else {
        let head: String = clean.chars().take(URL_CHARS - 3).collect();
        format!("{head}...")
    }
}

/// After the fill: put `units` in `dialog`'s message buffer if it shows the
/// cannot-connect error. Returns whether it wrote. Other dialogs are left
/// alone.
pub fn apply(guest: &mut Guest, dialog: u32, units: &[u16]) -> Result<bool, GuestFault> {
    if guest.u32(dialog.wrapping_add(DIALOG_ERROR_ID))? != CANNOT_CONNECT_ERROR {
        return Ok(false);
    }
    // At most 255 units, always terminated: the next buffer is a button label.
    let len = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    let units = units[..len.min(MESSAGE_UNITS - 1)].iter().chain(&[0]);
    let bytes: Vec<u8> = units.flat_map(|u| u.to_le_bytes()).collect();
    guest.write_bytes(dialog.wrapping_add(DIALOG_MESSAGE), &bytes, 1)?;
    Ok(true)
}

/// Install for this boot's Live settings. Returns whether the hook went in.
/// Must run after the generated module is registered.
pub fn install(rt: &Runtime, config: &LiveConfig) -> anyhow::Result<bool> {
    if std::env::var_os("H2_NO_LIVE_NOTICE").is_some() {
        return Ok(false);
    }
    match Notice::for_config(config) {
        Some(notice) => install_at(rt, &notice, DIALOG_TEXT_FILL, SHADOW),
        None => Ok(false),
    }
}

/// [`install`] at chosen addresses, for tests with a stand-in fill.
pub fn install_at(rt: &Runtime, notice: &Notice, fill: u32, shadow: u32) -> anyhow::Result<bool> {
    // Resolve before edit_dispatch: resolve_original takes the dispatch read
    // lock, which edit_dispatch holds for writing.
    let (Some(direct), Some(through_shadow)) =
        (rt.resolve_original(fill), rt.resolve_original(fill))
    else {
        eprintln!("live notice: no generated entry at {fill:#x}; hook skipped");
        return Ok(false);
    };
    // Wrapping another override would hide it; refuse before mapping anything.
    if let Some(name) = rt.edit_dispatch(|d| d.override_name(fill).map(str::to_owned)) {
        anyhow::bail!("live notice: {fill:#x} already has the override {name}");
    }
    rt.memory().map_zeroed(shadow, 0x1000, Permissions::RX)?;
    let units: Arc<[u16]> = notice.units().into();
    rt.edit_dispatch(|d| {
        d.register_override(shadow, "live notice: original fill", move |cpu, ctx, _| {
            Ok(through_shadow.invoke(cpu, ctx, fill))
        });
        d.register_override(fill, "live notice: dialog text", move |cpu, ctx, _| {
            let dialog = cpu.ebx;
            let return_pc = ctx.memory().u32(cpu.esp);
            let Ok(return_pc) = return_pc else {
                return Ok(direct.invoke(cpu, ctx, fill));
            };
            cpu.esp = cpu.esp.wrapping_add(4);
            let units = Arc::clone(&units);
            let transfer = {
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                ctx.call_guest(cpu, &mut mem, shadow, move |_, ctx| {
                    if let Err(e) = apply(&mut ctx.memory(), dialog, &units) {
                        eprintln!("live notice: dialog {dialog:#x} not patched: {e:?}");
                    }
                    Ok(TransferAbi::ret(return_pc))
                })
            };
            match transfer {
                Ok(t) if t.kind != abi::FAULT => Ok(t),
                _ => {
                    cpu.esp = cpu.esp.wrapping_sub(4);
                    Ok(direct.invoke(cpu, ctx, fill))
                }
            }
        });
    });
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIALOG: u32 = 0x0010_0000;

    fn guest_with_dialog(error: u32) -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(DIALOG, 0x1000, Permissions::RW).unwrap();
        g.set_u32(DIALOG + DIALOG_ERROR_ID, error).unwrap();
        g
    }

    fn message(g: &Guest) -> Vec<u16> {
        let mut b = vec![0; MESSAGE_UNITS * 2];
        g.read_bytes(DIALOG + DIALOG_MESSAGE, &mut b, 1).unwrap();
        let units: Vec<u16> = b
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let end = units.iter().position(|&u| u == 0).unwrap();
        units[..end].to_vec()
    }

    fn config(enabled: bool, server_url: Option<&str>) -> LiveConfig {
        LiveConfig {
            enabled,
            accounts_path: None,
            server_url: server_url.map(Into::into),
        }
    }

    /// An accounts file with `n` keyed entries; removed by the caller.
    fn accounts_file(n: usize) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "h2-notice-accounts-{}-{n}.json",
            std::process::id()
        ));
        let entries: Vec<String> = (0..n)
            .map(|i| {
                live_hle::account_setup::LocalEntry {
                    xuid: live_proto::Xuid(0x9000000000001 + i as u64),
                    gamertag: format!("P{i}"),
                    id: String::new(),
                    server: "http://127.0.0.1:8074".into(),
                    server_id: "live.test".into(),
                    key: Arc::new(live_proto::sign::KeyPair::generate()),
                }
                .to_json()
                .to_string()
            })
            .collect();
        std::fs::write(&path, format!("[{}]", entries.join(","))).unwrap();
        path
    }

    #[test]
    fn notice_follows_the_live_settings() {
        assert_eq!(
            Notice::for_config(&config(false, None)),
            Some(Notice::NotSetUp)
        );
        assert_eq!(
            Notice::for_config(&config(false, Some("http://a"))),
            Some(Notice::NotSetUp)
        );
        let one = accounts_file(1);
        let none = accounts_file(0);
        let with = |path: &std::path::PathBuf| LiveConfig {
            accounts_path: Some(path.clone()),
            ..config(true, Some("http://127.0.0.1:8074"))
        };
        assert_eq!(
            Notice::for_config(&with(&one)),
            Some(Notice::Unreachable("http://127.0.0.1:8074".into()))
        );
        for no_account in [with(&none), config(true, Some("http://127.0.0.1:8074"))] {
            assert_eq!(
                Notice::for_config(&no_account),
                Some(Notice::NoAccount("http://127.0.0.1:8074".into()))
            );
        }
        // An unreadable file is left to sign-in to report.
        std::fs::write(&none, "not json").unwrap();
        assert!(matches!(
            Notice::for_config(&with(&none)),
            Some(Notice::Unreachable(_))
        ));
        std::fs::remove_file(one).unwrap();
        std::fs::remove_file(none).unwrap();
        assert_eq!(Notice::for_config(&config(true, None)), None);
    }

    #[test]
    fn texts_fit_the_dialog() {
        let long = format!("https://{}.example/", "a".repeat(300));
        for n in [
            Notice::NotSetUp,
            Notice::Unreachable("http://127.0.0.1:8074/".into()),
            Notice::Unreachable(long.clone()),
            Notice::NoAccount("https://live.forbes.red".into()),
            Notice::NoAccount(long),
        ] {
            let text = n.text();
            let lines: Vec<&str> = text.split("\r\n").collect();
            // Five short lines, each broken the way the game's strings are.
            assert_eq!(lines.len(), 5, "{text:?}");
            assert!(
                lines
                    .iter()
                    .all(|l| l.len() <= URL_CHARS + 1 && !l.contains(['\r', '\n'])),
                "{text:?}"
            );
            assert!(!text.contains('|'));
            assert!(n.units().len() <= MESSAGE_UNITS);
            assert_eq!(n.units().last(), Some(&0));
        }
        assert!(
            Notice::NotSetUp
                .text()
                .starts_with("Xbox Live isn't set up in h2recomp.\r\n")
        );
        let text = Notice::NotSetUp.text();
        assert!(text.contains("\r\nh2 play --live  (live.forbes.red)\r\n"), "{text:?}");
        assert!(text.contains("h2 play --live-local  (this machine)"), "{text:?}");
        assert!(
            text.contains(live_proto::DEFAULT_SERVER.trim_start_matches("https://")),
            "the notice names the default service"
        );
        assert!(
            Notice::NotSetUp
                .text()
                .ends_with("h2 live account create <tag>")
        );
        assert_eq!(
            Notice::NoAccount("https://live.example/".into()).text(),
            "No Live account on this machine yet.\r\nMake one: h2 live account create <tag>\r\n\
             or recover one in a browser at\r\nhttps://live.example\r\n\
             /dash/signup/recover, then start again."
        );
        let unreachable = Notice::Unreachable("http://127.0.0.1:8074/".into()).text();
        assert!(
            unreachable
                .starts_with("Can't reach the h2 live server at\r\nhttp://127.0.0.1:8074.\r\n")
        );
    }

    #[test]
    fn urls_are_one_safe_line() {
        assert_eq!(display_url(" http://host:1/ "), "http://host:1");
        assert_eq!(display_url("http://a|b\r\nc"), "http://a?b??c");
        let long = display_url(&format!("http://{}", "x".repeat(100)));
        assert_eq!(long.chars().count(), URL_CHARS);
        assert!(long.ends_with("..."));
    }

    #[test]
    fn units_are_capped_with_a_terminator() {
        let n = Notice::Unreachable("x".repeat(1000));
        assert!(n.units().len() <= MESSAGE_UNITS);
        let big: Vec<u16> = std::iter::repeat_n(u16::from(b'a'), 400).collect();
        let mut g = guest_with_dialog(CANNOT_CONNECT_ERROR);
        g.set_u32(
            DIALOG + DIALOG_MESSAGE + MESSAGE_UNITS as u32 * 2,
            0xfeed_f00d,
        )
        .unwrap();
        assert!(apply(&mut g, DIALOG, &big).unwrap());
        assert_eq!(message(&g).len(), MESSAGE_UNITS - 1);
        // The next buffer (the first button label) is untouched.
        assert_eq!(
            g.u32(DIALOG + DIALOG_MESSAGE + MESSAGE_UNITS as u32 * 2)
                .unwrap(),
            0xfeed_f00d
        );
    }

    #[test]
    fn only_the_cannot_connect_dialog_changes() {
        let units = Notice::NotSetUp.units();
        let mut g = guest_with_dialog(CANNOT_CONNECT_ERROR);
        g.write_bytes(DIALOG + DIALOG_MESSAGE, &[b'X', 0, b'Y', 0, 0, 0], 1)
            .unwrap();
        assert!(apply(&mut g, DIALOG, &units).unwrap());
        assert_eq!(
            message(&g),
            Notice::NotSetUp.text().encode_utf16().collect::<Vec<_>>()
        );

        for other in [0, 0x22, 0x24, 0x38, 0x9a] {
            let mut g = guest_with_dialog(other);
            g.write_bytes(DIALOG + DIALOG_MESSAGE, &[b'X', 0, 0, 0], 1)
                .unwrap();
            assert!(!apply(&mut g, DIALOG, &units).unwrap());
            assert_eq!(message(&g), [u16::from(b'X')]);
        }
        // An unmapped dialog is a fault, not a panic.
        assert!(apply(&mut Guest::new(), DIALOG, &units).is_err());
    }
}
