//! A host light's passes (docs/game/forge-monitor-appearance.md, ML5,
//! "Colours, tint and light"; docs/graphics/e3-lighting.md). A host light
//! has no light datum and no `ligh` tag, so no guest record reads its
//! constants and no guest list holds its geometry. Its constants are the
//! guest's own code run on a light made here ([`constants`]): the volume,
//! the frame block and the definition [`Blocks`] holds at addresses past
//! guest RAM, read through the same `0x0004a1e0` port a light without a
//! record takes (`Call::light` of `u32::MAX`), then the flag code's
//! `pass_flags` with no shadow. The geometry it touches is the host's
//! ([`HostDraw`]): the light passes' owner picks the parts, layers 12 and
//! 13, from the sphere. The frame step calls [`draw`] once the main view's
//! light pass is over, after it has added the same lights to the scene.

use engine::light_constants as lc;
use engine::light_passes as lp;
use guest_layout::memory::{GuestMemory, MemFault};
use frame_scene::{LightVolume, SceneLight};

use super::{LightConstants, Stage, VIEW_TARGET, constants_of};

/// The light frame block's address (`record + 0x84`'s, in the call) and
/// its size: the last field read is the falloff word at `+0x84`.
const FRAME_AT: u32 = 0xffff_f000;
const FRAME_BYTES: usize = 0x88;
/// The `ligh` definition's address and size: its gel shader word at
/// `+0xe0` is the last field read.
const DEFINITION_AT: u32 = 0xffff_f100;
const DEFINITION_BYTES: usize = 0x100;
/// The posed light volume's address and size
/// (`engine::render::light_gather::volume::BYTES`).
const CONTEXT_AT: u32 = 0xffff_f200;
const CONTEXT_BYTES: usize = 0x7c;

/// One host light ready to draw: where it is and the constants its
/// passes read.
#[derive(Clone, Debug, PartialEq)]
pub struct HostLight {
    /// The seat whose body carries it.
    pub seat: u8,
    pub origin: [f32; 3],
    pub radius: f32,
    pub constants: LightConstants,
}

/// What the host is asked to draw: each light's layers 12 and 13, in the
/// order given, in the view whose target is `view_target`.
pub struct HostDraw<'a> {
    pub lights: &'a [HostLight],
    pub view_target: [u32; 8],
}

fn put(bytes: &mut [u8], at: usize, word: u32) {
    bytes[at..at + 4].copy_from_slice(&word.to_le_bytes());
}

fn put_f32(bytes: &mut [u8], at: usize, value: f32) {
    put(bytes, at, value.to_bits());
}

fn put_vec3(bytes: &mut [u8], at: usize, v: [f32; 3]) {
    for (i, c) in v.into_iter().enumerate() {
        put_f32(bytes, at + 4 * i, c);
    }
}

/// The guest's memory with a host light's three blocks over it.
struct Blocks<'a> {
    frame: [u8; FRAME_BYTES],
    definition: [u8; DEFINITION_BYTES],
    context: [u8; CONTEXT_BYTES],
    guest: &'a dyn GuestMemory,
}

impl<'a> Blocks<'a> {
    /// A sphere light: `origin`, `radius`, the colour `colour` (linear RGB
    /// times the intensity), the frame `forward` and `up`. The volume is
    /// kind 0 posed as `0x000c1a80` poses a point light of scale one: near
    /// zero, far the radius. The definition is a point light with no gel,
    /// no flags and no colour B (so no specular layer).
    fn sphere(
        guest: &'a dyn GuestMemory,
        origin: [f32; 3],
        radius: f32,
        colour: [f32; 3],
        forward: [f32; 3],
        up: [f32; 3],
    ) -> Self {
        let mut frame = [0u8; FRAME_BYTES];
        put_vec3(&mut frame, 0x28, forward);
        put_vec3(&mut frame, 0x34, up);
        put_vec3(&mut frame, 0x50, colour);
        put_f32(&mut frame, 0x68, 1.0);
        let mut definition = [0u8; DEFINITION_BYTES];
        put(&mut definition, lp::DEFINITION_GEL as usize, u32::MAX);
        put(
            &mut definition,
            lp::DEFINITION_GEL_SHADER as usize,
            u32::MAX,
        );
        let mut context = [0u8; CONTEXT_BYTES];
        put(&mut context, 0x18, 1);
        let [fx, fy, fz] = forward;
        let [ux, uy, uz] = up;
        put_vec3(
            &mut context,
            0x1c,
            [uz * fy - fz * uy, fz * ux - uz * fx, uy * fx - fy * ux],
        );
        put_vec3(&mut context, 0x2c, origin);
        put_f32(&mut context, 0x08, radius);
        put_f32(&mut context, 0x40, radius);
        put_f32(&mut context, 0x44, radius);
        Self {
            frame,
            definition,
            context,
            guest,
        }
    }

    fn blocks(&self) -> [(u32, &[u8]); 3] {
        [
            (FRAME_AT, &self.frame),
            (DEFINITION_AT, &self.definition),
            (CONTEXT_AT, &self.context),
        ]
    }
}

impl GuestMemory for Blocks<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        let fault = MemFault {
            addr,
            len: buf.len() as u32,
            write: false,
        };
        let end = u64::from(addr) + buf.len() as u64;
        for (base, bytes) in self.blocks() {
            let (base, last) = (u64::from(base), u64::from(base) + bytes.len() as u64);
            if u64::from(addr) >= base && end <= last {
                let at = (u64::from(addr) - base) as usize;
                buf.copy_from_slice(&bytes[at..at + buf.len()]);
                return Ok(());
            }
            if u64::from(addr) < last && end > base {
                return Err(fault);
            }
        }
        self.guest.read(addr, buf)
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        Err(MemFault {
            addr,
            len: data.len() as u32,
            write: true,
        })
    }
}

/// `light`'s constants as its layers 12 and 13 read them, with the
/// guest's globals (the view, the constants in the image) from `mem`.
/// `None` for a light that is not a host light with a sphere volume, or
/// when a read faults or the arithmetic raises an unmasked exception.
pub fn constants(mem: &dyn GuestMemory, light: &SceneLight, fpu: lc::Fpu) -> Option<HostLight> {
    let seat = light.host?;
    let c = light.candidate.as_ref()?;
    let Some(&LightVolume::Sphere { origin, radius }) = c.volume.as_ref() else {
        return None;
    };
    if !radius.is_finite() || radius <= 0.0 {
        return None;
    }
    let colour = c.colour_a.map(|v| v * c.intensity);
    let blocks = Blocks::sphere(mem, origin, radius, colour, c.forward, c.up);
    let call = lc::Call {
        frame: FRAME_AT,
        context: CONTEXT_AT,
        light: u32::MAX,
        definition: DEFINITION_AT,
        shadow: 0,
        weight: 1.0f32.to_bits(),
    };
    let globals = lc::light_globals(&blocks, &call, fpu, &mut |_| None).ok()?;
    let flags = lp::pass_flags(&blocks, &globals, 0, fpu.mxcsr).ok()?;
    let constants = constants_of(&blocks, &globals, flags.flags, DEFINITION_AT)?;
    Some(HostLight {
        seat,
        origin,
        radius,
        constants,
    })
}

/// The host's lights, drawn for the view whose light pass is over: each
/// light's constants are made ([`constants`]), then the host is asked for
/// the lot. Returns the lights it drew. The view's target is the one the
/// light passes saw ([`super::new_view`]).
pub fn draw(mem: &mut dyn GuestMemory, lights: &[SceneLight], fpu: lc::Fpu) -> usize {
    use std::sync::atomic::Ordering;
    if lights.is_empty() || Stage::Lights.standing_down() {
        return 0;
    }
    let Some(view_target) = VIEW_TARGET.get() else {
        super::refused("light view target unavailable");
        return 0;
    };
    let Some(host) = super::HOST
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    else {
        return 0;
    };
    let made: Vec<HostLight> = lights
        .iter()
        .filter_map(|l| constants(&*mem, l, fpu))
        .collect();
    HOST_UNREAD.fetch_add((lights.len() - made.len()) as u64, Ordering::Relaxed);
    if made.is_empty() {
        return 0;
    }
    let drawn = host.draw_host(
        mem,
        HostDraw {
            lights: &made,
            view_target,
        },
    );
    HOST_DRAWN.fetch_add(drawn as u64, Ordering::Relaxed);
    HOST_KEPT.fetch_add(
        (made.len() - drawn.min(made.len())) as u64,
        Ordering::Relaxed,
    );
    drawn
}

static HOST_DRAWN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static HOST_KEPT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static HOST_UNREAD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Host lights the host drew (`scene_host_lights_drawn`).
pub fn drawn() -> u64 {
    HOST_DRAWN.load(std::sync::atomic::Ordering::Relaxed)
}

/// Host lights whose constants were made but that the host did not draw
/// (`scene_host_lights_kept`).
pub fn kept() -> u64 {
    HOST_KEPT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Host lights whose constants could not be made (`scene_host_lights_unread`).
pub fn unread() -> u64 {
    HOST_UNREAD.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;
    use frame_scene::Light;

    fn fpu() -> lc::Fpu {
        lc::Fpu {
            mxcsr: 0x1f80,
            x87_control: 0x037f,
        }
    }

    fn blocks(mem: &FlatMemory) -> Blocks<'_> {
        Blocks::sphere(
            mem,
            [1.0, 2.0, 3.0],
            4.0,
            [0.5, 0.25, 0.125],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
        )
    }

    #[test]
    fn the_blocks_read_as_a_sphere_light_of_the_radius() {
        let mem = FlatMemory::new(0x1000, 0x100);
        let b = blocks(&mem);
        let f = |at| f32::from_bits(b.u32(at).unwrap());
        assert_eq!(b.u32(CONTEXT_AT).unwrap(), 0, "kind: a sphere");
        assert_eq!(b.u16(CONTEXT_AT).unwrap(), 0);
        assert_eq!(
            [
                f(CONTEXT_AT + 0x2c),
                f(CONTEXT_AT + 0x30),
                f(CONTEXT_AT + 0x34)
            ],
            [1.0, 2.0, 3.0]
        );
        assert_eq!(f(CONTEXT_AT + 0x44), 4.0, "the volume's extent: the radius");
        assert_eq!(
            f(CONTEXT_AT + 0x20),
            -1.0,
            "the volume's axis: up crossed with forward"
        );
        assert_eq!(
            [f(FRAME_AT + 0x50), f(FRAME_AT + 0x54), f(FRAME_AT + 0x58)],
            [0.5, 0.25, 0.125]
        );
        assert_eq!(b.u32(FRAME_AT + 0x5c).unwrap(), 0, "no colour B");
        assert_eq!(f(FRAME_AT + 0x68), 1.0, "scale one");
        assert_eq!(b.u32(DEFINITION_AT + lp::DEFINITION_GEL).unwrap(), u32::MAX);
        assert_eq!(
            b.u32(DEFINITION_AT + lp::DEFINITION_GEL_SHADER).unwrap(),
            u32::MAX
        );
        assert_eq!(b.u32(DEFINITION_AT).unwrap(), 0, "no definition flags");
    }

    #[test]
    fn other_addresses_read_the_guest_and_a_block_edge_is_a_fault() {
        let mut mem = FlatMemory::new(0x1000, 0x100);
        mem.write(0x1010, &[7, 0, 0, 0]).unwrap();
        let b = blocks(&mem);
        assert_eq!(b.u32(0x1010).unwrap(), 7);
        assert!(b.u32(0x2000).is_err(), "outside the guest");
        assert!(
            b.u32(FRAME_AT + FRAME_BYTES as u32 - 2).is_err(),
            "straddles the frame's end"
        );
        let mut b = blocks(&mem);
        assert!(b.write(FRAME_AT, &[0]).is_err(), "the overlay is read only");
    }

    #[test]
    fn a_guest_light_or_a_projected_volume_makes_no_host_light() {
        let mem = FlatMemory::new(0x1000, 0x100);
        let mut guest = Light::host_light(1, [0.0; 3], [1.0; 3], 2.0);
        guest.host = None;
        assert!(constants(&mem, &guest, fpu()).is_none(), "no seat");
        let mut projected = Light::host_light(1, [0.0; 3], [1.0; 3], 2.0);
        projected.candidate.as_mut().unwrap().volume = Some(LightVolume::Projected {
            kind: 1,
            origin: [0.0; 3],
            axis: [1.0, 0.0, 0.0],
            near: 0.1,
            far: 2.0,
            far_extent: [1.0, 1.0],
        });
        assert!(
            constants(&mem, &projected, fpu()).is_none(),
            "a sphere only"
        );
        let flat = Light::host_light(1, [0.0; 3], [1.0; 3], 0.0);
        assert!(constants(&mem, &flat, fpu()).is_none(), "a radius to light");
    }

    #[test]
    fn a_guest_without_the_constants_in_its_image_gives_none() {
        // The port reads the image's constants and the view: a memory
        // without them faults, and the light is left undrawn.
        let mem = FlatMemory::new(0x1000, 0x100);
        let light = Light::host_light(1, [0.0; 3], [1.0; 3], 2.0);
        assert!(constants(&mem, &light, fpu()).is_none());
    }
}
