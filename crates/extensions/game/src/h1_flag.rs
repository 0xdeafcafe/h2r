//! The source flag exceeds one native cloth datum. The two native panels
//! share the middle row, with one source mast pin and material on each.
use super::h1_particle_physics::{fault, in_liquid, point_velocity};
use bots::objective;
use guest_layout::datum::DatumArray;
use guest_layout::entry::{Cx, EntryError, EntryResult, GuestCall, Ret};
use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::tags::GroupTag;
use guest_layout::view::{Tags, objects};
use hooks::{BuildError, Builder, Feature, Flow, Manifest, points};
use tags::effects::h1_attachment::PointPhysics;

pub struct H1Flag;
impl Feature for H1Flag {
    fn manifest(&self) -> Manifest {
        Manifest::new("h1-flag", "Halo 1 cloth panels").doc("docs/halo1/objects.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Every panel, point and seam is in the guest's cloth array and
        // tags. The async integration's guest calls are continuations,
        // which the machine's own check refuses while one runs.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        b.raw().before(
            &points::CLOTH_INTEGRATE,
            "integrate source cloth points",
            |call| {
                let id = DatumHandle(call.cpu.eax);
                let active = active_panel(&*call.ctx.memory(), id).map_err(fault)?;
                match active {
                    Some((p, dt)) => call.answer_async(move |cx| Box::pin(integrate(cx, p, dt))),
                    None => Ok(Flow::Continue),
                }
            },
        );
        b.raw()
            .after(
                &points::CLOTH_UPDATE,
                "join the flag panels after the solver",
                |call| join(&mut *call.ctx.memory()).map_err(fault),
            )
            .context_only();
        b.raw()
            .before(
                &points::CLOTH_SHADER,
                "select the flag owner's material",
                |call| {
                    let sp = call.cpu.esp;
                    let selected = {
                        let m = call.ctx.memory();
                        let id = DatumHandle(GuestMemory::u32(&*m, sp + 4).map_err(fault)?);
                        let shader = select_shader(&*m, id).map_err(fault)?;
                        shader
                            .map(|s| GuestMemory::u32(&*m, sp).map(|ret| (s, ret)))
                            .transpose()
                            .map_err(fault)?
                    };
                    if let Some((shader, ret)) = selected {
                        call.cpu.eax = shader;
                        call.cpu.esp = sp.wrapping_add(8);
                        return Ok(Flow::Return(runtime::TransferAbi::ret(ret)));
                    }
                    Ok(Flow::Continue)
                },
            )
            .context_only();
        Ok(())
    }
}

#[derive(Copy, Clone)]
struct Panel {
    instance: u32,
    owner: u32,
    tag: u32,
    width: u32,
    height: u32,
    upper: bool,
}

fn array(m: &dyn GuestMemory) -> Result<Option<DatumArray>, MemFault> {
    let p = m.u32(0x4e0338)?;
    Ok((p != 0).then(|| DatumArray::at(p)))
}

fn panel(m: &dyn GuestMemory, instance: u32) -> Result<Option<Panel>, MemFault> {
    let id = DatumHandle(m.u32(instance + 4)?);
    let Some(tag) = Tags.get(m, id, GroupTag::from_chars(*b"clwd"))? else {
        return Ok(None);
    };
    let upper = match m.u32(tag + 0x34)?.to_le_bytes() {
        x if x == *b"H1F0" => false,
        x if x == *b"H1F1" => true,
        _ => return Ok(None),
    };
    let width = u32::from(m.u16(tag + 0x10)?);
    let height = u32::from(m.u16(tag + 0x12)?);
    if width < 2 || height < 2 || width * height > 128 || width * height != m.u32(tag + 0x4c)? {
        return Ok(None);
    }
    Ok(Some(Panel {
        instance,
        owner: m.u32(instance + 8)?,
        tag,
        width,
        height,
        upper,
    }))
}

fn panels(m: &dyn GuestMemory, a: DatumArray) -> Result<Vec<Panel>, MemFault> {
    let mut out = Vec::new();
    let mut next = 0;
    while let Some(i) = a.next_index(m, next)? {
        if let Some(p) = a.get_absolute(m, i)?
            && let Some(p) = panel(m, p)?
        {
            out.push(p);
        }
        next = i + 1;
    }
    Ok(out)
}

fn select_shader(m: &dyn GuestMemory, id: DatumHandle) -> Result<Option<u32>, MemFault> {
    let Some(a) = array(m)? else {
        return Ok(None);
    };
    let Some(at) = a.try_get(m, id)? else {
        return Ok(None);
    };
    let Some(this) = panel(m, at)? else {
        return Ok(None);
    };
    let game = m.u32(objective::GAME_ENGINE)?;
    // An unassigned team's -1 selects the second shader in H1 (0x9a92e).
    let mut team = -1i16;
    if game != 0 {
        let n = m
            .i32(game + objective::OBJECTIVE_COUNT)?
            .clamp(0, objective::OBJECTIVE_MAX as i32);
        for i in 0..n as u32 {
            let rec = game + objective::OBJECTIVES + i * objective::OBJECTIVE_STRIDE;
            if m.u32(rec + objective::RECORD_OBJECT)? == this.owner {
                team = m.u16(rec + objective::RECORD_TEAM)? as i16;
                break;
            }
        }
    }
    let upper = team != 0;
    if upper == this.upper {
        return Ok(Some(m.u32(this.tag + 0xc)?));
    }
    // Read the owner's other tag even when its native cloth slot is occupied.
    if let Some((_, body)) = objects::get(m, DatumHandle(this.owner))?
        && let Some(tag) = body.definition(m)?
        && let Some(entry) = Tags.entry(m, tag)?
    {
        let owner = entry.address(m)?;
        let widgets = m.u32(owner + 0xa0)?;
        let key = u32::from_le_bytes(if upper { *b"H1F1" } else { *b"H1F0" });
        for i in 0..m.u32(owner + 0x9c)? {
            let row = widgets + i * 8;
            if m.u32(row)? != u32::from_be_bytes(*b"clwd") {
                continue;
            }
            let id = DatumHandle(m.u32(row + 4)?);
            if let Some(tag) = Tags.get(m, id, GroupTag::from_chars(*b"clwd"))?
                && m.u32(tag + 0x34)? == key
            {
                return Ok(Some(m.u32(tag + 0xc)?));
            }
        }
    }
    Ok(None)
}

fn join(m: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let Some(a) = array(m)? else {
        return Ok(());
    };
    let all = panels(m, a)?;
    for lo in all.iter().filter(|p| !p.upper) {
        let Some(hi) = all
            .iter()
            .find(|p| p.upper && p.owner == lo.owner && p.width == lo.width)
        else {
            continue;
        };
        for x in 0..lo.width {
            let seam = |p: &Panel| p.instance + 0x20 + 0x24 * (p.width * (p.height - 1) + x);
            let lhs = seam(lo);
            let rhs = seam(hi);
            // Current and previous Verlet positions, plus the render normal
            // computed just before the update returns (0x117510).
            for offset in [0, 12, 24] {
                let v = m.vec3(lhs + offset)?;
                let w = m.vec3(rhs + offset)?;
                let mut avg: [f32; 3] = std::array::from_fn(|k| (v[k] + w[k]) * 0.5);
                if offset == 24 {
                    let len = avg.iter().map(|x| x * x).sum::<f32>().sqrt();
                    if len > 0. {
                        avg = avg.map(|x| x / len);
                    }
                }
                for (k, val) in avg.into_iter().enumerate() {
                    m.set_f32(lhs + offset + 4 * k as u32, val)?;
                    m.set_f32(rhs + offset + 4 * k as u32, val)?;
                }
            }
        }
    }
    Ok(())
}

fn active_panel(m: &dyn GuestMemory, id: DatumHandle) -> Result<Option<(Panel, f32)>, MemFault> {
    let Some(a) = array(m)? else {
        return Ok(None);
    };
    let Some(at) = a.try_get(m, id)? else {
        return Ok(None);
    };
    let Some(p) = panel(m, at)? else {
        return Ok(None);
    };
    // 0x118190 resets the position history when the owner's speed exceeds this threshold.
    if m.f32(at + 0x1c)? >= m.f32(0x43ff08)? {
        return Ok(None);
    }
    let dt = m.f32(m.u32(points::GAME_TIME.base)? + 4)?;
    Ok((dt.is_finite() && dt > 0.).then_some((p, dt)))
}

type V = [f32; 3];

fn reflected(v: V, normal: V, friction: f32, bounce: f32) -> V {
    let dot = (0..3).map(|k| v[k] * normal[k]).sum::<f32>();
    std::array::from_fn(|k| {
        let perpendicular = dot * normal[k];
        (v[k] - perpendicular) * (1. - friction) - perpendicular * bounce
    })
}

async fn sweep(cx: &Cx, position: V, delta: V) -> Result<Option<(f32, V, V)>, EntryError> {
    // The native particle collider (0x2b935b) reads the hit fraction at
    // +4, the impact point at +8 and the world-space plane normal at +0x28.
    let data = cx.entry().esp.wrapping_sub(24 + 0x68);
    let result = data + 24;
    let args = [0x0080_0001, data, data + 12, u32::MAX, u32::MAX, result]
        .into_iter()
        .chain(position.map(f32::to_bits))
        .chain(delta.map(f32::to_bits))
        .chain([0; 0x68 / 4]);
    if cx.call(GuestCall::new(0x0016_97c0).args(args)).await?.eax as u8 == 0 {
        return Ok(None);
    }
    Ok(Some(cx.read(|m| -> Result<_, MemFault> {
        Ok((
            m.f32(result + 4)?,
            m.vec3(result + 8)?,
            m.vec3(result + 0x28)?,
        ))
    })?))
}

async fn integrate(cx: Cx, p: Panel, dt: f32) -> EntryResult {
    let (physics, friction, bounce) = cx.read(|m| -> Result<_, MemFault> {
        Ok((
            PointPhysics {
                flags: 0,
                particle_type_flags: 0,
                mass: m.f32(p.tag + 0x38)?,
                acceleration: [m.f32(p.tag + 0x3c)?, m.f32(p.tag + 0x40)?],
                drag: [m.f32(p.tag + 0x44)?, m.f32(p.tag + 0x48)?],
                radius: 0.02,
            },
            m.f32(p.tag + 0x24)?,
            m.f32(p.tag + 0x2c)?,
        ))
    })?;
    for i in 0..p.width * p.height {
        let v = p.instance + 0x20 + 0x24 * i;
        let state = cx.read(|m| -> Result<_, MemFault> {
            if m.u32(p.instance + 0x1220 + 4 * (i / 32))? & (1 << (i % 32)) != 0 {
                return Ok(None);
            }
            let pos = m.vec3(v)?;
            let old = m.vec3(v + 12)?;
            let velocity = std::array::from_fn(|k| (pos[k] - old[k]) / dt);
            Ok(Some((
                pos,
                point_velocity(physics, velocity, in_liquid(m, pos)?, dt),
            )))
        })?;
        let Some((mut pos, mut velocity)) = state else {
            continue;
        };
        // H1 0x80970 limits a single point to three impacts per step.
        let mut remaining = dt;
        for _ in 0..3 {
            let delta = velocity.map(|v| v * remaining);
            if delta == [0.; 3] {
                break;
            }
            let Some((fraction, hit, normal)) = sweep(&cx, pos, delta).await? else {
                pos = std::array::from_fn(|k| pos[k] + delta[k]);
                break;
            };
            pos = std::array::from_fn(|k| hit[k] + normal[k] * physics.radius.min(0.005));
            velocity = reflected(velocity, normal, friction, bounce);
            remaining *= 1. - fraction.clamp(0., 1.);
        }
        cx.write(|m| -> Result<(), MemFault> {
            for k in 0..3 {
                let addr = v + 4 * k as u32;
                m.set_f32(addr, pos[k])?;
                m.set_f32(addr + 12, pos[k] - dt * velocity[k])?;
            }
            Ok(())
        })?;
    }
    Ok(Ret::new(0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cloth_impact_applies_source_friction_and_restitution_on_separate_axes() {
        let incoming = [4., -8., 8.];
        let normal = [0., 1., 0.];
        assert_eq!(reflected(incoming, normal, 0.25, 0.5), [3., 4., 6.]);
        assert_eq!(reflected(incoming, normal, 1., 0.), [0.; 3]);
    }
    #[test]
    fn both_cloth_panels_share_the_seam_and_use_the_owning_teams_shader() {
        use guest_layout::{tags, testing::FakeGuest};
        let mut m = FakeGuest::new();
        let (array, instances, bitmap) = (0x10000, 0x20000, 0x30000);
        let (index, groups, entries) = (0x40000, 0x41000, 0x42000);
        let (owner, object_array, objects, body, object_tag) =
            (0x10000, 0x60000, 0x61000, 0x62000, 0x63000);
        let group = u32::from_be_bytes(*b"clwd");
        m.put_u32(0x4e0338, array)
            .put_u32(array + 0x24, 0x1258)
            .put_u32(array + 0x38, 2)
            .put_u32(array + 0x44, instances)
            .put_u32(array + 0x48, bitmap)
            .put_u32(bitmap, 3)
            .put_u32(tags::TAG_INDEX_GLOBAL, index)
            .put_u32(tags::TAG_ENTRIES_GLOBAL, entries)
            .put_u32(index, groups)
            .put_u32(index + 4, 1)
            .put_u32(index + 0x18, 3)
            .put_u32(groups, group)
            .put_u32(points::OBJECTS.base, object_array)
            .put_u32(object_array + 0x38, 1)
            .put_u32(object_array + 0x44, objects)
            .put_u32(object_array + 0x24, 12)
            .put_u16(objects, 1)
            .put_u32(objects + 8, body)
            .put_u32(body, 0x77770002)
            .put_u32(entries + 0x24, 0x77770002)
            .put_u32(entries + 0x28, object_tag)
            .put_u32(object_tag + 0x9c, 2)
            .put_u32(object_tag + 0xa0, 0x64000);
        for i in 0..2 {
            let instance = instances + i * 0x1258;
            let id = 0x77770000 + i;
            let root = 0x43000 + 0x1000 * i;
            let seam = instance + 0x20 + 2 * 0x24;
            m.put_u16(instance, 1)
                .put_u32(instance + 4, id)
                .put_u32(instance + 8, owner)
                .put_u32(entries + 16 * i, group)
                .put_u32(entries + 16 * i + 4, id)
                .put_u32(entries + 16 * i + 8, root)
                .put_u32(0x64000 + 8 * i, group)
                .put_u32(0x64004 + 8 * i, id)
                .put(root + 0x34, if i == 0 { b"H1F0" } else { b"H1F1" })
                .put_u32(root + 0xc, 10 + i)
                .put_u16(root + 0x10, 2)
                .put_u16(root + 0x12, 2)
                .put_u32(root + 0x4c, 4)
                .put_f32(seam, 2. + 4. * i as f32)
                .put_f32(seam + 12, 3. + 4. * i as f32)
                .put_f32(seam + 24 + 4 * i, 1.);
        }
        join(&mut m).unwrap();
        let game = 0x50000;
        m.put_u32(objective::GAME_ENGINE, game)
            .put_u32(game + objective::OBJECTIVE_COUNT, 1)
            .put_u32(
                game + objective::OBJECTIVES + objective::RECORD_OBJECT,
                owner,
            );
        for i in 0..2 {
            let seam = instances + i * 0x1258 + 0x20 + 2 * 0x24;
            assert_eq!(m.f32(seam).unwrap(), 4.);
            assert_eq!(m.f32(seam + 12).unwrap(), 5.);
            assert!((m.f32(seam + 24).unwrap() - 0.5_f32.sqrt()).abs() < 1e-6);
            for team in 0..2u16 {
                m.put_u16(game + objective::OBJECTIVES + objective::RECORD_TEAM, team);
                assert_eq!(
                    select_shader(&m, DatumHandle(0x10000 + i)).unwrap(),
                    Some(10 + u32::from(team))
                );
            }
        }
    }
}
