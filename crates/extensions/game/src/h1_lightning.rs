//! H1 endpoint ribbons on the H2 lightning queue. The callback data starts
//! at queue entry +0x24 (0x429a0) and is passed to 0x508d0. Sorts move
//! indices, leaving the owner's marker transforms in attachment order.
use guest_layout::handle::DatumHandle;
use engine::light_constants::global::SPECIAL_FORWARD;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::tags::GroupTag;
use guest_layout::view::{Tags, objects};
use hooks::{BuildError, Builder, Feature, Flow, Manifest, points};
use tags::effects::h1_lightning::{Endpoint, HEADER, MAGIC, STRIDE};

pub struct H1Lightning;

impl Feature for H1Lightning {
    fn manifest(&self) -> Manifest {
        Manifest::new("h1-lightning", "Halo 1 marker lightning").doc("docs/halo1/objects.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Reads the queue and the tag, writes the ribbon. Its random
        // numbers are seeded from the owner and the marker each call.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        b.raw()
            .before(
                &points::LIGHTNING_QUEUE,
                "carry the endpoint width input",
                |call| {
                    let sp = call.cpu.esp;
                    forward_scale(&mut *call.ctx.memory(), sp)
                        .map_err(super::h1_particle_physics::fault)?;
                    Ok(Flow::Continue)
                },
            )
            .context_only();
        b.raw()
            .before(
                &points::LIGHT_VOLUME_SPRITES,
                "place Halo 1 lightning ribbons",
                |call| {
                    let sp = call.cpu.esp;
                    if prepare(&mut *call.ctx.memory(), sp, call.cpu.edi)
                        .map_err(super::h1_particle_physics::fault)?
                    {
                        Ok(Flow::Continue)
                    } else {
                        call.cpu.eax = 1;
                        call.cpu.esp = sp.wrapping_add(36);
                        Ok(Flow::Return(runtime::TransferAbi::ret(0x5164d)))
                    }
                },
            )
            .after_feature("h1-glow")
            .context_only();
        Ok(())
    }
}

fn profile(m: &dyn GuestMemory, layer: u32) -> Result<Option<u32>, MemFault> {
    let size = m.u32(layer + 0xdc)?;
    if size < (HEADER + 2 * STRIDE) as u32 {
        return Ok(None);
    }
    let p = m.u32(layer + 0xe0)?;
    if p == 0 || m.u32(p + 0xc)? != u32::from_le_bytes(MAGIC) {
        return Ok(None);
    }
    let count = m.u32(p + 0x14)?;
    Ok(((2..=64).contains(&count) && size == HEADER as u32 + count * STRIDE as u32).then_some(p))
}

fn forward_scale(m: &mut dyn GuestMemory, sp: u32) -> Result<(), MemFault> {
    if m.u32(sp)? != 0x41bb8 {
        return Ok(());
    }
    let id = DatumHandle(m.u32(sp + 8)?);
    if let Some(root) = Tags.get(m, id, GroupTag::from_chars(*b"tdtl"))?
        && m.u32(root + 0x68)? != 0
        && let Some(p) = profile(m, m.u32(root + 0x6c)?)?
        && m.u8(p + 0x13)? != 0
    {
        // Original attachment caller's [esp+20] (0x41b78).
        let scale = m.f32(sp + 0x3c)?;
        m.set_f32(sp + 0x18, scale)?;
    }
    Ok(())
}

type V = [f32; 3];
fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn random(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
    (*seed >> 16) as f32 / 32768. - 1.
}
fn position(m: &dyn GuestMemory, job: u32, e: Endpoint, mut seed: u32) -> Result<V, MemFault> {
    let pos = m.vec3(job + 0x10)?;
    let forward = m.vec3(job + 0x28)?;
    let up = m.vec3(job + 0x34)?;
    let left = cross(up, forward);
    let [x, y, z] = e.jitter.map(|j| j * random(&mut seed));
    Ok(std::array::from_fn(|k| {
        pos[k] + x * forward[k] + y * left[k] + z * up[k]
    }))
}

fn ribbon(start: V, end: V, camera: V, a: Endpoint, b: Endpoint, mut seed: u32) -> Vec<V> {
    let last = 1 << a.depth;
    let mut pts = vec![[0.; 3]; last + 1];
    pts[0] = start;
    pts[last] = end;
    let axis = cross(std::array::from_fn(|i| end[i] - start[i]), camera);
    let norm = axis.iter().map(|x| x * x).sum::<f32>().sqrt();
    let axis = if norm > 0.00001 {
        axis.map(|x| x / norm)
    } else {
        [0., 0., 1.]
    };
    let mut step = last;
    let mut gain = 1.;
    while step > 1 {
        let half = step / 2;
        for i in (half..last).step_by(step) {
            let t = i as f32 / last as f32;
            let d =
                (a.displacement + t * (b.displacement - a.displacement)) * gain * random(&mut seed);
            pts[i] =
                std::array::from_fn(|k| (pts[i - half][k] + pts[i + half][k]) * 0.5 + axis[k] * d);
        }
        step = half;
        gain *= 0.5;
    }
    pts
}

fn prepare(m: &mut dyn GuestMemory, sp: u32, layer: u32) -> Result<bool, MemFault> {
    if m.u32(sp)? != 0x5164d {
        return Ok(true);
    }
    let Some(profile) = profile(m, layer)? else {
        return Ok(true);
    };
    // [esp+19b8] before the 8 arguments and return address (0x50a40).
    let job = m.u32(sp + 0x19dc)?;
    let queue = m.u32(0x4b6288)?;
    let slots = m.u32(0x4b6280)?.min(1024);
    let Some(off) = job.checked_sub(queue).and_then(|n| n.checked_sub(0x24)) else {
        return Ok(false);
    };
    let index = off / 0x70;
    if off % 0x70 != 0 || index >= slots {
        return Ok(false);
    }
    let owner = m.u32(job + 4)?;
    let tag = m.u32(job + 8)?;
    let same = |i: u32| -> Result<bool, MemFault> {
        let p = queue + i * 0x70;
        Ok(m.u32(p + 0x20)? == 0x508d0 && m.u32(p + 0x28)? == owner && m.u32(p + 0x2c)? == tag)
    };
    let count = m.u32(profile + 0x14)?;
    let mut first = index;
    while first > 0 && same(first - 1)? {
        first -= 1;
    }
    let i = (index - first) % count;
    if i + 1 >= count || index - i + count > slots {
        return Ok(false);
    }
    for j in index - i..index - i + count {
        if !same(j)? {
            return Ok(false);
        }
    }
    let style = |n| -> Result<Endpoint, MemFault> {
        let mut row = [0; STRIDE];
        m.read(profile + HEADER as u32 + n * STRIDE as u32, &mut row)?;
        Ok(Endpoint::decode(&row))
    };
    let a = style(i)?;
    if a.end || a.depth > 6 {
        return Ok(false);
    }
    let b = style(i + 1)?;
    let clock = m.u32(points::GAME_TIME.base)?;
    let seed = owner.rotate_left(9)
        ^ tag
        ^ m.u32(clock + 8)?.wrapping_mul(1664525)
        ^ (u32::from(m.u8(profile + 0x10)?) << 24);
    let mark_seed = |n: u32| seed ^ n.wrapping_mul(2654435761);
    let start = position(m, job, a, mark_seed(i))?;
    let end = position(m, job + 0x70, b, mark_seed(i + 1))?;
    let pts = ribbon(
        start,
        end,
        m.vec3(SPECIAL_FORWARD)?,
        a,
        b,
        mark_seed(i).rotate_left(11),
    );
    let slot = u32::from(m.u8(profile + 0x11)?);
    let mut tint = [1.; 3];
    if slot != 0
        && let Some((_, body)) = objects::get(m, DatumHandle(owner))?
    {
        let n = u32::from(m.u16(body.addr + 0x124)?) / 24;
        if slot <= n {
            let table = body.addr + u32::from(m.u16(body.addr + 0x126)?);
            tint = m.vec3(table + 12 * (n + slot - 1))?;
        }
    }
    let scale = f32::from(m.u8(job + 1)?) / 255.;
    let dst = m.u32(sp + 4)?;
    let last = pts.len() - 1;
    for (n, pos) in pts.into_iter().enumerate() {
        let t = n as f32 / last as f32;
        let at = dst + n as u32 * 0x18;
        for (k, v) in pos.into_iter().enumerate() {
            m.set_f32(at + 4 * k as u32, v)?;
        }
        let width = (a.width + (b.width - a.width) * t) * scale;
        m.set_f32(at + 0xc, width)?;
        m.set_f32(at + 0x10, width)?;
        let mut argb = 0;
        for (j, c) in [1., tint[0], tint[1], tint[2]].into_iter().enumerate() {
            let v = (a.argb[j] + (b.argb[j] - a.argb[j]) * t) * c;
            argb = (argb << 8) | (255. * v.clamp(0., 1.)).round() as u32;
        }
        m.set_u32(at + 0x14, argb)?;
    }
    m.set_u32(sp + 8, (last + 1) as u32)?;
    let blend = u32::from(m.u8(profile + 0x12)?);
    m.set_u32(sp + 0x20, 0x484c0000 | (blend << 8))?;
    // Repeat the native setup when this callback submits another layer.
    m.set_u8(0x46713c, 1)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recursive_displacement_preserves_endpoints_and_stays_in_the_view_plane() {
        let marker = Endpoint {
            end: false,
            depth: 3,
            jitter: [0.; 3],
            displacement: 1.,
            width: 0.5,
            argb: [1.; 4],
        };
        let pts = ribbon([0.; 3], [2., 0., 0.], [0., 0., 1.], marker, marker, 123);
        assert_eq!(pts.len(), 9);
        assert_eq!(pts[0], [0.; 3]);
        assert_eq!(pts[8], [2., 0., 0.]);
        assert!(pts[4][1].abs() > 0.01);
        for (i, p) in pts.iter().enumerate() {
            assert_eq!(p[0], i as f32 / 4.);
            assert_eq!(p[2], 0.);
        }
    }
    #[test]
    fn queued_markers_emit_one_tapered_segment_and_stop_at_the_end_marker() {
        let mut m = guest_layout::testing::FakeGuest::new();
        let (sp, layer, profile, queue, samples, clock) =
            (0x10000, 0x20000, 0x30000, 0x40000, 0x50000, 0x60000);
        let mut curve = vec![0; HEADER];
        curve[12..16].copy_from_slice(&MAGIC);
        curve[0x12] = 3;
        curve[0x14..0x18].copy_from_slice(&2u32.to_le_bytes());
        let start = Endpoint {
            end: false,
            depth: 2,
            jitter: [0.; 3],
            displacement: 0.,
            width: 0.5,
            argb: [1.; 4],
        };
        start.encode(&mut curve);
        Endpoint {
            end: true,
            width: 0.,
            ..start
        }
        .encode(&mut curve);
        let job = queue + 0x24;
        m.put_u32(sp, 0x5164d)
            .put_u32(sp + 4, samples)
            .put_u32(sp + 0x19dc, job)
            .put_u32(layer + 0xdc, curve.len() as u32)
            .put_u32(layer + 0xe0, profile)
            .put(profile, &curve)
            .put_u32(0x4b6288, queue)
            .put_u32(0x4b6280, 2)
            .put_u32(points::GAME_TIME.base, clock)
            .put_u32(clock + 8, 9)
            .map(samples, 5 * 0x18)
            .put_f32(SPECIAL_FORWARD + 8, 1.)
            .map(0x46713c, 1);
        for i in 0..2 {
            let p = job + i * 0x70;
            m.put_u32(p - 4, 0x508d0)
                .put_u32(p + 4, 123)
                .put_u32(p + 8, 456)
                .put_u8(p + 1, 255)
                .put_f32(p + 0x10, 2. * i as f32);
        }
        assert!(prepare(&mut m, sp, layer).unwrap());
        assert_eq!(m.u32(sp + 8).unwrap(), 5);
        assert_eq!(m.vec3(samples + 2 * 0x18).unwrap(), [1., 0., 0.]);
        assert_eq!(m.f32(samples + 2 * 0x18 + 0xc).unwrap(), 0.25);
        assert_eq!(m.u32(sp + 0x20).unwrap(), 0x484c0300);
        m.put_u32(sp + 0x19dc, job + 0x70);
        assert!(!prepare(&mut m, sp, layer).unwrap());
    }
}
