//! The custom lobby's status line: the expired countdown while the host is
//! bringing up its phantoms, why Start Game is refused (the start gate's
//! first problem, `crate::lobby_mode::gate_text_id`, or the lobby's own
//! refusal, `crate::lobby_mode::start_refusal`: a Forge lobby with a member
//! on another console, a Firefight lobby on a map without the kit), and
//! the lobby modes' reason line (`crate::lobby_tabs::notice_now`): why a
//! mode picked in the GAME MODE picker did not switch, for four seconds.

use bots::{lobby, phantom};
use guest_layout::GuestMemory;

use phantom::session::{self, at::*};

const SETUP_TEXT: &str = "Setting up bots…";
const SETTING_UP_BOTS: u32 = crate::ui_kit::ids::family("lobby status").id(0);
pub(crate) const STRINGS: [(u32, &str); 1] = [(SETTING_UP_BOTS, SETUP_TEXT)];

/// The status widget write at the end of `0x24ffc9`. These IDs carry
/// the countdown token (U+E413). The other status IDs describe separate
/// states such as map load, cancellation and delay.
const CUSTOM_STATUS_RETURN: u32 = 0x0025_012f;
const CUSTOM_COUNTDOWN_IDS: [u32; 2] = [0x2000_01f0, 0x2b00_020b];

fn countdown_text(seconds: i32, setting_up: bool) -> Option<u32> {
    (seconds == 0 && setting_up).then_some(SETTING_UP_BOTS)
}

pub(crate) fn custom_game_text(m: &mut dyn GuestMemory, return_pc: u32, id: u32) -> Option<u32> {
    if return_pc != CUSTOM_STATUS_RETURN {
        return None;
    }
    // A mode picked that is off, or a switch refused: its reason, over
    // every other text, while it shows.
    if let Some(reason) = crate::lobby_tabs::notice_now() {
        return Some(reason.id());
    }
    // The start gate holds Start Game: the map lacks what the variant's
    // script needs (docs/game/rules-start-gate.md, "What a blocked start
    // shows").
    if let Some(gate) = crate::lobby_mode::gate_text_id(m) {
        return Some(gate);
    }
    // A Forge lobby with a member on another console, or a Firefight lobby
    // on a map without the kit: its start state is held off ready, and the
    // line says why in place of retail's text (the lobby modes' reasons,
    // `crate::lobby_tabs::strings`).
    if let Some(reason) = crate::lobby_mode::start_refusal(m) {
        return Some(reason.id());
    }
    // The lobby's map variant builds, or failed a check: its sentence.
    if let Some(id) = crate::lobby_mode::variant_text_id(m) {
        return Some(id);
    }
    let m: &dyn GuestMemory = m;
    if !CUSTOM_COUNTDOWN_IDS.contains(&id) {
        return None;
    }
    let s = session::owned_session(m)?;
    // The custom lobby's countdown token reads the squad session.
    if m.u32(lobby::SESSION).ok()? != s {
        return None;
    }
    countdown_override(m, s, phantom::global())
}

fn countdown_override(m: &dyn GuestMemory, s: u32, phantoms: &phantom::Phantoms) -> Option<u32> {
    if m.u8(s + lobby::COUNTDOWN).ok()? == 0 {
        return None;
    }
    let seconds = m.i32(s + lobby::COUNTDOWN + 4).ok()?;
    countdown_text(seconds, seconds == 0 && setting_up(m, s, phantoms))
}

fn setting_up(m: &dyn GuestMemory, s: u32, phantoms: &phantom::Phantoms) -> bool {
    if phantoms.match_only() && !lobby::matchmaking_cycle(m) {
        return false;
    }
    let bots = phantoms.list();
    if bots.iter().any(|bot| bot.state == phantom::State::Pending) {
        return true;
    }

    // Seated phantoms count while the session loads the map or the
    // simulation server establishes its views. Neither stage waits for
    // the asynchronous nav cache.
    let starting = m
        .u32(SIM_GLOBALS)
        .ok()
        .filter(|&sim| sim != 0 && sim.checked_add(SIM_STATE).is_some())
        .is_some_and(|sim| {
            matches!(m.u32(sim + SIM_MODE), Ok(2 | session::WORLD_DIST_SERVER))
                && matches!(m.u32(sim + SIM_STATE), Ok(session::SIM_STARTING | 4))
        });
    (m.u32(s + CLASS) == Ok(2) || starting)
        && bots.iter().any(|bot| {
            bot.state == phantom::State::Joined
                && session::find_peer(m, s, bot.machine).is_ok_and(|p| p.is_some())
        })
}

/// The matchmaking countdown is a tag text (`0x0f00188e`) with token
/// U+E45B. Draw and measure substitute into temporary UTF-16 copies of
/// the widget text. The copy-back caller at `0x22d2de` keeps its template.
const MATCH_COUNTDOWN_TOKEN: u16 = 0xe45b;
const TEMP_TEXT_RETURNS: [u32; 2] = [0x0022_ce44, 0x0022_d18f];

fn has_match_countdown(m: &dyn GuestMemory, text: u32, capacity: u32) -> bool {
    for i in 0..capacity {
        match m.u16(text + i * 2) {
            Ok(MATCH_COUNTDOWN_TOKEN) => return true,
            Ok(0) | Err(_) => return false,
            _ => {}
        }
    }
    false
}

pub(crate) struct MatchCountdown;

impl hooks::Feature for MatchCountdown {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new("match-countdown-text", "Matchmaking's bot setup status")
            .doc("docs/game/bots.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        b.raw()
            .before(
                &guest_layout::points::HUD_TEXT_SUBSTITUTE,
                "name bot setup on the matchmaking countdown",
                |c| {
                    let sp = c.cpu.esp;
                    let mut m = c.ctx.memory();
                    if m.u32(sp).ok().is_none_or(|pc| !TEMP_TEXT_RETURNS.contains(&pc))
                        || m.u32(lobby::SESSION_TYPE) != Ok(7)
                    {
                        return Ok(hooks::Flow::Continue);
                    }
                    let Some(s) = session::owned_session(&*m) else {
                        return Ok(hooks::Flow::Continue);
                    };
                    let text = m.u32(sp + 4)?;
                    let capacity = m.u32(sp + 8)?.min(0x400);
                    let len = SETUP_TEXT.encode_utf16().count() as u32;
                    if capacity > len
                        && text.checked_add(capacity * 2).is_some()
                        && has_match_countdown(&*m, text, capacity)
                        && countdown_override(&*m, s, phantom::global()).is_some()
                    {
                        let bytes: Vec<u8> = SETUP_TEXT
                            .encode_utf16()
                            .chain([0])
                            .flat_map(u16::to_le_bytes)
                            .collect();
                        m.write_bytes(text, &bytes, 1)?;
                    }
                    Ok(hooks::Flow::Continue)
                },
            )
            .after_feature(crate::h1_hud_text::FEATURE)
            .context_only();
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_expired_countdown_names_active_bot_setup() {
        assert_eq!(countdown_text(0, true), Some(SETTING_UP_BOTS));
        assert_eq!(STRINGS[0].1, "Setting up bots…");
        assert_eq!(countdown_text(0, false), None);
        assert_eq!(countdown_text(1, true), None);
        assert_eq!(countdown_text(-1, true), None);
    }

    #[test]
    fn an_active_zero_counts_pending_phantoms_in_its_own_session() {
        const S: u32 = 0x0060_0000;
        let mut g = guest_layout::testing::FakeGuest::new();
        g.put_u8(S + lobby::COUNTDOWN, 1);
        let phantoms = phantom::Phantoms::default();
        let phantom = phantoms
            .add(
                phantom::Identity {
                    xuid: 1,
                    gamertag: "Clanker".into(),
                },
                None,
                Box::new(|_: &bots::pad_source::Frame<'_>| Default::default()),
            )
            .unwrap();
        assert_eq!(countdown_override(&g, S, &phantoms), Some(SETTING_UP_BOTS));
        phantoms.set_match_only(true);
        assert_eq!(countdown_override(&g, S, &phantoms), None);
        g.put_u8(lobby::NETWORKING, 1).put_u32(lobby::SESSION_TYPE, 7);
        assert_eq!(countdown_override(&g, S, &phantoms), Some(SETTING_UP_BOTS));
        g.put_u8(S + lobby::COUNTDOWN, 0);
        assert_eq!(countdown_override(&g, S, &phantoms), None);
        g.put_u8(S + lobby::COUNTDOWN, 1);
        phantoms.remove(phantom).unwrap();
        assert_eq!(countdown_override(&g, S, &phantoms), None);
    }
}
