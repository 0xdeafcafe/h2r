//! The game's own globals: options and variant, game time, the local
//! players, and the multiplayer engine's statistics
//! (docs/live/live-gamestate.md, docs/runtime/level-boot.md).
//!
//! Each structure is reached through the pointer a [`crate::global`]
//! holds: [`GameGlobals::current`] reads [`crate::global::GAME_GLOBALS`].

use crate::field::Datum;
use crate::global::{self, Global};
use crate::mem::{Fault, Mem};
use crate::View;

/// The view at the pointer `global` holds, `None` while it is null.
pub fn through<V: View, M: Mem + ?Sized>(m: &M, global: &Global) -> Result<Option<V>, Fault> {
    Ok(global.pointer(m)?.map(V::at))
}

crate::layout! {
    /// The game globals (`*GAME_GLOBALS`).
    pub struct GameGlobals: 0x112c {
        /// The active structure-BSP index (confirmed: `0x0011c1b0` and
        /// `0x00137d40` index the scenario's BSP block with it).
        ACTIVE_BSP active_bsp @ 0x2: i16,
        /// "A map is active" (docs/runtime/level-boot.md).
        MAP_ACTIVE map_active @ 0x1120: u8,
        /// The end latch: set when the game has ended.
        GAME_ENDED game_ended @ 0x1128: u8,
    }
}

/// The active [`GameOptions`], inside the game globals.
pub const OPTIONS: u32 = 0x8;

impl GameGlobals {
    /// The game globals the global points at, `None` while it is null.
    pub fn current<M: Mem + ?Sized>(m: &M) -> Result<Option<Self>, Fault> {
        through(m, &global::GAME_GLOBALS)
    }
    /// The active options.
    pub const fn options(self) -> GameOptions {
        GameOptions::at(self.addr.wrapping_add(OPTIONS))
    }
}

crate::layout! {
    /// Game options: what the level boot was asked for.
    pub struct GameOptions: 0x134 {
        /// The game mode ([`MODE_CAMPAIGN`], [`MODE_MULTIPLAYER`],
        /// [`MODE_MAIN_MENU`]).
        MODE mode @ 0x0: u32,
        /// Ticks a second.
        TICK_RATE tick_rate @ 0x6: u16,
        /// Campaign difficulty: 0 easy, 1 normal, 2 heroic, 3 legendary.
        /// Confirmed: `0x00138110` defaults it to 1; the menu's campaign
        /// start (`0x0023643f`) sets it.
        DIFFICULTY difficulty @ 0x12a: i16,
    }
}

pub const MODE_CAMPAIGN: u32 = 1;
pub const MODE_MULTIPLAYER: u32 = 2;
pub const MODE_MAIN_MENU: u32 = 3;
/// The game variant, inside the options.
pub const VARIANT: u32 = 0x134;

impl GameOptions {
    pub const fn variant(self) -> GameVariant {
        GameVariant::at(self.addr.wrapping_add(VARIANT))
    }
}

crate::layout! {
    /// The multiplayer game variant, inside the options.
    pub struct GameVariant: 0x58 {
        /// The game engine (slayer, ctf, ...), `i32`.
        ENGINE engine @ 0x44: i32,
        /// Variant flags: bit 0 is team play.
        FLAGS flags @ 0x48: u32,
        SCORE_LIMIT score_limit @ 0x50: i32,
        /// Seconds.
        TIME_LIMIT time_limit @ 0x54: i32,
    }
}

/// [`GameVariant::FLAGS`] bit 0: team play.
pub const TEAM_PLAY: u32 = 1;

crate::layout! {
    /// The game-time record (`*GAME_TIME`).
    pub struct GameTime: 0xc {
        /// Ticks a second.
        TICK_RATE tick_rate @ 0x2: i16,
        /// Seconds a tick (the projectile update loads it, `0x000f8ee2`).
        SECONDS_PER_TICK seconds_per_tick @ 0x4: f32,
        /// Ticks since the game started.
        TICK tick @ 0x8: u32,
    }
}

impl GameTime {
    pub fn current<M: Mem + ?Sized>(m: &M) -> Result<Option<Self>, Fault> {
        through(m, &global::GAME_TIME)
    }
}

crate::layout! {
    /// The players globals (`*PLAYER_GLOBALS`).
    pub struct PlayerGlobals: 0x1c {
        /// Local players.
        LOCAL_COUNT local_count @ 0x8: i16,
        /// The four local users' player datums (none for an unmapped user).
        LOCAL_PLAYERS local_players @ 0xc: [Datum; 4],
    }
}

impl PlayerGlobals {
    pub fn current<M: Mem + ?Sized>(m: &M) -> Result<Option<Self>, Fault> {
        through(m, &global::PLAYER_GLOBALS)
    }
}

crate::layout! {
    /// The multiplayer engine globals (`*GAME_ENGINE`): only the parts read.
    pub struct GameEngine: 0xf4 {
        /// likely: the tick the round started.
        ROUND_START_TICK round_start_tick @ 0x70: u32,
        /// Four local users' black-overlay opacities.
        BLACK_OPACITY black_opacity @ 0xe4: [f32; 4],
    }
}

/// The statistics block, inside the engine globals.
pub const STATBORG: u32 = 0x304;
/// Per-player statistics ([`PlayerStats`], [`PLAYER_STATS_STRIDE`] apart).
pub const PLAYER_STATS: u32 = 0x308;
pub const PLAYER_STATS_STRIDE: u32 = 0x1c;
/// Per-team statistics ([`TeamStats`], [`TEAM_STATS_STRIDE`] apart).
pub const TEAM_STATS: u32 = 0x4c8;
pub const TEAM_STATS_STRIDE: u32 = 0x12;

impl GameEngine {
    pub fn current<M: Mem + ?Sized>(m: &M) -> Result<Option<Self>, Fault> {
        through(m, &global::GAME_ENGINE)
    }
    /// Player `index`'s statistics.
    pub const fn player_stats(self, index: u32) -> PlayerStats {
        PlayerStats::at(
            self.addr
                .wrapping_add(PLAYER_STATS)
                .wrapping_add(index.wrapping_mul(PLAYER_STATS_STRIDE)),
        )
    }
    /// Team `index`'s statistics.
    pub const fn team_stats(self, index: u32) -> TeamStats {
        TeamStats::at(
            self.addr
                .wrapping_add(TEAM_STATS)
                .wrapping_add(index.wrapping_mul(TEAM_STATS_STRIDE)),
        )
    }
}

crate::layout! {
    /// One player's statistics in the engine globals.
    pub struct PlayerStats: 0x1c {
        SCORE score @ 0x0: i16,
        KILLS kills @ 0x4: u16,
        DEATHS deaths @ 0x6: u16,
        SUICIDES suicides @ 0x8: u16,
        ASSISTS assists @ 0xc: u16,
    }
}

crate::layout! {
    /// One team's statistics in the engine globals: the same leading
    /// fields as a player's.
    pub struct TeamStats: 0x12 {
        SCORE score @ 0x0: i16,
        KILLS kills @ 0x4: u16,
        DEATHS deaths @ 0x6: u16,
        SUICIDES suicides @ 0x8: u16,
        ASSISTS assists @ 0xc: u16,
    }
}
