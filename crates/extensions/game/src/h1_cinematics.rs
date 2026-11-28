//! Halo 1 cinematic controls on Halo 2's render path.
use engine::{
    datum::{DatumArray, layout},
    hs_pool,
    memory::{GuestMemory, MemFault},
    points,
};
use h1_contract::cinematics::{Field, MARKER};
#[cfg(feature = "gpu")]
use std::sync::{Arc, Mutex};

#[cfg(test)]
mod tests;

const OBJECT_RENDER_STATES: u32 = 0x0050_9434;

/// Re-arm the illumination cache just like a fresh render record. Owners,
/// directional light links and live-light registrations survive the reset.
pub fn reset_light_caches(m: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let addr = m.u32(OBJECT_RENDER_STATES)?;
    if addr == 0 || m.u32(addr + layout::ELEMENT_SIZE)? != 0x100 {
        return Ok(());
    }
    let records = DatumArray::at(addr);
    let mut next = 0;
    while let Some(i) = records.next_index(m, next)? {
        let p = records.element_addr(m, i)?;
        m.set_u8(p + 2, 0)?;
        m.set_i32(p + 8, -1)?;
        next = i + 1;
    }
    Ok(())
}

struct Cells(u32);
impl Cells {
    fn find(m: &dyn GuestMemory) -> Option<Self> {
        let sc = m.u32(points::SCENARIO.base).ok()?;
        if sc == 0 {
            return None;
        }
        let count = m.u32(sc + 0x1c0).ok()?;
        if !(1..=hs::MAX_DEFINITIONS as u32).contains(&count) {
            return None;
        }
        let defs = m.u32(sc + 0x1c4).ok()?;
        let mut label = [0; 32];
        // Up to four AI markers follow ours: hooks, timers, teleport and perception.
        let index = (count.saturating_sub(5)..count).rev().find(|&i| {
            m.read(defs + i * 40, &mut label).is_ok()
                && label[..MARKER.len()] == *MARKER.as_bytes()
                && label[MARKER.len()] == 0
        })?;
        let pool = m.u32(hs_pool::GLOBALS).ok()?;
        if pool == 0 || m.u32(pool + layout::HIGH_WATER).ok()? < hs_pool::EXTERNALS + count {
            return None;
        }
        let data = m.u32(pool + layout::DATA).ok()? + hs_pool::EXTERNALS * 8 + 4;
        let base = m.u32(data + index * 8).ok()?;
        if base.checked_add(Field::ALL.len() as u32)? > index
            || m.read(defs + base * 40, &mut label).is_err()
            || label.split(|b| *b == 0).next()? != h1_contract::cinematics::ACTIVE_NAME.as_bytes()
        {
            return None;
        }
        Some(Self(data + base * 8))
    }

    fn word(&self, m: &dyn GuestMemory, f: Field) -> Result<u32, MemFault> {
        m.u32(self.0 + (f as u32) * 8)
    }

    fn reflection(&self, m: &dyn GuestMemory) -> Option<[f32; 4]> {
        use Field::*;
        let mut rgba = [0.; 4];
        for (v, f) in rgba
            .iter_mut()
            .zip([Reflect0, Reflect1, Reflect2, Reflect3])
        {
            *v = f32::from_bits(self.word(m, f).ok()?);
        }
        rgba.iter().any(|v| *v > 0.).then_some(rgba)
    }
}

pub fn reflection_active(m: &dyn GuestMemory) -> bool {
    Cells::find(m).and_then(|c| c.reflection(m)).is_some()
}

fn material(m: &mut dyn GuestMemory, shader: u32) -> Result<(), MemFault> {
    use h1_contract::shader::{MODEL_REFLECTION_SIGNATURE, model_reflection_start};
    if shader == u32::MAX {
        return Ok(());
    }
    let table = m.u32(engine::screen_effect::TAG_TABLE)?;
    let entry = table + (shader & 0xffff) * 16;
    if m.u32(entry)? != u32::from_be_bytes(*b"shad") || m.u32(entry + 4)? != shader {
        return Ok(());
    }
    let shad = m.u32(entry + 8)?;
    if m.u32(shad + 0x20)? == 0 {
        return Ok(());
    }
    let pp = m.u32(shad + 0x24)?;
    let count = m.u32(pp + 0xc)?;
    let Some(first) = count
        .checked_sub(5)
        .and_then(|n| model_reflection_start(n as usize))
    else {
        return Ok(());
    };
    let colors = m.u32(pp + 0x10)?;
    if m.u32(colors + (count - 1) * 4)? != MODEL_REFLECTION_SIGNATURE {
        return Ok(());
    }
    let saved = colors + (count - 5) * 4;
    let override_color = Cells::find(m).and_then(|c| c.reflection(m));
    let mut words = [0; 4];
    if let Some([r, g, b, a]) = override_color {
        // H1 0x709d3 emits zero slope and this intercept, the same
        // tint and strength at both ends of the facing-angle ramp.
        let channel = |x: f32| (x.clamp(0., 1.) * 255.).round() as u32;
        let rgb = channel(r) << 16 | channel(g) << 8 | channel(b);
        words = [rgb, rgb, channel(a) << 24, channel(a) << 24];
    } else {
        for (i, w) in words.iter_mut().enumerate() {
            *w = m.u32(saved + i as u32 * 4)?;
        }
    }
    let vectors = m.u32(pp + 0x18)?;
    for (i, word) in words.into_iter().enumerate() {
        let at = colors + (first + i) as u32 * 4;
        if m.u32(at)? != word {
            m.set_u32(at, word)?;
        }
        if first == 4 {
            let unpack = |shift: u32| ((word >> shift) & 255) as f32 / 255.;
            let mut vector = if i < 2 {
                [unpack(16), unpack(8), unpack(0), 0.]
            } else {
                [0., 0., 0., unpack(24)]
            };
            if let Some([r, g, b, a]) = override_color {
                vector = if i < 2 {
                    [r, g, b, 0.]
                } else {
                    [0., 0., 0., a]
                };
            }
            for (j, v) in vector.into_iter().enumerate() {
                let p = vectors + ((6 + i) * 16 + j * 4) as u32;
                if m.u32(p)? != v.to_bits() {
                    m.set_f32(p, v)?;
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    passes: u16,
    kernel: i16,
    radius: f32,
    light: f32,
    desat: f32,
    tint: [f32; 3],
    filtered: bool,
    video: bool,
    gain: f32,
    noise: f32,
    tick: u32,
}

fn fraction(tick: u32, start: u32, seconds: f32) -> f32 {
    if seconds <= 0. {
        return 1.;
    }
    ((tick.wrapping_sub(start) as i32 as f32) / (30. * seconds)).clamp(0., 1.)
}

impl Frame {
    fn read(m: &dyn GuestMemory) -> Option<Self> {
        use Field::*;
        let cell = Cells::find(m)?;
        let word = |f| cell.word(m, f).ok();
        let real = |f| word(f).map(f32::from_bits);
        let boolean = |f| word(f).map(|w| w as u8 != 0);
        if !boolean(Active)? {
            return None;
        }
        let clock = m.u32(points::GAME_TIME.base).ok()?;
        let tick = m.u32(clock + 8).ok()?;
        let blur = fraction(tick, word(BlurTick)?, real(BlurSeconds)?);
        let filter = fraction(tick, word(FilterTick)?, real(FilterSeconds)?);
        let lerp = |from, to, t| Some(real(from)? * (1. - t) + real(to)? * t);
        let mut tint = [real(Red)?, real(Green)?, real(Blue)?];
        // H1 0x61ba7 compares the three words against the black constant.
        // Its replacement at *0x232f7c is green (0, 1, 0).
        if tint.map(f32::to_bits) == [0; 3] {
            tint = [0., 1., 0.];
        }
        let f = Self {
            passes: word(Passes)? as u16,
            kernel: word(Kernel)? as i16,
            radius: lerp(RadiusFrom, RadiusTo, blur)?,
            light: lerp(LightFrom, LightTo, filter)?,
            desat: lerp(DesatFrom, DesatTo, filter)?,
            tint,
            filtered: boolean(FilterFlag)?,
            video: boolean(Video)?,
            gain: (1 << (word(VideoMode)? as i16).clamp(0, 2)) as f32,
            noise: real(Noise)?,
            tick,
        };
        (f.video || (f.kernel != 0 && f.radius > 0.) || f.light > 0. || f.desat > 0.).then_some(f)
    }
}

/// One frame's pass for the native post glue to draw.
#[cfg(feature = "gpu")]
pub struct Request {
    pub params: native_render::cinematic_gpu::Params,
    /// The loaded map, for the video effect's bitmaps.
    pub map: Option<String>,
}

#[cfg(feature = "gpu")]
type Draw = dyn Fn(&mut dyn GuestMemory, Request) + Send + Sync;
#[cfg(feature = "gpu")]
static DRAW: Mutex<Option<Arc<Draw>>> = Mutex::new(None);

/// Installs what draws the pass (`native_cinematic_glue`).
#[cfg(feature = "gpu")]
pub fn install_draw(draw: Arc<Draw>) {
    *DRAW.lock().unwrap_or_else(|e| e.into_inner()) = Some(draw);
}

/// H1 0x6bc52 samples the render random stream twice for the noise UVs.
/// Seek directly to this tick's pair so both target copies and restored
/// frames see the same phase without advancing the simulation's random seed.
#[cfg_attr(not(any(test, feature = "gpu")), allow(dead_code))]
pub(crate) fn video_offset(tick: u32) -> [f32; 2] {
    use render_bridge::game_random::{RANDOM_A, RANDOM_C};
    let (mut mult, mut add) = (RANDOM_A, RANDOM_C);
    let mut seed = 0u32;
    let mut steps = tick.wrapping_mul(2);
    while steps != 0 {
        if steps & 1 != 0 {
            seed = seed.wrapping_mul(mult).wrapping_add(add);
        }
        add = add.wrapping_mul(mult.wrapping_add(1));
        mult = mult.wrapping_mul(mult);
        steps >>= 1;
    }
    std::array::from_fn(|_| {
        seed = seed.wrapping_mul(RANDOM_A).wrapping_add(RANDOM_C);
        (seed >> 16) as f32 / 65535.
    })
}

/// The video effect's mask and noise bitmaps from map `name`.
#[cfg(feature = "gpu")]
pub fn video_images(name: &str) -> Option<[native_render::gpu::Image; 2]> {
    use crate::native_world_glue::decode_image;
    let (mut map, mut files) = crate::native_map::open(name).ok()?;
    let [mask, noise] = h1_contract::cinematics::VIDEO_BITMAPS.map(|tag| {
        let id = map
            .tags()
            .iter()
            .find(|t| t.group == map::FourCC(*b"bitm") && t.name.as_deref() == Some(tag))?
            .id;
        decode_image(&mut map, &mut files, id, 0)
    });
    Some([mask?, noise?])
}

#[cfg(feature = "gpu")]
impl Frame {
    fn request(self, mem: &dyn GuestMemory) -> Option<Request> {
        let map = self
            .video
            .then(|| crate::health::loaded_map_name(mem).ok())
            .flatten();
        if self.video && map.is_none() {
            return None;
        }
        Some(Request {
            params: native_render::cinematic_gpu::Params {
                passes: self.passes,
                kernel: self.kernel,
                radius: self.radius,
                light: self.light,
                desat: self.desat,
                tint: self.tint,
                filtered: self.filtered,
                video: self.video,
                gain: self.gain,
                noise: self.noise,
                shift: if self.video { video_offset(self.tick) } else { [0.; 2] },
            },
            map,
        })
    }
}

pub fn hooks(b: &mut hooks::Builder) {
    b.raw()
        .before(
            &points::RENDER_SCREEN_EFFECTS,
            "Draw Halo 1 cinematic screen effect",
            |c| {
                let mut m = c.ctx.memory();
                #[cfg(not(feature = "gpu"))]
                let _ = &mut m;
                #[cfg(feature = "gpu")]
                if let Some(frame) = Frame::read(&*m) {
                    let draw = DRAW.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    if let (Some(draw), Some(request)) = (draw, frame.request(&*m)) {
                        draw(&mut *m, request);
                    }
                }
                Ok(hooks::Flow::Continue)
            },
        )
        .context_only();
    b.raw()
        .before(
            &points::SHADER_DRAW_BEGIN,
            "Bind Halo 1 model reflection",
            |c| {
                let shader = c.cpu.edi;
                let _ = material(&mut *c.ctx.memory(), shader);
                Ok(hooks::Flow::Continue)
            },
        )
        .after_feature("h1-hud-text")
        .context_only();
}
