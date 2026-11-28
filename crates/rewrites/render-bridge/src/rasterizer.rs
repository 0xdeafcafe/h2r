//! The rasterizer's start-up and shutdown in Rust, with no device
//! (docs/graphics/d3d-entries.md, "Start-up and shutdown").
//!
//! Every guest function on the path from `game_main` (`0x12190`) to a D3D
//! entry at start-up or shutdown is an implementation here
//! ([`Rasterizer`]), so its body never runs: what the game and the native
//! renderer read afterwards is written as the guest's body leaves it, and
//! everything that only fed the D3D device is left out. The game's own
//! functions under them are called as they are ([`Builder::calls_guest`]).
//!
//! | Address | Kept | Dropped |
//! |---|---|---|
//! | `0x12420` boot display | `0x123b0`, `[0x5093b0]` back to 0 | the first `Swap(0)`, the device's release |
//! | `0x12490` boot device | `[0x509350]` = 1, `[0x5093b0]`, the `0x47ffd8` table copy | `CreateDevice` |
//! | `0x12560` rasterizer | every game call, the state words below, `[0x484dd0]`, `[0x51f408]`, the `0x4b72b4` view, `[0x50934c]` | the push-buffer sizes `[0x40864c]`, `[0x408648]`; `SetPalette` x4; `SetShaderConstantMode`; the NV2A pushes of `SetRenderState_Simple`; the dirty word `[0x407490]` |
//! | `0x128c0` game device | the mode words `0x485ac8..`, `[0x5093b0]`, the table copy, `[0x485acc]`'s clamp, `[0x467008]`, `[0x46700c]`, `[0x5093bc]`; the device's surfaces ([`surfaces`]) | `CreateDevice` / `Reset` and their parameters |
//! | `0x14600` frame buffers | the globals `0x509354..0x509370` over [`surfaces`] | `AddRef` |
//! | `0x18900` state block | each state's slot of the render-state words | the pushes, the dirty word |
//! | `0x1d0e0` default textures | the three textures and their checker texels | `BlockOnResource` |
//! | `0x1d3f0` palette | the palette and its colours | `BlockOnTime` |
//! | `0x141c0` shutdown | the fade record cleared, every game call, `[0x5093b0]`, `[0x509350]` | the device's release |
//! | `0x14980`, `0x14a60`, `0x1d2a0`, `0x1359d0` | the globals and fields cleared, `0x1359d0`'s unlink and frees | `Release` |
//!
//! The render-state words (`D3D__RenderState` `0x407498`, `D3D__TextureState`
//! `0x404dd0`) are kept: game code reads them back and the native draws take
//! their state from them (`game::native_targets_glue::render_state`), so a
//! store here is the store the setter's answer makes
//! ([`crate::device_state`]).
//!
//! The device's surfaces are three guest headers in this feature's own
//! memory ([`surfaces`]): the back buffer, the display surface and the
//! depth buffer. Their `Data` words name no RAM ([`BACK_DATA`],
//! [`DISPLAY_DATA`], [`DEPTH_DATA`]), so a CPU read through one faults; a
//! header with no game role is the scene's own colour or depth
//! (`game::native_targets_glue::target_ref`). `[0x5093b0]`, the game's
//! device pointer, holds the block's address: game code only tests it.
//!
//! A case the bodies do not cover panics, named `rasterizer: <cause>`.

use engine::entry::{Cx, EntryError, EntryFuture, EntryResult, GuestCall, Regs, Ret};
use engine::memory::{GuestMemory, MemFault};
use hooks::points::{self, Point};
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use runtime::guest::Permissions;

use crate::device_state;

pub const ID: &str = "rasterizer";

/// The game's device pointer: nonzero once a display exists.
pub const GAME_DEVICE: u32 = 0x0050_93b0;
/// Set by the boot device, cleared at shutdown.
pub const BOOT_DEVICE: u32 = 0x0050_9350;
/// The display mode words `0x128c0` takes from its argument.
pub const MODE: u32 = 0x0048_5ac8;
/// The mode's kind (a short) and the refresh index `0x128c0` clamps.
pub const MODE_KIND: u32 = 0x0048_5aca;
pub const MODE_INDEX: u32 = 0x0048_5acc;
/// The mode's depth byte: `F24S8` when set, else `D24S8`.
pub const MODE_FLOAT_DEPTH: u32 = MODE;
/// The window shorts (top, left, bottom, right) `0x16a30` writes.
pub const WINDOW: u32 = 0x0048_5a8a;
/// The table both device creates copy, `0x35` dwords.
pub const MODE_TABLE: u32 = 0x0047_ffd8;
pub const MODE_TABLE_COPY: u32 = 0x0048_4ce8;
pub const MODE_TABLE_WORDS: u32 = 0x35;
/// `0x128c0`'s two `-1` words and its rebuild flag (`0x12a50` rebuilds
/// the frame buffers on an odd frame while it is set).
pub const MODE_WORDS: [u32; 2] = [0x0046_7008, 0x0046_700c];
pub const REBUILD: u32 = 0x0050_93bc;

/// The frame buffers' globals (`0x14600`): the device's back, display and
/// depth surfaces; the textures over the back and display surfaces; the
/// depth as colour and as swapped colour; the colour surface over the
/// depth's memory.
pub const BACK: u32 = 0x0050_935c;
pub const DISPLAY: u32 = 0x0050_9360;
pub const DEPTH: u32 = 0x0050_9364;
pub const FRAME_TEXTURES: [u32; 2] = [0x0050_9354, 0x0050_9358];
pub const DEPTH_TEXTURE: u32 = 0x0050_9368;
pub const DEPTH_SWAPPED: u32 = 0x0050_936c;
pub const DEPTH_ALIAS: u32 = 0x0050_9370;
/// `0x14850`'s textures and their level-0 surfaces (`0x14a60`).
pub const SMALL_TARGETS: [u32; 4] = [0x0050_9378, 0x0050_9380, 0x0050_9374, 0x0050_937c];
/// `0x1d0e0`'s textures: 2D, volume, cube.
pub const DEFAULT_TEXTURES: [u32; 3] = [0x0048_5ae4, 0x0048_5ae8, 0x0048_5aec];

/// The fade record `[0x5093e0]`, its `0xff` words, and the target-copy
/// gate and flag `0x141c0` sets (as `0x1fcd0`, `crate::frame_tail`).
const FADE_RECORD: u32 = 0x0050_93e0;
const FADE_RECORD_WORDS: u32 = 0xff;
const FADE_ALLOCATED: u32 = 0x004b_5690;
const FADE_REFUSED: u32 = 0x004b_569d;

/// The frame buffers' `Data` words: above any Xbox RAM.
pub const BACK_DATA: u32 = 0x0C00_0000;
pub const DISPLAY_DATA: u32 = 0x0E00_0000;
pub const DEPTH_DATA: u32 = 0x0D00_0000;

/// The headers' words. A surface is `Common`, `Data`, `Lock`, `Format`,
/// `Size`, `Parent` (`0x18` bytes); a texture the first five (`0x14`).
const SURFACE: u32 = 0x18;
const TEXTURE: u32 = 0x14;
/// A surface's `Common` with one reference (`D3DCOMMON_TYPE_SURFACE`).
const SURFACE_COMMON: u32 = 0x0005_0001;
/// `0x14600`'s texture words: `Common`, the 640 x 480 `Size`, and the
/// formats `LIN_A8R8G8B8` and `LIN_A8B8G8R8`.
const TEXTURE_COMMON: u32 = 0x0004_0001;
const TEXTURE_SIZE: u32 = 0x271d_f27f;
const LIN_A8R8G8B8: u32 = 0x0001_1229;
const LIN_A8B8G8R8: u32 = 0x0001_3f29;
/// The depth surfaces' formats: `LIN_D24S8`, `LIN_F24S8`.
const LIN_D24S8: u32 = 0x0001_2e29;
const LIN_F24S8: u32 = 0x0001_2f29;

/// Offsets in the feature's block ([`BLOCK_LEN`]).
pub const BLOCK_BACK: u32 = 0;
pub const BLOCK_DISPLAY: u32 = SURFACE;
pub const BLOCK_DEPTH: u32 = 2 * SURFACE;
const BLOCK_FRAME_TEXTURES: [u32; 2] = [3 * SURFACE, 3 * SURFACE + TEXTURE];
const BLOCK_DEPTH_ALIAS: u32 = 3 * SURFACE + 2 * TEXTURE;
const BLOCK_DEPTH_TEXTURE: u32 = BLOCK_DEPTH_ALIAS + SURFACE;
const BLOCK_DEPTH_SWAPPED: u32 = BLOCK_DEPTH_TEXTURE + TEXTURE;
pub const BLOCK_LEN: u32 = BLOCK_DEPTH_SWAPPED + TEXTURE;

/// The state words.
const RENDER_STATE: u32 = device_state::RENDER_STATE;
/// `0x12560`'s own stores, in its order: `(address, value)` after each
/// `SetRenderState_Simple` (`0x1269a..0x12783`).
const INIT_STATES: [(u32, u32); 13] = [
    (0x0040_7598, 1),
    (0x0040_757c, 0x203),
    (0x0040_75dc, 0),
    (0x0040_7588, 0),
    (0x0040_7580, 0x204),
    (0x0040_758c, 0),
    (0x0040_7584, 0),
    (0x0040_7590, 1),
    (0x0040_7594, 0),
    (0x0040_75c0, 0x8006),
    (0x0040_7608, 1),
    (0x0040_7630, 0),
    (0x0040_7634, 1),
];
/// `SetRenderState_ZEnable`'s slot and `0x12560`'s value.
const ZENABLE: (u32, u32) = (143, 2);
/// `D3DTSS_TEXCOORDINDEX`: stage `s` gets `s` (`0x1278d..0x127b1`).
const TEXCOORDINDEX: u32 = 0x1c;

/// The state block's setter table (`0x18978`) and the first state that
/// goes through it; the states below it are stored in their slot.
const STATE_SETTERS: u32 = 0x0040_38d8;
const FIRST_SETTER_STATE: u32 = 0x88;

/// The palette sizes by kind (`0x1d452`).
const PALETTE_BYTES: u32 = 0x0043_e8e8;
/// The default palette's kind, colours and header (`0x125f9..0x12608`).
const PALETTE_KIND: u32 = 3;
const PALETTE_SOURCE: u32 = 0x0046_8848;
pub const PALETTE: u32 = 0x0048_4dbc;

/// The format table the lock reads bits per texel from (`0x3fdbcc`).
const FORMAT_BITS: u32 = 0x0040_3a50;
/// The default textures' checker texels (`0x1d174`).
const CHECKER: [u16; 2] = [0x0f00, 0xf0f0];

fn word(mem: &dyn GuestMemory, at: u32) -> Result<u32, MemFault> {
    mem.u32(at)
}

fn short(mem: &dyn GuestMemory, at: u32) -> Result<i32, MemFault> {
    Ok(i32::from(mem.u16(at)? as i16))
}

fn copy_words(mem: &mut dyn GuestMemory, from: u32, to: u32, words: u32) -> Result<(), MemFault> {
    for i in 0..words {
        let v = mem.u32(from + 4 * i)?;
        mem.set_u32(to + 4 * i, v)?;
    }
    Ok(())
}

fn zero_words(mem: &mut dyn GuestMemory, at: u32, words: u32) -> Result<(), MemFault> {
    mem.write(at, &vec![0; 4 * words as usize])
}

/// The device's surfaces at `block`: the back buffer, display surface and
/// depth buffer headers, sized by the window `0x16a30` left
/// (`0x128f5..0x1291f`: width `right - left`, height `bottom - top`), the
/// depth `F24S8` when the mode's byte is set (`0x128f3`).
pub fn surfaces(mem: &mut dyn GuestMemory, block: u32) -> Result<(), MemFault> {
    let width = short(mem, WINDOW + 6)? - short(mem, WINDOW + 2)?;
    let height = short(mem, WINDOW + 4)? - short(mem, WINDOW)?;
    if width <= 0 || height <= 0 || width > 0x1000 || height > 0x1000 {
        unimplemented!("rasterizer: a {width} x {height} window");
    }
    let (w, h) = (width as u32, height as u32);
    // Linear, four bytes a texel: the pitch in 64-byte units.
    let size = (w - 1) | (h - 1) << 12 | ((w * 4).div_ceil(64) - 1) << 24;
    let depth = if mem.u8(MODE_FLOAT_DEPTH)? != 0 { LIN_F24S8 } else { LIN_D24S8 };
    for (at, data, format) in [
        (BLOCK_BACK, BACK_DATA, LIN_A8R8G8B8),
        (BLOCK_DISPLAY, DISPLAY_DATA, LIN_A8R8G8B8),
        (BLOCK_DEPTH, DEPTH_DATA, depth),
    ] {
        for (i, v) in [SURFACE_COMMON, data, 0, format, size, 0].into_iter().enumerate() {
            mem.set_u32(block + at + 4 * i as u32, v)?;
        }
    }
    Ok(())
}

/// `0x128c0`, `mode` its `EAX`: the mode words, the device (its surfaces
/// at `block`), then what the body leaves on success. Always succeeds.
pub fn game_device_create(mem: &mut dyn GuestMemory, block: u32, mode: u32) -> Result<bool, MemFault> {
    copy_words(mem, mode, MODE, 3)?;
    // `0x1292a..0x1295b`: a kind past 2 becomes 1.
    if !(0..=2).contains(&short(mem, MODE_KIND)?) {
        mem.set_u16(MODE_KIND, 1)?;
    }
    surfaces(mem, block)?;
    mem.set_u32(GAME_DEVICE, block)?;
    let index = word(mem, MODE_INDEX)? as i32;
    copy_words(mem, MODE_TABLE, MODE_TABLE_COPY, MODE_TABLE_WORDS)?;
    if !(0..=5).contains(&index) {
        mem.set_u32(MODE_INDEX, 3)?;
    }
    for at in MODE_WORDS {
        mem.set_u32(at, u32::MAX)?;
    }
    mem.set_u8(REBUILD, 1)?;
    Ok(true)
}

/// `0x12490`: the boot device. Always succeeds.
pub fn boot_device_create(mem: &mut dyn GuestMemory, block: u32) -> Result<bool, MemFault> {
    mem.set_u32(BOOT_DEVICE, 1)?;
    mem.set_u32(GAME_DEVICE, block)?;
    copy_words(mem, MODE_TABLE, MODE_TABLE_COPY, MODE_TABLE_WORDS)?;
    Ok(true)
}

/// `0x14600`: the frame buffers' globals over the device's surfaces at
/// `block`, and the textures over them (in `block`, written afresh each
/// call). Always succeeds.
pub fn frame_buffers(mem: &mut dyn GuestMemory, block: u32) -> Result<bool, MemFault> {
    let (back, display, depth) = (block + BLOCK_BACK, block + BLOCK_DISPLAY, block + BLOCK_DEPTH);
    mem.set_u32(BACK, back)?;
    mem.set_u32(DISPLAY, display)?;
    mem.set_u32(DEPTH, depth)?;
    let depth_data = word(mem, depth + 4)?;
    let texture = |mem: &mut dyn GuestMemory, at: u32, data: u32, format: u32| -> Result<(), MemFault> {
        for (i, v) in [TEXTURE_COMMON, data, 0, format, TEXTURE_SIZE].into_iter().enumerate() {
            mem.set_u32(at + 4 * i as u32, v)?;
        }
        Ok(())
    };
    for (global, (at, surface)) in FRAME_TEXTURES.into_iter().zip([(BLOCK_FRAME_TEXTURES[0], back), (BLOCK_FRAME_TEXTURES[1], display)]) {
        let data = word(mem, surface + 4)?;
        texture(mem, block + at, data, LIN_A8R8G8B8)?;
        mem.set_u32(global, block + at)?;
    }
    // `0x14776..0x147ab`: the back buffer's header with the depth's `Data`.
    let alias = block + BLOCK_DEPTH_ALIAS;
    copy_words(mem, back, alias, SURFACE / 4)?;
    mem.set_u32(alias + 4, depth_data)?;
    mem.set_u32(DEPTH_ALIAS, alias)?;
    texture(mem, block + BLOCK_DEPTH_TEXTURE, depth_data, LIN_A8R8G8B8)?;
    mem.set_u32(DEPTH_TEXTURE, block + BLOCK_DEPTH_TEXTURE)?;
    texture(mem, block + BLOCK_DEPTH_SWAPPED, depth_data, LIN_A8B8G8R8)?;
    mem.set_u32(DEPTH_SWAPPED, block + BLOCK_DEPTH_SWAPPED)?;
    Ok(true)
}

/// `0x14980`: the frame buffers' globals cleared. The headers are the
/// block's, so nothing is freed.
pub fn frame_buffers_dispose(mem: &mut dyn GuestMemory) -> Result<(), MemFault> {
    for at in [BACK, DISPLAY, DEPTH, FRAME_TEXTURES[0], FRAME_TEXTURES[1], DEPTH_TEXTURE, DEPTH_SWAPPED, DEPTH_ALIAS] {
        mem.set_u32(at, 0)?;
    }
    Ok(())
}

/// `0x14a60` and `0x1d2a0`: the globals cleared. `Release` freed the
/// memory at its last reference; only the shutdown calls them.
pub fn clear(mem: &mut dyn GuestMemory, globals: &[u32]) -> Result<(), MemFault> {
    for &at in globals {
        mem.set_u32(at, 0)?;
    }
    Ok(())
}

/// The stores `0x12560` makes once the palette is made
/// (`0x12670..0x127b1`): `ZEnable` 2, its own words, `ZBias` 0 and each
/// stage's texture coordinate index.
pub fn init_states(mem: &mut dyn GuestMemory) -> Result<(), MemFault> {
    mem.set_u32(device_state::render_state(ZENABLE.0), ZENABLE.1)?;
    let [unsigned, slope] = device_state::ZBIAS_FLOATS;
    let zbias = device_state::zbias_words(0, word(mem, unsigned)?, word(mem, slope)?);
    // `0x1269a..0x126b4`: two words, then `ZBias`, then the rest.
    for &(at, v) in &INIT_STATES[..2] {
        mem.set_u32(at, v)?;
    }
    for (slot, v) in zbias {
        mem.set_u32(device_state::render_state(slot), v)?;
    }
    for &(at, v) in &INIT_STATES[2..] {
        mem.set_u32(at, v)?;
    }
    for stage in 0..4 {
        mem.set_u32(device_state::texture_state(stage, TEXCOORDINDEX), stage)?;
    }
    Ok(())
}

/// `0x18900(block, entries)`: each of the block's states stored in its
/// slot. `entries` points at a short: the first entry's index (low 9
/// bits) and the count; the entries are 5 bytes (state, value) from
/// `[[block + 0x20] + 0x14]`. A state from `0x88` stores what its setter
/// stores ([`device_state::setter`]).
pub fn state_block(mem: &mut dyn GuestMemory, block: u32, entries: u32) -> Result<(), MemFault> {
    let head = u32::from(mem.u16(entries)?);
    let table = word(mem, word(mem, block + 0x20)? + 0x14)?;
    let first = table.wrapping_add(5 * (head & 0x1ff));
    for i in 0..head >> 9 {
        let at = first.wrapping_add(5 * i);
        let state = u32::from(mem.u8(at)?);
        let value = word(mem, at + 1)?;
        if state < FIRST_SETTER_STATE {
            mem.set_u32(RENDER_STATE + 4 * state, value)?;
            continue;
        }
        let setter = word(mem, STATE_SETTERS + 4 * state)?;
        match device_state::setter(setter) {
            Some(device_state::Setter::RenderState { slot }) => mem.set_u32(device_state::render_state(slot), value)?,
            Some(device_state::Setter::ZBias) => {
                let [unsigned, slope] = device_state::ZBIAS_FLOATS;
                for (slot, v) in device_state::zbias_words(value, word(mem, unsigned)?, word(mem, slope)?) {
                    mem.set_u32(device_state::render_state(slot), v)?;
                }
            }
            _ => unimplemented!("rasterizer: state {state:#x}'s setter {setter:#x} in a state block"),
        }
    }
    Ok(())
}

/// The byte offset of `face`'s level 0 in the texture at `header`, as the
/// lock `0x3fdbc0` computes it for a swizzled texture: the faces' level
/// chains, each rounded up to 128 bytes.
pub fn face_offset(mem: &dyn GuestMemory, header: u32, face: u32) -> Result<u32, MemFault> {
    if face == 0 {
        return Ok(0);
    }
    if word(mem, header + 0x10)? != 0 {
        unimplemented!("rasterizer: a linear cube texture at {header:#x}");
    }
    let format = word(mem, header + 0xc)?;
    let kind = format >> 8 & 0xff;
    let bits = u32::from(mem.u8(FORMAT_BITS + kind)?) & 0x3c;
    // Compressed formats keep 4 x 4 blocks (`0x3fdc10..0x3fdc29`).
    let min = if kind == 0xc || (0xe..=0xf).contains(&kind) { 2 } else { 0 };
    let (mut w, mut h) = (format >> 20 & 0xf, format >> 24 & 0xf);
    let mut chain = 0u32;
    for _ in 0..format >> 16 & 0xf {
        chain += (1u32 << (w.max(min) + h.max(min))) * bits >> 3;
        w = w.saturating_sub(1);
        h = h.saturating_sub(1);
    }
    Ok(((chain + 0x7f) & !0x7f) * face)
}

/// The default textures' texels (`0x1d174..0x1d286`): the 2D texture's 16,
/// the volume's 64 and each of the cube's six faces' 16, alternating
/// [`CHECKER`].
pub fn checker(mem: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let fill = |mem: &mut dyn GuestMemory, at: u32, texels: u32| -> Result<(), MemFault> {
        for i in 0..texels {
            mem.set_u16(at + 2 * i, CHECKER[(i & 1) as usize])?;
        }
        Ok(())
    };
    let data = |mem: &dyn GuestMemory, header: u32| -> Result<u32, MemFault> { Ok(word(mem, header + 4)? | 0x8000_0000) };
    let [flat, volume, cube] = DEFAULT_TEXTURES.map(|g| mem.u32(g));
    let (flat, volume, cube) = (flat?, volume?, cube?);
    let at = data(&*mem, flat)?;
    fill(mem, at, 0x10)?;
    let at = data(&*mem, volume)?;
    fill(mem, at, 0x40)?;
    for face in 0..6 {
        let at = data(&*mem, cube)? + face_offset(&*mem, cube, face)?;
        fill(mem, at, 0x10)?;
    }
    Ok(())
}

/// `0x1d3f0` palette copy: `bytes` from `source` into the palette at
/// `header`'s memory.
pub fn palette_fill(mem: &mut dyn GuestMemory, header: u32, kind: u32, source: u32) -> Result<(), MemFault> {
    let to = word(mem, header + 4)? | 0x8000_0000;
    let bytes = word(mem, PALETTE_BYTES + 4 * kind)?;
    let mut buf = vec![0; bytes as usize];
    mem.read(source, &mut buf)?;
    mem.write(to, &buf)
}

fn al(r: &Regs) -> bool {
    r.eax as u8 != 0
}

async fn call(cx: &Cx, c: GuestCall) -> Result<Regs, EntryError> {
    cx.call(c).await
}

async fn plain(cx: &Cx, p: &Point) -> Result<Regs, EntryError> {
    call(cx, GuestCall::new(p.base)).await
}

/// Bytes below the entry `ESP` a body keeps for an out word its callee
/// writes.
const LOCALS: u32 = 0x10;

/// `0x1d3f0`: a palette of `kind` from `source` (none: left as made),
/// its header to `out`.
async fn palette(cx: &Cx, kind: u32, source: u32, out: u32) -> Result<bool, EntryError> {
    let local = cx.entry().esp.wrapping_sub(4);
    cx.write(|m| m.set_u32(local, 0))?;
    let r = call(cx, GuestCall::new(points::PALETTE_ALLOCATE.base).eax(kind).ebx(local).reserve(LOCALS)).await?;
    let header = cx.read(|m| m.u32(local))?;
    if al(&r) && source != 0 {
        cx.write(|m| palette_fill(m, header, kind, source))?;
    }
    cx.write(|m| m.set_u32(out, header))?;
    Ok(al(&r))
}

/// `0x1d0e0`: the 2D, volume and cube textures, then their texels.
async fn default_textures(cx: &Cx) -> Result<bool, EntryError> {
    let [flat, volume, cube] = DEFAULT_TEXTURES;
    let made = [
        GuestCall::new(points::TEXTURE_ALLOCATE.base).eax(0).ecx(9).ebx(flat).args([4, 4, 0, 0]),
        GuestCall::new(points::VOLUME_ALLOCATE.base).eax(9).ecx(0).ebx(volume).args([4, 4, 4, 0]),
        GuestCall::new(points::CUBE_ALLOCATE.base).eax(4).ecx(9).ebx(cube).args([0, 0]),
    ];
    for c in made {
        if !al(&call(cx, c).await?) {
            return Ok(false);
        }
    }
    if cx.read(|m| DEFAULT_TEXTURES.iter().any(|&g| m.u32(g).map_or(true, |v| v == 0))) {
        return Ok(false);
    }
    cx.write(checker)?;
    Ok(true)
}

/// `0x12560`.
async fn initialize(cx: Cx, block: u32) -> EntryResult {
    let done = cx.read(|m| m.u8(0x0048_5a88))?;
    if done != 0 {
        return Ok(Ret::new(u32::from(done), 0));
    }
    plain(&cx, &points::DISPLAY_DEFAULTS).await?;
    let small = cx.read(|m| -> Result<bool, MemFault> { Ok(m.u8(0x0048_5ac5)? != 0 && m.u8(0x0048_5ac6)? != 0) })?;
    let (x, y) = if small { (0x20, 0x18) } else { (0x30, 0x24) };
    let one = cx.read(|m| m.u32(0x0045_dbc0))?;
    call(&cx, GuestCall::new(points::DISPLAY_WINDOW.base).ecx(y).edx(x).xmm0(u128::from(one))).await?;
    let mut ok = cx.write(|m| game_device_create(m, block, 0x0043_ed80))?;
    if ok {
        cx.write(|m| m.set_u8(0x0046_70bc, 1))?;
        call(&cx, GuestCall::new(points::LAYER_SETUP.base).esi(0x0048_5b48)).await?;
        plain(&cx, &points::RASTERIZER_TABLES_RESET).await?;
        plain(&cx, &points::TEXTURE_SWITCH_RESET).await?;
        ok = palette(&cx, PALETTE_KIND, PALETTE_SOURCE, PALETTE).await?;
        cx.write(|m| zero_words(m, 0x0048_4dd0, 0x60))?;
        if ok {
            cx.write(init_states)?;
        }
    }
    plain(&cx, &points::RASTERIZER_ARRAYS_RESET).await?;
    if ok {
        ok = cx.write(|m| frame_buffers(m, block))?
            && al(&plain(&cx, &points::SMALL_TARGETS_CREATE).await?)
            && al(&plain(&cx, &points::TARGETS_CREATE).await?)
            && default_textures(&cx).await?
            && al(&plain(&cx, &points::RASTERIZER_BUFFER_CREATE).await?)
            && {
                cx.write(|m| m.set_u32(0x0046_70f8, 0x24))?;
                al(&plain(&cx, &points::RASTERIZER_POOL_CREATE).await?)
            };
        if ok {
            // `0x12801..0x12820`: the view's three words, and -1.
            cx.write(|m| -> Result<(), MemFault> {
                let view = m.u32(0x0046_8788)?;
                copy_words(m, view, 0x004b_72b4, 3)?;
                m.set_u32(0x004b_72c0, u32::MAX)
            })?;
        }
    }
    plain(&cx, &points::FADE_RECORD_CREATE).await?;
    plain(&cx, &points::GEOMETRY_CACHE_CREATE).await?;
    plain(&cx, &points::TEXTURE_CACHE_CREATE).await?;
    cx.write(|m| zero_words(m, 0x0051_f408, 0x1030))?;
    for i in 0..0x1ff {
        call(&cx, GuestCall::new(points::RASTERIZER_BIT_CLEAR.base).eax(i)).await?;
    }
    // `0x12864..0x12895`: `0x34` bytes of game state, the size's checksum.
    let size = cx.entry().esp.wrapping_sub(4);
    let record = cx.write(|m| -> Result<u32, MemFault> {
        let used = m.u32(0x004e_6084)?;
        let record = used.wrapping_add(m.u32(0x004e_6080)?);
        m.set_u32(0x004e_6084, used + 0x34)?;
        m.set_u32(size, 0x34)?;
        Ok(record)
    })?;
    call(&cx, GuestCall::new(points::GAME_STATE_SIZE_CRC.base).eax(size).edi(4).args([0x004e_608c]).reserve(LOCALS)).await?;
    cx.write(|m| m.set_u32(0x0050_934c, record))?;
    plain(&cx, &points::RASTERIZER_STATE_CREATE).await?;
    if ok {
        cx.write(|m| m.set_u8(0x0048_5a88, 1))?;
    }
    Ok(Ret::new(u32::from(ok), 0))
}

/// `0x12420`.
async fn boot_display(cx: Cx, block: u32) -> EntryResult {
    plain(&cx, &points::VIDEO_MODE_READ).await?;
    let ok = cx.write(|m| boot_device_create(m, block))?;
    if ok {
        cx.write(|m| m.set_u32(GAME_DEVICE, 0))?;
    }
    Ok(Ret::new(u32::from(ok), 0))
}

/// `0x1359d0` with `block` its `ESI`.
async fn cache_block_dispose(cx: &Cx, block: u32) -> Result<(), EntryError> {
    if block == 0 {
        return Ok(());
    }
    call(cx, GuestCall::new(points::CACHE_BLOCK_UNLINK.base).args([block])).await?;
    cx.write(|m| m.set_u32(block + 0x50, 0))?;
    if cx.read(|m| m.u8(block + 0xf))? & 1 == 0 {
        return Ok(());
    }
    let extra = cx.read(|m| m.u32(block + 0x54))?;
    for base in [extra, block] {
        if base == 0 {
            continue;
        }
        if call(cx, GuestCall::new(points::VIRTUAL_FREE.base).args([base, 0, 0x8000])).await?.eax == 0 {
            plain(cx, &points::FREE_FAILED).await?;
        }
    }
    Ok(())
}

/// `0x141c0`.
async fn dispose(cx: Cx) -> EntryResult {
    cx.write(|m| -> Result<(), MemFault> {
        let record = m.u32(FADE_RECORD)?;
        if record != 0 {
            zero_words(m, record, FADE_RECORD_WORDS)?;
            if m.u32(FADE_ALLOCATED)? != 0 && m.u8(FADE_REFUSED)? == 0 {
                m.set_u8(FADE_REFUSED, 1)?;
            }
        }
        frame_buffers_dispose(m)?;
        clear(m, &SMALL_TARGETS)?;
        clear(m, &DEFAULT_TEXTURES)
    })?;
    plain(&cx, &points::RASTERIZER_BUFFER_DISPOSE).await?;
    if cx.read(|m| m.u8(0x004b_62a0))? != 0 {
        plain(&cx, &points::RASTERIZER_POOL_DISPOSE).await?;
        let block = cx.read(|m| m.u32(0x004b_62ac))?;
        cache_block_dispose(&cx, block).await?;
        cx.write(|m| m.set_u8(0x004b_62a0, 0))?;
    }
    plain(&cx, &points::TEXTURE_CACHE_DISPOSE).await?;
    plain(&cx, &points::GEOMETRY_CACHE_DISPOSE).await?;
    cx.write(|m| -> Result<(), MemFault> {
        m.set_u32(GAME_DEVICE, 0)?;
        m.set_u32(BOOT_DEVICE, 0)
    })?;
    Ok(Ret::new(0, 0))
}

/// A body over memory alone, `f`'s `Some` its `EAX`.
type Body = fn(&mut dyn GuestMemory, u32, &runtime::CpuAbi) -> Result<Option<u32>, MemFault>;

fn sync(b: &mut Builder, block: u32, point: &'static Point, label: &'static str, f: Body, pop: u32) {
    b.raw()
        .implement(point, label, move |cpu, ctx, _| {
            let rt = std::sync::Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            let m: &mut dyn GuestMemory = &mut *mem;
            match f(m, block, cpu) {
                Ok(eax) => {
                    if let Some(eax) = eax {
                        cpu.eax = eax;
                    }
                    mem.ret(cpu, pop)
                }
                Err(f) => Err(runtime::GuestFault {
                    addr: f.addr,
                    len: f.len as usize,
                    access: if f.write { runtime::guest::Access::Write } else { runtime::guest::Access::Read },
                    reason: runtime::guest::FaultReason::Unmapped,
                }),
            }
        })
        .replaces_guest();
}

fn boxed(f: impl std::future::Future<Output = EntryResult> + Send + 'static) -> EntryFuture {
    Box::pin(f)
}

/// The feature: every body above as an implementation that never runs
/// its guest body.
pub struct Rasterizer;

impl Feature for Rasterizer {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The rasterizer's start-up and shutdown, with no device").always_on().doc("docs/graphics/d3d-entries.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // The block is guest RAM, saved with it.
        b.snapshot(SnapshotPolicy::Stateless);
        let block = b.guest_alloc(BLOCK_LEN, Permissions::RW)?;
        b.raw().implement_async(&points::RASTERIZER_INITIALIZE, "rasterizer", move |cx| boxed(initialize(cx, block))).replaces_guest();
        b.raw().implement_async(&points::BOOT_DISPLAY_ONCE, "boot display", move |cx| boxed(boot_display(cx, block))).replaces_guest();
        b.raw().implement_async(&points::RASTERIZER_DISPOSE, "shutdown", |cx| boxed(dispose(cx))).replaces_guest();
        b.raw()
            .implement_async(&points::PALETTE_CREATE, "palette", |cx| {
                boxed(async move {
                    let (source, out) = (cx.arg(0)?, cx.arg(1)?);
                    let ok = palette(&cx, cx.entry().eax, source, out).await?;
                    Ok(Ret::new(u32::from(ok), 8))
                })
            })
            .replaces_guest();
        b.raw()
            .implement_async(&points::DEFAULT_TEXTURES_CREATE, "default textures", |cx| {
                boxed(async move { Ok(Ret::new(u32::from(default_textures(&cx).await?), 0)) })
            })
            .replaces_guest();
        b.raw()
            .implement_async(&points::CACHE_BLOCK_DISPOSE, "cache block", |cx| {
                boxed(async move {
                    cache_block_dispose(&cx, cx.entry().esi).await?;
                    Ok(Ret::new(cx.entry().eax, 0))
                })
            })
            .replaces_guest();
        sync(b, block, &points::BOOT_DEVICE_CREATE, "boot device", |m, block, _| Ok(Some(u32::from(boot_device_create(m, block)?))), 0);
        sync(b, block, &points::GAME_DEVICE_CREATE, "game device", |m, block, cpu| Ok(Some(u32::from(game_device_create(m, block, cpu.eax)?))), 0);
        sync(b, block, &points::FRAME_BUFFERS_CREATE, "frame buffers", |m, block, _| Ok(Some(u32::from(frame_buffers(m, block)?))), 0);
        sync(b, block, &points::FRAME_BUFFERS_DISPOSE, "frame buffers", |m, _, _| frame_buffers_dispose(m).map(|()| None), 0);
        sync(b, block, &points::SMALL_TARGETS_DISPOSE, "small targets", |m, _, _| clear(m, &SMALL_TARGETS).map(|()| None), 0);
        sync(b, block, &points::DEFAULT_TEXTURES_DISPOSE, "default textures", |m, _, _| clear(m, &DEFAULT_TEXTURES).map(|()| None), 0);
        sync(
            b,
            block,
            &points::STATE_BLOCK_APPLY,
            "state block",
            |m, _, cpu| {
                let (block, entries) = (m.u32(cpu.esp + 4)?, m.u32(cpu.esp + 8)?);
                state_block(m, block, entries).map(|()| None)
            },
            8,
        );
        for p in [
            &points::DISPLAY_DEFAULTS,
            &points::DISPLAY_WINDOW,
            &points::LAYER_SETUP,
            &points::RASTERIZER_TABLES_RESET,
            &points::TEXTURE_SWITCH_RESET,
            &points::RASTERIZER_ARRAYS_RESET,
            &points::SMALL_TARGETS_CREATE,
            &points::TARGETS_CREATE,
            &points::RASTERIZER_BUFFER_CREATE,
            &points::RASTERIZER_POOL_CREATE,
            &points::FADE_RECORD_CREATE,
            &points::GEOMETRY_CACHE_CREATE,
            &points::TEXTURE_CACHE_CREATE,
            &points::RASTERIZER_BIT_CLEAR,
            &points::GAME_STATE_SIZE_CRC,
            &points::RASTERIZER_STATE_CREATE,
            &points::PALETTE_ALLOCATE,
            &points::TEXTURE_ALLOCATE,
            &points::VOLUME_ALLOCATE,
            &points::CUBE_ALLOCATE,
            &points::VIDEO_MODE_READ,
            &points::RASTERIZER_BUFFER_DISPOSE,
            &points::RASTERIZER_POOL_DISPOSE,
            &points::CACHE_BLOCK_UNLINK,
            &points::TEXTURE_CACHE_DISPOSE,
            &points::GEOMETRY_CACHE_DISPOSE,
            &points::VIRTUAL_FREE,
            &points::FREE_FAILED,
        ] {
            b.calls_guest(p);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
