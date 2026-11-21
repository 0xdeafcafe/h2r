//! The actor record used by the AI snapshot and the damage observer.

crate::record! {
    /// An actor in the pool at [`crate::global::ACTORS`]. The pool is
    /// created at `0x001dfb24` with capacity for 256 records.
    pub struct Actor: 0x888 {
        // 0x001e0dab also installs the reciprocal actor handle in the unit.
        unit @ 0x18: u32,
        team @ 0x24: i16,
        // Squad assignment at 0x00203e84, scenario squad index.
        squad @ 0x30: i32,
        character @ 0x54: u32,
        // ai_combat_status takes the maximum of these (0x00276070).
        combat_status @ 0x86: i16,
        // 0x001a813d reads the top of four 64-byte behavior frames.
        behavior_depth @ 0x190: i16,
        // 0x0026c19d checks the cache before refreshing position and sector.
        location_cached @ 0x278: u8,
        sector @ 0x28c: i32,
        focus @ 0x338: Option<crate::DatumHandle>,
    }
}
