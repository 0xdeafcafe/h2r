//! Virtual addresses in the retail Halo 2 XBE (title 4D530064, XDK 5849).
//! See `docs/bink-hle.md` for how each one was identified.

// Bink 1.x API entry points in the BINK section. All are `__stdcall`: the
// callee pops its arguments.
pub const BINK_SET_SOUND_SYSTEM: u32 = 0x003e_0450; // (open_fn, ds) ret 8
pub const BINK_OPEN: u32 = 0x003e_0af0; // (path, flags) ret 8
pub const BINK_WAIT: u32 = 0x003e_2330; // (bnk) ret 4
pub const BINK_SET_MIX_BINS: u32 = 0x003e_2630; // (bnk, track, bins, n) ret 16
pub const BINK_SET_MIX_BIN_VOLUMES: u32 = 0x003e_2680; // (bnk, track, bins, vols, n) ret 20
pub const BINK_SET_MEMORY: u32 = 0x003e_2820; // (alloc, free) ret 8
pub const BINK_COPY_TO_BUFFER: u32 = 0x003e_2830; // (bnk, dst, pitch, h, x, y, flags) ret 28
pub const BINK_DO_FRAME: u32 = 0x003e_2870; // (bnk) ret 4
pub const BINK_NEXT_FRAME: u32 = 0x003e_2e50; // (bnk) ret 4
pub const BINK_CLOSE: u32 = 0x003e_2ff0; // (bnk) ret 4
/// Bink's DirectSound open callback, passed to BinkSetSoundSystem.
pub const BINK_OPEN_DIRECT_SOUND: u32 = 0x003e_3c90;

// The engine's movie wrapper (global state at WRAPPER_STATE).
pub const MOVIE_INIT: u32 = 0x0015_5ea0; // BinkSetMemory + mark active
pub const MOVIE_DISPOSE: u32 = 0x0015_5ed0;
pub const MOVIE_STOP_CLOSE: u32 = 0x0015_5f10; // ret 4
pub const MOVIE_IS_PLAYING: u32 = 0x0015_5f60; // -> al
pub const MOVIE_SERVICE: u32 = 0x0015_5f80; // wait/finished/skip poll
pub const MOVIE_TICK: u32 = 0x0015_6040; // per-frame update
/// Start a movie: `(path: *const c_char, flags: u32)`, `ret 8`.
pub const MOVIE_START: u32 = 0x0015_6090;
pub const MOVIE_REMAINING_MS: u32 = 0x0015_64e0;
pub const MOVIE_STOP: u32 = 0x0015_6560;
pub const MOVIE_STOP_ALL: u32 = 0x0015_65a0; // clears flags; called on finish/skip
pub const MOVIE_RELEASE_TEXTURE: u32 = 0x0015_65e0;
pub const MOVIE_DECODE_AND_BLIT: u32 = 0x0015_68d0;
pub const MOVIE_DRAW: u32 = 0x0015_6960;
pub const MOVIE_SKIP_INPUT: u32 = 0x0015_6ab0; // -> al, any skip button down
pub const MOVIE_PATH_BUILDER: u32 = 0x0022_3aa8; // eax=out buf, [esp+4]=name ptr, ret 4

// Wrapper global state, 0xe0 bytes at 0x4e9188.
pub const WRAPPER_STATE: u32 = 0x004e_9188;
pub const ST_ACTIVE: u32 = 0x004e_9188; // u8: wrapper initialised
pub const ST_MOVIE_OPEN: u32 = 0x004e_9189; // u8: a movie was started
pub const ST_READY: u32 = 0x004e_918a; // u8: BinkWait said a frame is due
pub const ST_FINISHED: u32 = 0x004e_918c; // u8: last frame shown
pub const ST_FLAGS: u32 = 0x004e_9190; // u32: start flags
pub const ST_BINK: u32 = 0x004e_9194; // u32: HBINK, 0 when none
pub const ST_WIDTH: u32 = 0x004e_9198; // i16
pub const ST_HEIGHT: u32 = 0x004e_919a; // i16
pub const ST_TEXTURE: u32 = 0x004e_91a0; // u32: D3D texture
