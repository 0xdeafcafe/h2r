//! Named guest globals: a word or a structure at a fixed address in the
//! base (1.0) image, each with the page that records the evidence.
//!
//! This is the one table of global addresses. `guest_layout::points` re-exports
//! it, so `guest_layout::points::GAME_GLOBALS` and `guest_layout::global::GAME_GLOBALS`
//! are the same item; addresses here and hook points there are checked
//! together for duplicates (`guest_layout::points` tests).

/// A named guest global.
#[derive(Debug, PartialEq, Eq)]
pub struct Global {
    pub name: &'static str,
    /// Address in the base (1.0) image.
    pub base: u32,
    /// The page that records the evidence.
    pub doc: &'static str,
}

impl Global {
    /// The pointer the global holds, for globals that hold one, or `None`
    /// while it is null.
    pub fn pointer<M: crate::Mem + ?Sized>(&self, m: &M) -> Result<Option<u32>, crate::Fault> {
        let p = crate::Off::<u32>::new(0).read(m, self.base)?;
        Ok((p != 0).then_some(p))
    }
}

crate::globals! { GLOBALS;
    /// Multiplayer engine state and its per-variant views (§3.7). Points
    /// at [`crate::game::GameEngine`].
    GAME_ENGINE @ 0x004e_9ae8; "docs/game/bots.md";
    CTF_ASSAULT_GLOBALS @ 0x0051_ec80; "docs/game/bots.md";
    ODDBALL_GLOBALS @ 0x0051_ecc4; "docs/game/bots.md";
    KOTH_GLOBALS @ 0x0051_ecc8; "docs/game/bots.md";
    TERRITORIES_GLOBALS @ 0x0051_eccc; "docs/game/bots.md";
    JUGGERNAUT_GLOBALS @ 0x0051_0c9c; "docs/game/bots.md";
    /// Loaded hudg metadata; the motion sensor reads its range at +0x290.
    HUD_GLOBALS_TAG @ 0x0051_0c94; "docs/halo1/hud.md";
    /// Loaded matg metadata. Its multiplayer reference leads to the weapon list.
    GLOBALS_TAG @ 0x004e_034c; "docs/game/bots.md";
    /// Pointer to the game globals ([`crate::game::GameGlobals`]): active
    /// options at `+8`, "map active" at `+0x1120`, the end latch at
    /// `+0x1128`.
    GAME_GLOBALS @ 0x004e_6948; "docs/runtime/level-boot.md";
    /// Pointer to the game-time record ([`crate::game::GameTime`]); `+8` is
    /// the tick count.
    GAME_TIME @ 0x0051_0c54; "docs/game/high-framerate.md";
    /// Pointer to the tick clock `game_tick` advances after its phases when
    /// the update record's run flag is set: `+4` gains the f32 tick length
    /// (`[*GAME_TIME + 4]`), `+0x14` is a counter that falls by one to zero.
    TICK_CLOCK @ 0x0051_0c6c; "docs/engine/sim-rewrite.md";
    /// Pointer to the two shared random seeds: `+0` the game's, `+4` the
    /// effects', each stepped `x * 0x19660d + 0x3c6ef35f` (both the sim
    /// and the render step them).
    RANDOM_SEEDS @ 0x004e_7408; "docs/engine/hs_vm.md";
    /// Pointer to the players datum array (`0x21c`-byte
    /// [`crate::player::Player`]s).
    PLAYERS @ 0x004e_8c24; "docs/live/live-gamestate.md";
    /// Pointer to the objects datum array (`0xc`-byte
    /// [`crate::object::ObjectHeader`]s).
    OBJECTS @ 0x004e_0300; "docs/formats/objects.md";
    /// Pointer to the lights datum array (`0x110`-byte elements, the salt
    /// first: docs/engine/object-update.md, "Slice 5 bodies").
    LIGHTS @ 0x004e_030c; "docs/engine/object-update.md";
    /// Pointer to the loaded scenario tag (`scnr`): player starting
    /// locations at `+0x100` (count) and `+0x104`, `0x34` each.
    SCENARIO @ 0x004e_0350; "docs/editor/scenario-editor.md";
    /// Holds the guest address of the tag-index header.
    TAG_INDEX @ 0x0054_7f00; "docs/engine/tag_get_checked.md";
    /// Holds the guest address of the 16-byte tag-entry table.
    TAG_ENTRIES @ 0x004e_3b44; "docs/engine/tag_get_checked.md";
    /// The Controls screen's vtable (28 slots).
    CONTROLS_SCREEN_VTABLE @ 0x0045_be30; "docs/formats/ui.md";
    /// The Controls list handler's vtable (22 slots).
    CONTROLS_LIST_VTABLE @ 0x0045_d240; "docs/formats/ui.md";
    /// Pointer to the simulation globals: the world's mode at `+8` (0
    /// none, 1 local, 2 sync-server, 3 sync-client, 4 dist-server, 5
    /// dist-client; not the game mode's numbering, see `0x68da8`), the update number at `+0x28`, the out-of-sync latch
    /// byte at `+0x2c`.
    SIM_GLOBALS @ 0x004c_f77c; "docs/live/live-coop.md";
    /// Pointer to the shared simulation entity and event type registry.
    SIM_REPLICATION_REGISTRY @ 0x004c_f784; "docs/game/notes/creature-entity.md";
    /// Generic object entity definition vtable. The probe inherits its object adapter.
    SIM_OBJECT_ENTITY_VTABLE @ 0x0045_2030; "docs/game/notes/creature-entity.md";
    /// Network time override enable byte, with the override milliseconds at +4.
    SIM_NETWORK_TIME_OVERRIDE @ 0x0051_0548; "docs/game/notes/creature-entity.md";
    /// GetTickCount's import cell pointing at the kernel's millisecond counter.
    KERNEL_TICK_COUNT_IMPORT @ 0x0041_1690; "docs/runtime/xapi.md";
    /// The simulation's "ended" latch byte.
    SIM_ENDED @ 0x004c_f772; "docs/live/live-coop.md";
    /// The network life cycle's ready byte.
    NETWORK_READY @ 0x0052_7330; "docs/live/live-custom-games.md";
    /// Pointer to the squad (lobby) network session.
    SQUAD_SESSION @ 0x0052_7364; "docs/live/live-custom-games.md";
    /// The base address of the game-state block (`0x3fe000` bytes).
    GAME_STATE_BASE @ 0x004e_6080; "docs/runtime/checkpoints.md";
    /// Pointer to the players globals ([`crate::game::PlayerGlobals`]): the
    /// local player count (`i16`) at `+8`, the four local users' player
    /// datums at `+0xc` (-1 for none).
    PLAYER_GLOBALS @ 0x004e_8c20; "docs/game/bots.md";
    /// Pointer to sound state. The four local-user listeners start at
    /// `+0x88` and have stride `0x48`.
    SOUND_GLOBALS @ 0x004e_6380; "docs/audio/dsound.md";
    /// UI elapsed milliseconds, accumulated by the UI frame service.
    UI_CLOCK @ 0x0054_d5b8; "docs/audio/bink-hle.md";
    /// UI-clock milliseconds of the last input that resets the idle timer.
    UI_LAST_INPUT @ 0x0054_d5bc; "docs/audio/bink-hle.md";
    /// UI-clock origin set by the title menu and movie wrapper.
    UI_ATTRACT_RESET @ 0x0051_ebec; "docs/audio/bink-hle.md";
    /// Byte the attract timer clears to request a music fade.
    UI_MUSIC_GATE @ 0x0047_01b8; "docs/audio/bink-hle.md";
    /// Attract movie timeout, in UI-clock milliseconds.
    UI_ATTRACT_TIMEOUT @ 0x0047_01bc; "docs/audio/bink-hle.md";
    /// The menu music controller's current and requested modes (+0 and +4).
    UI_MUSIC_STATE @ 0x0054_e7e0; "docs/audio/bink-hle.md";
    /// Pending cache copy flag read by the movie wrapper's busy check.
    CACHE_COPY_PENDING @ 0x0055_bf28; "docs/audio/bink-hle.md";
    /// Pointer to the active looping-sound datum array (0x21a0d6).
    LOOPING_SOUNDS @ 0x0050_2110; "docs/runtime/agent-api.md";
    /// Pointer to the sound instance datum array (0x1254c3).
    SOUND_INSTANCES @ 0x004e_637c; "docs/runtime/agent-api.md";
    /// Pointer to the sound-channel records (0x129f20).
    SOUND_CHANNELS @ 0x004e_6378; "docs/runtime/agent-api.md";
    /// Pointer to the sound-driver globals, which hold its stream records (0x21f8a0).
    SOUND_DRIVER @ 0x0051_ebe4; "docs/runtime/agent-api.md";
    /// The loaded map's short name in the cache header (`lockout`), 32
    /// bytes, NUL-terminated.
    CACHE_MAP_NAME @ 0x0054_7898; "docs/live/live-gamestate.md";
    /// The skull array: one byte per skull, 1 = on (15). Written only by
    /// the pickup (`0x1df3e0`) and our campaign start; every reader gates
    /// on a campaign.
    SKULLS @ 0x004f_55dc; "docs/game/skulls.md";
    /// Pointer to the AI globals: byte `+0` is "AI on" (the script's `ai`
    /// switch), byte `+1` is set once AI is initialised for the map.
    /// `ai_update` (`0x1c83c0`) needs both, and never runs on a
    /// distributed client.
    AI_GLOBALS @ 0x004f_55d0; "docs/game/notes/ai-networking.md";
    /// Pointer to the actor datum array (`0x888`-byte actors).
    ACTORS @ 0x004f_55f0; "docs/live/live-gamestate.md";
    /// likely: the four local player slots, one per controller, `0xc70`
    /// apart; a signed-in slot embeds its `0x1e0`-byte profile at `+0x18`.
    PLAYER_SLOTS @ 0x0054_e8e0; "docs/live/live-gamestate.md";
    /// Pointer to the `0x24`-byte cinematic globals: `+5` is "a cinematic
    /// is playing", the byte the game itself asks (`0x0013cb40`).
    CINEMATIC_GLOBALS @ 0x0051_0c50; "docs/live/live-gamestate.md";
    /// Pointer to the physics component datum array (`0xa0`-byte
    /// components; an object's index at body `+0xb4`).
    PHYSICS_COMPONENTS @ 0x0051_e9b8; "docs/engine/havok-runtime.md";
    /// Pointer to the Havok world (`0x001c2f07` stores it after
    /// `0x003101c0`).
    HAVOK_WORLD @ 0x0051_e9a4; "docs/engine/havok-runtime.md";
    /// The engine table: one engine object pointer per record engine
    /// index (`+0x44`), read at `[ENGINE_TABLE + 4 * E+0xc14]`. Filled by
    /// `0x378500`; 5, 6 and 10 are zero. An object's only dword is its
    /// vtable of 51 thiscall slots.
    ENGINE_TABLE @ 0x0055_e4d0; "docs/game/game-types-without-carriers.md";
    /// Slayer's engine vtable (51 slots); its object is `0x47fc80`.
    SLAYER_VTABLE @ 0x0045_c6d8; "docs/engine/notes/2026-11-21-engine-vtables.md";
}

/// The global at `base`, if the table names one.
pub fn by_base(base: u32) -> Option<&'static Global> {
    GLOBALS.iter().copied().find(|g| g.base == base)
}
