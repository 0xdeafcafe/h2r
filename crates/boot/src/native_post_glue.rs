//! The `native-post` host (docs/graphics/native-renderer.md, stage 5):
//! the colour pass of `render_screen_effects` (`0x000214f0`) as the game
//! runs it, with its quads dropped on the device and the native glow
//! drawn in place of the last.
//!
//! At the call's entry ([`feature::native_post`]) the host plans the glow
//! from its seven words ([`plan`]: `engine::colour_pass::parameters`, the
//! three combiner blocks and the blur's steps from `engine::glow`, each
//! bit-equal to the lifted original, and `native_render::glow`'s frame
//! words), then arms `d3d::Device::swallow` with one entry per quad
//! ([`targets`]): a four-vertex fan into the view's target (slot 0, bound
//! at the call), target 7 (slot 1) or target 8 (slot 2). The device drops
//! exactly those and issues the glow ([`native_render::glow::GpuGlow`])
//! after the last. A quad the plan does not have ends the swallow: it and
//! the rest are drawn by the D3D path and the glow is not; the host then
//! logs it and keeps every later call on the D3D path. Anything it cannot
//! plan (a copy after the blur, a target it does not know) is not armed.
//!
//! The frame A/B, behind `H2_POST_AB=<dir>` ([`Ab`]): the view's colour
//! where the glow lands (after the composite, before the screen effects,
//! the fade and the HUD), from the call numbered `H2_POST_AB_AT` (default
//! 600) for `H2_POST_AB_COUNT` calls (default 1), into
//! `<dir>/<native|d3d>-<call>-<w>x<h>.png` (`.raw` for a format other than
//! 8-bit RGBA or BGRA), one per copy of the target under the render scale.
//! With the switch off the hook is placed for it and the host observes
//! the same quads, dropping none, so both sides are captured at the same
//! draw.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use engine::colour_pass;
use engine::glow::{self as game_glow, BlurArgs};
use engine::memory::GuestMemory;
use feature::native_post::{Fp, Host};
use gfx::vertex::PrimitiveType;
use native_render::camera::GuestRead;
use native_render::glow::{self, Blend, Draw, GlowTarget, GpuGlow, Plan, Surface};

use crate::native_lights_glue::DeviceMemory;

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

fn err<E: std::fmt::Debug>(what: &'static str) -> impl Fn(E) -> String {
    move |e| format!("{what}: {e:?}")
}

/// The glow `0x000214f0` draws for `args`, or `None` when it draws
/// nothing; `Err` when it draws something this cannot plan.
pub fn plan(mem: &dyn GuestMemory, args: [u32; 7], fp: Fp) -> Result<Option<Plan>, String> {
    let p = colour_pass::parameters(mem, args, fp.mxcsr).map_err(err("parameters"))?;
    if !p.draws {
        return Ok(None);
    }
    let mut plan = glow::read_frame(&Ro(mem)).ok_or("the window, signs or target sizes")?;
    let pass_a = game_glow::pass_a(mem, &p, fp.x87_control).map_err(err("pass_a"))?;
    let blur_combiner =
        game_glow::blur_combiner(mem, p.args[4], p.y, fp.mxcsr, fp.x87_control).map_err(err("blur_combiner"))?;
    let blur = game_glow::blur(mem, &BlurArgs::of(&p), fp.mxcsr).map_err(err("blur"))?;
    if blur.copy.is_some() {
        return Err("the blur copies its result (0x000351a0)".into());
    }
    let pass_b = game_glow::pass_b(mem, &p, fp.x87_control).map_err(err("pass_b"))?;
    let view = mem.u32(glow::VIEW_TARGET).map_err(err("the view's target"))? as u16 as i16 as i32 as u32;
    let surface = |t: u32| Surface::of_target(t, view).ok_or(format!("target {t} in the blur"));
    // 0x000214f0: the view at stages 0..3, four taps at 0.5 into 7.
    let mut stages = [Some(Surface::View); 4];
    plan.draws.push(Draw {
        dest: Surface::Down,
        stages,
        combiner: pass_a.0,
        taps: 4,
        spread: 0.5,
        blend: None,
    });
    // 0x000211a0: each step's source at stages 0..3.
    for d in &blur.draws {
        stages = [Some(surface(d.source)?); 4];
        plan.draws.push(Draw {
            dest: surface(d.dest)?,
            stages,
            combiner: blur_combiner.0,
            taps: 4,
            spread: f32::from_bits(d.spread),
            blend: None,
        });
    }
    // The composite: the result at stage 0, one tap, 0x000222d0's blend.
    stages[0] = Some(surface(blur.result)?);
    let source = if p.args[5] & 0xff != 0 { 0x307 } else { 1 };
    let dest = u32::from(p.args[6] & 0xff == 0);
    plan.draws.push(Draw {
        dest: Surface::View,
        stages,
        combiner: pass_b.0,
        taps: 1,
        spread: 0.0,
        blend: Blend::of_states(source, dest),
    });
    Ok(Some(plan))
}

/// The swallow's slot per quad: the view 0, target 7 1, target 8 2.
pub fn targets(plan: &Plan) -> Vec<u8> {
    plan.draws
        .iter()
        .map(|d| match d.dest {
            Surface::View => 0,
            Surface::Down => 1,
            Surface::Other => 2,
        })
        .collect()
}

/// A quad of `0x00034420`: `D3DDevice_Begin(7)`, four vertices.
pub const QUAD: (PrimitiveType, usize) = (PrimitiveType::TriangleFan, 4);

/// `H2_POST_AB`: where and which calls the A/B captures.
pub struct Ab {
    dir: PathBuf,
    at: u64,
    count: u64,
}

/// Calls after the last capture the host still arms for, so the copies
/// already made are mapped and written (each only once the frame that
/// made it was submitted: the next pass's encode, on the GPU's thread).
const AB_TAIL: u64 = 8;

impl Ab {
    fn from_env() -> Option<Ab> {
        let dir = PathBuf::from(std::env::var_os("H2_POST_AB")?);
        let num = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        Some(Ab {
            dir,
            at: num("H2_POST_AB_AT", 600),
            count: num("H2_POST_AB_COUNT", 1).max(1),
        })
    }

    fn get() -> Option<&'static Ab> {
        static AB: OnceLock<Option<Ab>> = OnceLock::new();
        AB.get_or_init(Ab::from_env).as_ref()
    }

    fn captures(&self, call: u64) -> bool {
        (self.at..self.at + self.count).contains(&call)
    }

    fn pumps(&self, call: u64) -> bool {
        (self.at..self.at + self.count + AB_TAIL).contains(&call)
    }
}

/// Whether the A/B is asked for (boot places the hook for it).
pub fn ab_requested() -> bool {
    Ab::get().is_some()
}

const PENDING: u8 = 0;
const READY: u8 = 1;
const FAILED: u8 = 2;

/// A copy of the view's colour on its way back.
struct Readback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    mapping: bool,
    path: PathBuf,
    size: (u32, u32),
    padded: u32,
    format: wgpu::TextureFormat,
}

static READBACKS: Mutex<Vec<Readback>> = Mutex::new(Vec::new());

/// Copies the bound colour into a buffer, to be written by a later
/// [`pump`].
fn capture(t: &mut d3d::ExternalTarget<'_>, stem: &str) {
    let Some(ab) = Ab::get() else { return };
    let texture = t.color.texture();
    let Some(bpp) = t.color_format.block_copy_size(None) else { return };
    let (w, h) = (texture.width(), texture.height());
    let padded = (w * bpp).div_ceil(256) * 256;
    let buffer = t.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("native post A/B"),
        size: u64::from(padded) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    t.encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    READBACKS.lock().unwrap_or_else(|e| e.into_inner()).push(Readback {
        buffer,
        state: Arc::new(AtomicU8::new(PENDING)),
        mapping: false,
        path: ab.dir.join(format!("{stem}-{w}x{h}")),
        size: (w, h),
        padded,
        format: t.color_format,
    });
}

/// Maps the copies made before this pass (their frames were submitted:
/// passes encode in order on the GPU's thread) and writes the ones
/// mapped. Never waits.
fn pump() {
    let mut list = READBACKS.lock().unwrap_or_else(|e| e.into_inner());
    list.retain_mut(|r| {
        if !r.mapping {
            r.mapping = true;
            let state = Arc::clone(&r.state);
            r.buffer.slice(..).map_async(wgpu::MapMode::Read, move |res| {
                state.store(if res.is_ok() { READY } else { FAILED }, Ordering::Release);
            });
            return true;
        }
        match r.state.load(Ordering::Acquire) {
            READY => {
                if let Err(e) = write(r) {
                    eprintln!("native-post A/B: {}: {e}", r.path.display());
                }
                r.buffer.unmap();
                false
            }
            FAILED => false,
            _ => true,
        }
    });
}

fn write(r: &Readback) -> Result<(), String> {
    let data = r.buffer.slice(..).get_mapped_range().map_err(|e| format!("{e:?}"))?;
    let (w, h) = r.size;
    let row = (w * r.format.block_copy_size(None).unwrap_or(4)) as usize;
    let mut px = Vec::with_capacity(row * h as usize);
    for line in data.chunks(r.padded as usize).take(h as usize) {
        px.extend_from_slice(&line[..row]);
    }
    drop(data);
    use wgpu::TextureFormat as F;
    let rgba = match r.format {
        F::Rgba8Unorm | F::Rgba8UnormSrgb => true,
        F::Bgra8Unorm | F::Bgra8UnormSrgb => {
            for p in px.chunks_exact_mut(4) {
                p.swap(0, 2);
            }
            true
        }
        _ => false,
    };
    if let Some(dir) = r.path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if !rgba {
        let path = r.path.with_extension(format!("{:?}.raw", r.format));
        return std::fs::write(path, px).map_err(|e| e.to_string());
    }
    let path = r.path.with_extension("png");
    let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut out = enc.write_header().map_err(|e| e.to_string())?;
    out.write_image_data(&px).map_err(|e| e.to_string())?;
    eprintln!("native-post A/B: wrote {}", path.display());
    Ok(())
}

/// The native glow, in place of the pass's last quad.
struct GlowPass {
    plan: Plan,
    gpu: Arc<Mutex<GpuGlow>>,
    /// `Some(stem)`: capture after drawing.
    capture: Option<String>,
}

impl d3d::ExternalPass for GlowPass {
    fn encode(&mut self, mut t: d3d::ExternalTarget<'_>) {
        pump();
        let drawn = self.gpu.lock().unwrap_or_else(|e| e.into_inner()).encode(
            GlowTarget {
                device: t.device,
                queue: t.queue,
                encoder: &mut *t.encoder,
                color: t.color,
                color_format: t.color_format,
                guest_size: t.guest_size,
                size: t.size,
            },
            &self.plan,
        );
        if let Err(r) = drawn {
            GLOW_REFUSED.fetch_add(1, Ordering::Relaxed);
            if !GLOW_REFUSAL_LOGGED.swap(true, Ordering::Relaxed) {
                eprintln!("native-post: the glow was not drawn: {r:?} (logged once)");
            }
        }
        if let Some(stem) = &self.capture {
            capture(&mut t, stem);
        }
    }
}

/// The A/B's D3D side: a capture where the glow would land.
struct CapturePass(Option<String>);

impl d3d::ExternalPass for CapturePass {
    fn encode(&mut self, mut t: d3d::ExternalTarget<'_>) {
        pump();
        if let Some(stem) = &self.0 {
            capture(&mut t, stem);
        }
    }
}

static GLOW_REFUSED: AtomicU64 = AtomicU64::new(0);
static GLOW_REFUSAL_LOGGED: AtomicBool = AtomicBool::new(false);
static PLAN_LOGGED: AtomicBool = AtomicBool::new(false);
/// A swallow ended early: later calls keep the D3D path.
static FELL_BACK: AtomicBool = AtomicBool::new(false);

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    gpu: Arc<Mutex<GpuGlow>>,
    calls: AtomicU64,
    mismatches: AtomicU64,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn colour_pass(&self, mem: &mut dyn GuestMemory, args: [u32; 7], fp: Fp, native: bool) -> bool {
        let call = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        let ab = Ab::get();
        let capturing = ab.is_some_and(|a| a.captures(call));
        let stem = |side: &str| capturing.then(|| format!("{side}-{call:06}"));
        let Ok(mismatches) = self.device.lock().map(|d| d.stats().swallow_mismatches) else { return false };
        if mismatches > self.mismatches.swap(mismatches, Ordering::Relaxed) && !FELL_BACK.swap(true, Ordering::Relaxed) {
            eprintln!(
                "native-post: the colour pass drew a quad its plan did not have; that call's glow fell back to D3D, and every later call keeps it"
            );
        }
        let native = native && !FELL_BACK.load(Ordering::Relaxed);
        if !native && !ab.is_some_and(|a| a.pumps(call)) {
            return false;
        }
        let plan = match plan(&*mem, args, fp) {
            Ok(Some(p)) => p,
            Ok(None) => return false,
            Err(e) => {
                if !PLAN_LOGGED.swap(true, Ordering::Relaxed) {
                    eprintln!("native-post: the colour pass keeps its D3D quads: {e} (logged once)");
                }
                return false;
            }
        };
        let targets = targets(&plan);
        let pass: Box<dyn d3d::ExternalPass> = if native {
            Box::new(GlowPass {
                plan,
                gpu: Arc::clone(&self.gpu),
                capture: stem("native"),
            })
        } else {
            Box::new(CapturePass(stem("d3d")))
        };
        let swallow = d3d::Swallow {
            targets,
            primitive: QUAD.0,
            vertices: QUAD.1,
            drop: native,
            pass,
        };
        let Ok(mut d) = self.device.lock() else { return false };
        d.swallow(&mut DeviceMemory(mem), swallow).unwrap_or(false)
    }
}

/// Installs the host over `device`.
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    feature::native_post::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        gpu: Arc::new(Mutex::new(GpuGlow::new())),
        calls: AtomicU64::new(0),
        mismatches: AtomicU64::new(0),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(dest: Surface) -> Draw {
        Draw {
            dest,
            stages: [None; 4],
            combiner: [0; glow::COMBINER_WORDS],
            taps: 4,
            spread: 0.0,
            blend: None,
        }
    }

    #[test]
    fn the_swallow_has_a_slot_per_quad() {
        let plan = Plan {
            window: [0.0; 4],
            signs: [0.0; 2],
            view: (640, 480),
            small: (160, 120),
            draws: [Surface::Down, Surface::Other, Surface::Down, Surface::Other, Surface::Down, Surface::View]
                .map(draw)
                .to_vec(),
        };
        assert_eq!(targets(&plan), vec![1, 2, 1, 2, 1, 0]);
        assert_eq!(QUAD, (PrimitiveType::from_xbox(7).unwrap(), 4), "0x00034420's Begin(7)");
    }
}
