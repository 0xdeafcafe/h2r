//! The Halo 2 volume queue already holds the animated world transform of
//! each attachment marker. Its sort permutes indices, leaving the marker
//! records consecutive (0x1e392, 0x1e3d0). Use those transforms for the
//! source glow curve at the native billboard submission.
use engine::light_constants::global::{SPECIAL_FORWARD, SPECIAL_UP};
use guest_layout::memory::{GuestMemory, MemFault};
use hooks::{BuildError, Builder, Feature, Flow, Manifest, points};
use tags::effects::h1_glow::{Glow, MAGIC};

pub struct H1Glow;

impl Feature for H1Glow {
    fn manifest(&self) -> Manifest {
        Manifest::new("h1-glow", "Halo 1 marker-chain glows").doc("docs/halo1/objects.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Reads the volume queue and the tag, writes the sprites. Its
        // random numbers are seeded from the owner and the tag each call.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        b.raw()
            .before(
                &points::LIGHT_VOLUME_SPRITES,
                "place Halo 1 glow sprites",
                |call| {
                    let (stack, job, layer) = (call.cpu.esp, call.cpu.ebx, call.cpu.esi);
                    let draw = prepare(&mut *call.ctx.memory(), stack, job, layer)
                        .map_err(super::h1_particle_physics::fault)?;
                    if draw {
                        Ok(Flow::Continue)
                    } else {
                        call.cpu.eax = 1;
                        call.cpu.esp = stack.wrapping_add(36);
                        Ok(Flow::Return(runtime::TransferAbi::ret(0x4f797)))
                    }
                },
            )
            .context_only();
        Ok(())
    }
}

type V = [f32; 3];
fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: V, b: V) -> f32 {
    a.into_iter().zip(b).map(|(x, y)| x * y).sum()
}
fn delta(a: V, b: V) -> V {
    std::array::from_fn(|i| a[i] - b[i])
}

#[derive(Clone, Copy, Default)]
struct Marker {
    pos: V,
    forward: V,
    up: V,
}

/// 0x994a0 selects the most forward-facing neighbour, then 0x99565
/// walks the predecessor links from the terminal marker.
fn ordered(markers: &[Marker]) -> Option<Vec<Marker>> {
    let mut next = Vec::new();
    for (i, m) in markers.iter().enumerate() {
        let mut best = 0.0;
        let mut target = None;
        for (j, other) in markers.iter().enumerate() {
            if i == j {
                continue;
            }
            let d = delta(other.pos, m.pos);
            let alignment = dot(m.forward, d) / dot(d, d).sqrt();
            if alignment > best {
                best = alignment;
                target = Some(j);
            }
        }
        next.push(target);
    }
    let mut at = None;
    let mut chain = Vec::new();
    for _ in markers {
        let i = next.iter().rposition(|n| *n == at)?;
        if chain.contains(&i) {
            return None;
        }
        chain.push(i);
        at = Some(i);
    }
    Some(chain.into_iter().rev().map(|i| markers[i]).collect())
}

/// Nonuniform polynomial interpolation through the four nearest source
/// markers (0x983fd, 0x120b0). Distances are cumulative chord lengths.
fn sample(path: &[Marker], knots: &[f32], d: f32, angle: f32, orbit: f32) -> V {
    let segment = knots.partition_point(|&k| k <= d).saturating_sub(1);
    let n = path.len().min(4);
    let first = segment.saturating_sub(1).min(path.len() - n);
    let (sin, cos) = angle.sin_cos();
    let mut pos = [0.; 3];
    for j in first..first + n {
        let mut w = 1.;
        for k in first..first + n {
            if k != j {
                w *= (d - knots[k]) / (knots[j] - knots[k]);
            }
        }
        let m = path[j];
        let left = cross(m.up, m.forward);
        for i in 0..3 {
            pos[i] += w * (m.pos[i] + orbit * (cos * left[i] + sin * m.up[i]));
        }
    }
    pos
}

fn prepare(m: &mut dyn GuestMemory, sp: u32, job: u32, layer: u32) -> Result<bool, MemFault> {
    if m.u32(sp)? != 0x4f797 || m.u32(layer + 0x68)? != u32::from_le_bytes(MAGIC) {
        return Ok(true);
    }
    let mut profile = [0; 48];
    m.read(layer + 0x68, &mut profile)?;
    let glow = Glow::decode(&profile).expect("checked glow signature");
    let count = m.u32(sp + 8)?;
    if !(2..=256).contains(&count) {
        return Ok(false);
    }
    let queue = m.u32(0x4b6288)?;
    let slots = m.u32(0x4b6280)?.min(1024);
    let Some(off) = job.checked_sub(queue).and_then(|d| d.checked_sub(0x24)) else {
        return Ok(false);
    };
    let index = off / 0x70;
    if off % 0x70 != 0 || index >= slots {
        return Ok(false);
    }
    let owner = m.u32(job)?;
    let tag = m.u32(job + 4)?;
    let same = |i| -> Result<bool, MemFault> {
        let p = queue + i * 0x70;
        Ok(m.u32(p + 0x20)? == 0x4f010 && m.u32(p + 0x24)? == owner && m.u32(p + 0x28)? == tag)
    };
    if index > 0 && same(index - 1)? {
        return Ok(false);
    }
    let mut markers = Vec::new();
    for i in index..(index + 5).min(slots) {
        if !same(i)? {
            break;
        }
        let p = queue + i * 0x70;
        markers.push(Marker {
            pos: m.vec3(p + 0x2c)?,
            forward: m.vec3(p + 0x38)?,
            up: m.vec3(p + 0x44)?,
        });
    }
    if markers.len() < 2 {
        return Ok(false);
    }
    let Some(path) = ordered(&markers) else {
        return Ok(false);
    };
    let mut knots = vec![0.];
    for pair in path.windows(2) {
        let v = delta(pair[0].pos, pair[1].pos);
        knots.push(knots.last().unwrap() + dot(v, v).sqrt());
    }
    let length = *knots.last().unwrap();
    if !length.is_finite() || length <= 0. {
        return Ok(false);
    }
    let clock = m.u32(points::GAME_TIME.base)?;
    let seconds = m.i32(clock + 8)? as f32 * m.f32(clock + 4)?;
    let camera_up = m.vec3(SPECIAL_UP)?;
    let camera_right = cross(m.vec3(SPECIAL_FORWARD)?, camera_up);
    let opacity = m.f32(job + 0x30)?;
    let dst = m.u32(sp + 4)?;
    let mut seed = owner.rotate_left(9) ^ tag;
    let mut random = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 16) as f32 / 65536.
    };
    for i in 0..count {
        let orbit = lerp(glow.orbit, random());
        let size = lerp(glow.size, random());
        let tint = random();
        let phase = random() * std::f32::consts::TAU;
        let dir = if i % 2 == 0 { -1. } else { 1. };
        let d = (length * i as f32 / count as f32 + dir * seconds * glow.speed).rem_euclid(length);
        let pos = sample(&path, &knots, d, phase + d * glow.twist / glow.speed, orbit);
        let t = d / length;
        let fade = if glow.fade > 0. {
            (t.min(1. - t) * 2. / glow.fade).clamp(0., 1.)
        } else {
            1.
        };
        let mut colour = ((opacity * fade).clamp(0., 1.) * 255.).round() as u32;
        for shift in [16u32, 8, 0] {
            let values = glow.colours.map(|c| ((c >> shift) & 255u32) as f32);
            colour = (colour << 8) | lerp(values, tint).round() as u32;
        }
        let p = dst + i * 0x18;
        for k in 0..3 {
            let offset = (0.5 - glow.registration[0]) * camera_right[k]
                + (glow.registration[1] - 0.5) * camera_up[k];
            m.set_f32(p + 4 * k as u32, pos[k] + size * offset)?;
        }
        m.set_f32(p + 0xc, size * 0.5)?;
        m.set_f32(p + 0x10, size * 0.5)?;
        m.set_u32(p + 0x14, colour)?;
    }
    Ok(true)
}

fn lerp([a, b]: [f32; 2], t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unordered_markers_follow_forward_links_and_arc_length() {
        let marker = |x| Marker {
            pos: [x, 0., 0.],
            forward: [1., 0., 0.],
            up: [0., 0., 1.],
        };
        // Along a line each forward direction ties. Use the disc's
        // four-point interpolation directly with nonuniform distances.
        let path = [marker(0.), marker(1.), marker(3.), marker(6.)];
        let p = sample(&path, &[0., 1., 3., 6.], 2., 0., 0.25);
        assert!((p[0] - 2.).abs() < 1e-5 && (p[1] - 0.25).abs() < 1e-5);
        let path = ordered(&[marker(3.), marker(1.)]).unwrap();
        assert_eq!(path[0].pos[0], 1.);
    }
}
