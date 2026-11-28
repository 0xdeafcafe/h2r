//! RGB for each Xbox controller port, read at a guest boundary. The
//! palettes come from the loaded map, as they do for the player models
//! and scoreboard. See docs/audio/playstation-audio.md.

use guest_layout::tags::GroupTag;
use guest_layout::view::{Tags, players};
use guest_layout::{DatumHandle, GuestMemory, points};
use live_telemetry::layout;

const PORTS: usize = layout::PLAYER_SLOT_COUNT as usize;
const TEAMS: u32 = 8;
const RGB_STRIDE: u32 = 12;
const TEAM_PLAY: u32 = live_proto::hopper::variant::flag::TEAM_PLAY;

// The profile colour lookup at 0x001a06a0 indexes this RGB block.
const MATG_PLAYER_COLOURS: u32 = 0x160;
// The team colour lookup at 0x0007f720 takes the first universal record
// in the referenced mulg tag, then indexes its team RGB block.
const MATG_MULTIPLAYER_GLOBALS_DATUM: u32 = 0x16c;
const MULG_UNIVERSAL: u32 = 0;
const MULG_UNIVERSAL_TEAM_COLOURS: u32 = 0x10;
const BLOCK_ADDRESS: u32 = 4;

struct Palette {
    count: u32,
    base: u32,
}

impl Palette {
    fn read(m: &dyn GuestMemory, block: u32) -> Option<Self> {
        let count = u32::try_from(m.i32(block).ok()?).ok()?;
        let base = pointer(m, block.checked_add(BLOCK_ADDRESS)?)?;
        (count > 0).then_some(Self { count, base })
    }

    fn rgb(&self, m: &dyn GuestMemory, index: u32) -> Option<[u8; 3]> {
        if index >= self.count {
            return None;
        }
        let addr = self.base.checked_add(index.checked_mul(RGB_STRIDE)?)?;
        addr.checked_add(RGB_STRIDE - 1)?;
        let channels = m.vec3(addr).ok()?;
        channels
            .iter()
            .all(|v| v.is_finite())
            .then(|| channels.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8))
    }
}

fn pointer(m: &dyn GuestMemory, at: u32) -> Option<u32> {
    m.u32(at).ok().filter(|&p| p != 0)
}

fn team_palette(m: &dyn GuestMemory, matg: u32) -> Option<Palette> {
    let reference = matg.checked_add(MATG_MULTIPLAYER_GLOBALS_DATUM)?;
    let datum = DatumHandle::from_raw(m.u32(reference).ok()?)?;
    let entry = Tags.entry(m, datum).ok()??;
    if entry.group(m).ok()? != GroupTag::from_chars(*b"mulg") {
        return None;
    }
    let mulg = entry.address(m).ok()?;
    if mulg == 0 || m.i32(mulg.checked_add(MULG_UNIVERSAL)?).ok()? <= 0 {
        return None;
    }
    let universal = pointer(m, mulg.checked_add(MULG_UNIVERSAL + BLOCK_ADDRESS)?)?;
    Palette::read(m, universal.checked_add(MULG_UNIVERSAL_TEAM_COLOURS)?)
}

/// Slot order here is physical controller order, the order of the menu's
/// local profile copies. It is independent of in-match player datum order.
fn primary_index(m: &dyn GuestMemory, port: usize) -> Option<u32> {
    let slot = layout::PLAYER_SLOTS + port as u32 * layout::PLAYER_SLOT_SIZE;
    let flags = m.u32(slot + layout::PLAYER_SLOT_FLAGS).ok()?;
    if flags & layout::PLAYER_SLOT_SIGNED_IN == 0 {
        return None;
    }
    let addr = slot
        + layout::PLAYER_SLOT_PROFILE
        + layout::PROFILE_APPEARANCE
        + layout::APPEARANCE_PRIMARY;
    u32::try_from(m.u8(addr).ok()? as i8).ok()
}

fn team_play(m: &dyn GuestMemory) -> Option<bool> {
    let globals = pointer(m, layout::GAME_GLOBALS_PTR)?;
    let options = globals.checked_add(layout::OPTIONS)?;
    let mode = m.u32(options.checked_add(layout::OPTIONS_TYPE)?).ok()?;
    if mode != crate::level_boot::MODE_MULTIPLAYER {
        return Some(false);
    }
    let flags = m
        .u32(options.checked_add(layout::VARIANT + layout::VARIANT_FLAGS)?)
        .ok()?;
    Some(flags & TEAM_PLAY != 0)
}

fn local_teams(m: &dyn GuestMemory) -> [Option<u32>; PORTS] {
    let mut teams = [None; PORTS];
    let Ok(Some(players)) = players::array(m) else {
        return teams;
    };
    let Ok(roster) = players.live_iter(m, 16) else {
        return teams;
    };
    for (_, player) in roster.flatten() {
        if player.listed(m) != Ok(true) {
            continue;
        }
        let Ok(port) = m.i32(player.addr.wrapping_add(layout::PLAYER_CONTROLLER)) else {
            continue;
        };
        if let Some(team) = teams.get_mut(port as usize) {
            *team = player
                .team(m)
                .ok()
                .map(|id| id as u32)
                .filter(|&id| id < TEAMS);
        }
    }
    teams
}

/// Local seat `port`'s profile primary colour as a palette index
/// (`0..forge::look::PALETTE`): the seat's colour for a Forge monitor's
/// default look. `None` when the port is not signed in or the index is not
/// in the palette.
pub fn seat_colour(m: &dyn GuestMemory, port: usize) -> Option<u8> {
    primary_index(m, port)
        .and_then(|i| u8::try_from(i).ok())
        .filter(|i| *i < forge::look::PALETTE)
}

/// RGB of team `team`'s colour from the loaded map's multiplayer globals
/// (`mulg`), the block the lightbar reads for a team match; `None` where
/// the map has none or the team is outside it. The HUD kit's `Style::Team`
/// draws in it (docs/game/hud-kit.md, "Colours").
pub fn team_rgb(m: &dyn GuestMemory, team: u8) -> Option<[u8; 3]> {
    let matg = pointer(m, points::GLOBALS_TAG.base)?;
    team_palette(m, matg)?.rgb(m, u32::from(team))
}

/// RGB of palette colour `index` from the loaded map's player colours, or
/// `None` where the map has none or the index is outside them.
pub fn palette_rgb(m: &dyn GuestMemory, index: u8) -> Option<[u8; 3]> {
    let matg = pointer(m, points::GLOBALS_TAG.base)?;
    Palette::read(m, matg.checked_add(MATG_PLAYER_COLOURS)?)?.rgb(m, u32::from(index))
}

/// The colour of each local seat, or `None` when the guest has no colour
/// for it. In an active team variant a local player's assigned team wins.
/// Until a port has a valid team, it shows its profile colour when signed in.
pub fn colours(m: &dyn GuestMemory) -> [Option<[u8; 3]>; PORTS] {
    let Some(matg) = pointer(m, points::GLOBALS_TAG.base) else {
        return [None; PORTS];
    };
    let profile = matg
        .checked_add(MATG_PLAYER_COLOURS)
        .and_then(|block| Palette::read(m, block));
    let (teams, team) = if team_play(m) == Some(true) {
        (local_teams(m), team_palette(m, matg))
    } else {
        ([None; PORTS], None)
    };
    std::array::from_fn(|port| {
        if let Some(index) = teams[port] {
            team.as_ref()?.rgb(m, index)
        } else {
            profile.as_ref()?.rgb(m, primary_index(m, port)?)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::datum::layout as datum;
    use guest_layout::tags::{entry_layout, index_layout};
    use guest_layout::testing::FakeGuest;

    const GAME: u32 = 0x10000;
    const OPTIONS: u32 = GAME + layout::OPTIONS;
    const FLAGS: u32 = OPTIONS + layout::VARIANT + layout::VARIANT_FLAGS;
    const MATG: u32 = 0x20000;
    const PLAYER_PALETTE: u32 = 0x21000;
    const TAG_HEADER: u32 = 0x30000;
    const TAG_ENTRIES: u32 = 0x31000;
    const MULG: u32 = 0x40000;
    const UNIVERSAL: u32 = 0x41000;
    const TEAM_PALETTE: u32 = 0x42000;
    const MULG_DATUM: u32 = 0x1234_0001;
    const PLAYER_HEADER: u32 = 0x50000;
    const PLAYERS: u32 = 0x51000;

    // Synthetic values with different channels, so table/stride/component
    // mix-ups cannot produce the right result by accident.
    fn palette(m: &mut FakeGuest, block: u32, base: u32, entries: &[[f32; 3]]) {
        m.put_u32(block, entries.len() as u32)
            .put_u32(block + BLOCK_ADDRESS, base);
        for (index, rgb) in entries.iter().enumerate() {
            for (channel, &value) in rgb.iter().enumerate() {
                m.put_f32(base + index as u32 * RGB_STRIDE + channel as u32 * 4, value);
            }
        }
    }

    fn profile(m: &mut FakeGuest, port: u32, primary: u8) {
        let slot = layout::PLAYER_SLOTS + port * layout::PLAYER_SLOT_SIZE;
        m.put_u32(
            slot + layout::PLAYER_SLOT_FLAGS,
            layout::PLAYER_SLOT_SIGNED_IN,
        )
        .put_u8(
            slot + layout::PLAYER_SLOT_PROFILE
                + layout::PROFILE_APPEARANCE
                + layout::APPEARANCE_PRIMARY,
            primary,
        );
    }

    fn basic() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.put_u32(layout::GAME_GLOBALS_PTR, GAME)
            .put_u32(points::GLOBALS_TAG.base, MATG);
        palette(
            &mut m,
            MATG + MATG_PLAYER_COLOURS,
            PLAYER_PALETTE,
            &[[0.125, 0.25, 0.5], [0.75, 0.5, 0.0]],
        );
        profile(&mut m, 0, 1);
        profile(&mut m, 2, 0);
        m
    }

    fn teams() -> FakeGuest {
        let mut m = basic();
        let entry = TAG_ENTRIES + entry_layout::STRIDE;
        m.put_u32(
            OPTIONS + layout::OPTIONS_TYPE,
            crate::level_boot::MODE_MULTIPLAYER,
        )
        .put_u32(FLAGS, TEAM_PLAY)
        .put_u32(points::TAG_INDEX.base, TAG_HEADER)
        .put_u32(TAG_HEADER + index_layout::TAG_COUNT, 2)
        .put_u32(points::TAG_ENTRIES.base, TAG_ENTRIES)
        .put_u32(MATG + MATG_MULTIPLAYER_GLOBALS_DATUM, MULG_DATUM)
        .put_u32(
            entry + entry_layout::GROUP,
            GroupTag::from_chars(*b"mulg").0,
        )
        .put_u32(entry + entry_layout::DATUM, MULG_DATUM)
        .put_u32(entry + entry_layout::ADDRESS, MULG)
        .put_u32(MULG + MULG_UNIVERSAL, 1)
        .put_u32(MULG + MULG_UNIVERSAL + BLOCK_ADDRESS, UNIVERSAL)
        .put_u32(points::PLAYERS.base, PLAYER_HEADER)
        .put_u32(PLAYER_HEADER + datum::DATA, PLAYERS)
        .put_u32(PLAYER_HEADER + datum::ELEMENT_SIZE, players::Player::SIZE)
        .put_u32(PLAYER_HEADER + datum::HIGH_WATER, 6);
        palette(
            &mut m,
            UNIVERSAL + MULG_UNIVERSAL_TEAM_COLOURS,
            TEAM_PALETTE,
            &[[0.5, 1.0, 0.75], [0.25, 0.125, 1.0]],
        );
        m
    }

    fn player(m: &mut FakeGuest, slot: u32, port: i32, team: u8, flags: u16) {
        let addr = PLAYERS + slot * players::Player::SIZE;
        m.put_u16(addr + players::Player::SALT, 0x4321)
            .put_u16(addr + players::Player::FLAGS, flags)
            .put_u32(addr + layout::PLAYER_CONTROLLER, port as u32)
            .put_u8(addr + players::Player::TEAM, team);
    }

    #[test]
    fn menus_campaign_and_ffa_follow_the_primary_profile_index() {
        let mut m = basic();
        let profile_colours = [Some([191, 128, 0]), None, Some([32, 64, 128]), None];
        for (mode, flags) in [
            (crate::level_boot::MODE_MAINMENU, TEAM_PLAY),
            (crate::level_boot::MODE_CAMPAIGN, TEAM_PLAY),
            (crate::level_boot::MODE_MULTIPLAYER, 0),
        ] {
            m.put_u32(OPTIONS + layout::OPTIONS_TYPE, mode)
                .put_u32(FLAGS, flags);
            assert_eq!(colours(&m), profile_colours);
        }
        profile(&mut m, 0, 0);
        profile(&mut m, 2, u8::MAX);
        assert_eq!(colours(&m), [Some([32, 64, 128]), None, None, None]);
    }

    #[test]
    fn a_seats_palette_index_and_any_palette_colour_are_read_from_the_map() {
        let mut m = basic();
        assert_eq!(seat_colour(&m, 0), Some(1));
        assert_eq!(seat_colour(&m, 2), Some(0));
        assert_eq!(seat_colour(&m, 1), None, "not signed in");
        profile(&mut m, 1, 100);
        assert_eq!(seat_colour(&m, 1), None, "outside the retail palette");
        assert_eq!(palette_rgb(&m, 1), Some([191, 128, 0]));
        assert_eq!(palette_rgb(&m, 2), None, "outside this map's block");
        m.put_u32(points::GLOBALS_TAG.base, 0);
        assert_eq!(palette_rgb(&m, 0), None);
    }

    #[test]
    fn teams_use_the_local_controller_port_and_ignore_remote_and_leaving_players() {
        let mut m = teams();
        player(&mut m, 0, -1, 1, 0);
        player(&mut m, 1, 2, 1, 2);
        player(&mut m, 3, 0, 1, 0);
        player(&mut m, 5, 1, 0, 0);
        assert_eq!(
            colours(&m),
            [
                Some([64, 32, 255]),
                Some([128, 255, 191]),
                Some([32, 64, 128]),
                None
            ],
        );
        player(&mut m, 5, 1, 1, 0);
        assert_eq!(colours(&m)[1], Some([64, 32, 255]));
    }

    #[test]
    fn missing_palettes_and_invalid_entries_leave_the_colour_unset() {
        let mut m = teams();
        player(&mut m, 0, 0, 1, 0);
        m.put_u32(
            TAG_ENTRIES + entry_layout::STRIDE + entry_layout::DATUM,
            MULG_DATUM << 1,
        );
        assert_eq!(colours(&m)[0], None, "do not use a stale tag entry");
        m.put_u32(
            TAG_ENTRIES + entry_layout::STRIDE + entry_layout::DATUM,
            MULG_DATUM,
        )
        .put_f32(TEAM_PALETTE + RGB_STRIDE, f32::NAN);
        assert_eq!(colours(&m)[0], None, "non-finite palette component");
        m.put_u32(UNIVERSAL + MULG_UNIVERSAL_TEAM_COLOURS, 1);
        assert_eq!(colours(&m)[0], None, "team index outside the palette");
        m.put_u32(points::GLOBALS_TAG.base, 0);
        assert_eq!(colours(&m), [None; PORTS]);
    }
}
