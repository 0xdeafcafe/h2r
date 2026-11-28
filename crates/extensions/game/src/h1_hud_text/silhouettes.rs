//! Supply map-owned glyphs to the native measuring and atlas upload paths.

use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points::{
    HUD_GLYPH_CREATE, HUD_GLYPH_FIND, HUD_GLYPH_LOAD, HUD_GLYPH_PIXELS, HUD_GLYPH_PIXELS_READY,
};
use hooks::{Builder, Flow};
use h1_contract::hud::{
    MARKER, MARKER_AT, SILHOUETTE_BASE, SILHOUETTE_LIMIT, SILHOUETTE_SIZE, SILHOUETTES,
};

use super::mission_hud;

const GLYPH_RECORDS: u32 = 0x0054_d574;

struct Sprite {
    key: u32,
    metrics: [u8; 16],
    pixels: u32,
    len: usize,
}

fn sprite(mem: &dyn GuestMemory, tag: u32, index: u32) -> Result<Option<Sprite>, MemFault> {
    if index >= mem.u32(tag + SILHOUETTES)? {
        return Ok(None);
    }
    let rows = mem.u32(tag + SILHOUETTES + 4)?;
    if rows == 0 {
        return Ok(None);
    }
    let row = rows + index * SILHOUETTE_SIZE;
    let width = mem.u16(row + 4)?;
    let height = mem.u16(row + 6)?;
    let len = 2 * usize::from(width) * usize::from(height);
    let pixels = mem.u32(row + 0x14)?;
    let key = mem.u32(row)?;
    if width > 128
        || height > 128
        || !(2..=8192).contains(&len)
        || len as u32 != mem.u32(row + 0x10)?
        || pixels == 0
        || key >> 30 != 2
    {
        return Ok(None);
    }
    let mut metrics = [0; 16];
    metrics[..2].copy_from_slice(&mem.u16(row + 8)?.to_le_bytes());
    metrics[2..4].copy_from_slice(&(len as u16).to_le_bytes());
    metrics[4..8].copy_from_slice(&[width, height].map(u16::to_le_bytes).concat());
    mem.read(row + 0xa, &mut metrics[8..12])?;
    Ok(Some(Sprite {
        key,
        metrics,
        pixels,
        len,
    }))
}

fn glyph(mem: &dyn GuestMemory, unit: u32) -> Result<Option<Sprite>, MemFault> {
    let Some(index) = unit
        .checked_sub(u32::from(SILHOUETTE_BASE))
        .filter(|&index| index < u32::from(SILHOUETTE_LIMIT))
    else {
        return Ok(None);
    };
    let Some(tag) = mission_hud(mem)? else {
        return Ok(None);
    };
    if mem.u32(tag + MARKER_AT)? != MARKER {
        return Ok(None);
    }
    sprite(mem, tag, index)
}

const UNITS: std::ops::Range<u32> =
    SILHOUETTE_BASE as u32..(SILHOUETTE_BASE as u32 + SILHOUETTE_LIMIT as u32);

fn cached(mem: &dyn GuestMemory, datum: u32) -> Result<Option<(u32, Sprite)>, MemFault> {
    let index = datum & 0xffff;
    let array = mem.u32(GLYPH_RECORDS)?;
    if datum == u32::MAX || array == 0 || index >= mem.u32(array + 0x20)? {
        return Ok(None);
    }
    let row = mem.u32(array + 0x44)? + 0x38 * index;
    let key = mem.u32(row + 4)?;
    if key >> 30 != 2 || u32::from(mem.u16(row)?) != datum >> 16 {
        return Ok(None);
    }
    Ok(glyph(mem, mem.u32(row + 8)?)?
        .filter(|sprite| sprite.key == key)
        .map(|sprite| (row, sprite)))
}

pub(super) fn register(b: &mut Builder) {
    b.raw()
        .before(&HUD_GLYPH_FIND, "look up the map glyph cache key", |c| {
            if UNITS.contains(&c.cpu.ecx)
                && let Some(sprite) = glyph(&*c.ctx.memory(), c.cpu.ecx).ok().flatten()
            {
                c.cpu.eax = sprite.key;
            }
            Ok(Flow::Continue)
        })
        .context_only();
    b.raw()
        .before(&HUD_GLYPH_CREATE, "allocate a map glyph cache entry", |c| {
            let sp = c.cpu.esp;
            let mut mem = c.ctx.memory();
            if let Some(sprite) = mem
                .u32(sp + 8)
                .ok()
                .and_then(|unit| glyph(&*mem, unit).ok().flatten())
            {
                let _ = mem.set_u32(sp + 4, sprite.key);
            }
            Ok(Flow::Continue)
        })
        .context_only();
    b.raw()
        .before(&HUD_GLYPH_LOAD, "load the map glyph metrics", |c| {
            let loaded = {
                let mut mem = c.ctx.memory();
                cached(&*mem, c.cpu.eax)
                    .ok()
                    .flatten()
                    .is_some_and(|(row, sprite)| {
                        mem.write(row + 0x1c, sprite.metrics).is_ok()
                            && mem.set_u32(row + 0x10, 4).is_ok()
                    })
            };
            if loaded {
                c.cpu.eax = 1;
                return c.ret(0).map(Flow::Return);
            }
            Ok(Flow::Continue)
        })
        .context_only();
    b.raw()
        .before(
            &HUD_GLYPH_PIXELS_READY,
            "use the resident map glyph pixels",
            |c| {
                if cached(&*c.ctx.memory(), c.cpu.eax).ok().flatten().is_some() {
                    c.cpu.eax = 1;
                    return c.ret(4).map(Flow::Return);
                }
                Ok(Flow::Continue)
            },
        )
        .context_only();
    b.raw()
        .before(
            &HUD_GLYPH_PIXELS,
            "upload the map glyph into the font atlas",
            |c| {
                let uploaded = {
                    let mut mem = c.ctx.memory();
                    let sp = c.cpu.esp;
                    cached(&*mem, c.cpu.eax)
                        .ok()
                        .flatten()
                        .is_some_and(|(_, sprite)| {
                            let result = mem.u32(sp + 4).and_then(|output| {
                                let mut bytes = vec![0; sprite.len];
                                mem.read_bytes(sprite.pixels, &mut bytes, 1)?;
                                mem.write_bytes(output, &bytes, 1)
                            });
                            result.is_ok()
                        })
                };
                if uploaded {
                    c.cpu.eax = 1;
                    return c.ret(4).map(Flow::Return);
                }
                Ok(Flow::Continue)
            },
        )
        .context_only();
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn map_sprite_supplies_native_glyph_metrics_and_pixels() {
        let mut mem = FlatMemory::new(0x1000, 0x1000);
        let tag = 0x1000;
        let row = 0x1300;
        for (at, value) in [
            (tag + SILHOUETTES, 1),
            (tag + SILHOUETTES + 4, row),
            (row, 0x8123_4567),
            (row + 0x10, 12),
            (row + 0x14, 0x1400),
        ] {
            mem.set_u32(at, value).unwrap();
        }
        for (at, value) in [(4, 3), (6, 2), (8, 4), (0xa, u16::MAX), (0xc, 2)] {
            mem.set_u16(row + at, value).unwrap();
        }
        let glyph = sprite(&mem, tag, 0).unwrap().unwrap();
        assert_eq!(glyph.key, 0x8123_4567);
        assert_eq!(
            glyph.metrics,
            [4, 0, 12, 0, 3, 0, 2, 0, 255, 255, 2, 0, 0, 0, 0, 0]
        );
        assert_eq!((glyph.pixels, glyph.len), (0x1400, 12));
        assert!(sprite(&mem, tag, 1).unwrap().is_none());
        mem.set_u32(row + 0x10, 13).unwrap();
        assert!(sprite(&mem, tag, 0).unwrap().is_none());
    }
}
