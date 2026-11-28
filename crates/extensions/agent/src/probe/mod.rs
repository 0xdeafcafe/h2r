//! Tick-boundary measurements with scoped capture of transient engine events.
pub(crate) mod animation;
mod hooks;
use crate::{
    Access, Agent,
    sim::{SimHandle, calls, state},
};
use agent_proto::{
    Boundary, RpcError, code, method,
    probe::{self, Animation, Hit, Weapon},
};
use guest_layout::{DatumHandle, GuestMemory, MemFault, view::objects};
use live_telemetry::layout as l;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

fn lock<T>(v: &Mutex<T>) -> MutexGuard<'_, T> {
    v.lock().unwrap_or_else(|e| e.into_inner())
}
fn invalid(s: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, s)
}
fn unavailable(s: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, s)
}
fn fault(e: MemFault) -> RpcError {
    unavailable(format!("probe read fault at {:#x}", e.addr))
}
fn params<T: serde::de::DeserializeOwned>(p: Value) -> Result<T, RpcError> {
    serde_json::from_value(p).map_err(|e| invalid(e.to_string()))
}

#[derive(Default)]
struct Capture {
    weapon: Option<u32>,
    shots: Vec<Value>,
    impacts: Vec<Value>,
    damage: Vec<Value>,
    error: Option<String>,
}

#[derive(Clone, Default)]
pub struct Probes {
    active: Arc<Mutex<Weak<Mutex<Capture>>>>,
    round: Arc<Mutex<hooks::Round>>,
}

fn datum(s: &str) -> Result<u32, RpcError> {
    if s.len() != 8 {
        return Err(invalid("expected an eight-digit object datum"));
    }
    u32::from_str_radix(s, 16).map_err(|e| invalid(e.to_string()))
}

fn weapon_state(m: &dyn GuestMemory, object: u32) -> Result<Value, RpcError> {
    let Some((objects::WEAPON, w)) = objects::get(m, DatumHandle(object)).map_err(fault)? else {
        return Err(unavailable("the measured weapon no longer exists"));
    };
    let heat = m.f32(w.addr + 0x180).map_err(fault)?;
    if !heat.is_finite() {
        return Err(unavailable("non-finite weapon heat"));
    }
    Ok(
        json!({"loaded": m.u16(w.addr + l::WEAPON_MAGAZINE_LOADED).map_err(fault)? as i16,
        "reserve": m.u16(w.addr + l::WEAPON_MAGAZINE_RESERVE).map_err(fault)? as i16,
        "magazine_state": m.u8(w.addr + 0x228).map_err(fault)?, "heat":heat}),
    )
}

pub(crate) fn line(
    m: &dyn GuestMemory,
    target: u32,
    node: Option<u16>,
    from: [f32; 3],
    to: [f32; 3],
) -> Result<Option<([f32; 3], [f32; 3], objects::Body)>, MemFault> {
    let Some((objects::BIPED | objects::VEHICLE, body)) = objects::get(m, DatumHandle(target))?
    else {
        return Ok(None);
    };
    let (from, to) = if let Some(node) = node {
        let size = m.u16(body.addr + 0x114)? as u32;
        let offset = m.u16(body.addr + 0x116)? as i16;
        if offset < 0 || (u32::from(node) + 1) * 0x34 > size {
            return Ok(None);
        }
        let matrix = body.addr + offset as u32 + u32::from(node) * 0x34;
        let origin = m.vec3(matrix + 0x28)?;
        let axes = [
            m.vec3(matrix + 4)?,
            m.vec3(matrix + 0x10)?,
            m.vec3(matrix + 0x1c)?,
        ];
        let scale = m.f32(matrix)?;
        let world = |p: [f32; 3]| -> [f32; 3] {
            std::array::from_fn(|i| {
                origin[i] + scale * (p[0] * axes[0][i] + p[1] * axes[1][i] + p[2] * axes[2][i])
            })
        };
        (world(from), world(to))
    } else {
        (from, to)
    };
    let delta: [f32; 3] = std::array::from_fn(|i| to[i] - from[i]);
    let len = delta.iter().map(|v| v * v).sum::<f32>().sqrt();
    Ok((len.is_finite() && len > 0.).then(|| (from, delta.map(|v| v / len), body)))
}

impl Probes {
    fn begin(&self, ticks: u64, weapon: Option<u32>) -> Result<Arc<Mutex<Capture>>, RpcError> {
        if !(1..=290).contains(&ticks) {
            return Err(invalid("ticks must be 1..290"));
        }
        let mut active = lock(&self.active);
        if active.upgrade().is_some() {
            return Err(unavailable("another probe is capturing"));
        }
        let capture = Arc::new(Mutex::new(Capture {
            weapon,
            ..Default::default()
        }));
        *active = Arc::downgrade(&capture);
        Ok(capture)
    }

    pub fn install(&self, agent: &Agent, sim: &SimHandle) {
        let (probes, session) = (self.clone(), sim.clone());
        agent.add_game_method(probe::method::WEAPON, Access::Act, move |p, ctx| {
            session.writable_session(ctx)?;
            let p: Weapon = params(p)?;
            let weapon = datum(&p.object)?;
            let capture = probes.begin(p.ticks, Some(weapon))?;
            let (initial, port) = ctx.with_guest(|m| {
                if state::held_object(m, p.player).as_deref() != Some(&p.object) {
                    return Err(unavailable(
                        "the player is not holding the specified weapon",
                    ));
                }
                let player = state::player(m, p.player)?;
                let port = m.i32(player.addr + l::PLAYER_CONTROLLER).map_err(fault)?;
                if !(0..4).contains(&port) {
                    return Err(invalid("weapon probe needs a local controller"));
                }
                Ok((weapon_state(m, weapon)?, port))
            })?;
            let mut duration = 0;
            for step in &p.steps {
                step.check().map_err(invalid)?;
                let n = step
                    .ticks
                    .filter(|n| (1..=p.ticks).contains(n))
                    .ok_or_else(|| {
                        invalid("each step needs a positive tick duration within the capture")
                    })?;
                if step.ms.is_some() || step.reads.is_some() {
                    return Err(invalid("use tick durations"));
                }
                duration += n;
            }
            if duration > p.ticks || p.steps.is_empty() {
                return Err(invalid("steps must fit the capture"));
            }
            let start = ctx
                .at
                .tick
                .ok_or_else(|| unavailable("missing engine tick"))?;
            ctx.host(method::INPUT_PAD, json!({"player":port, "steps":p.steps}))
                .ok_or_else(|| unavailable("no input tap"))??;
            let mut rows = Vec::new();
            ctx.defer(
                p.ticks + 2,
                Box::new(move |ctx| {
                    if ctx.at.boundary != Boundary::Tick {
                        return None;
                    }
                    let sample = (|| {
                        let tick = ctx
                            .at
                            .tick
                            .ok_or_else(|| unavailable("missing engine tick"))?;
                        if tick != start + rows.len() as u64 + 1 {
                            return Err(unavailable("non-contiguous engine ticks"));
                        }
                        let mut row = ctx.with_guest(|m| {
                            if state::held_object(m, p.player).as_deref() != Some(&p.object) {
                                return Err(unavailable("weapon changed during capture"));
                            }
                            weapon_state(m, weapon)
                        })?;
                        let mut c = lock(&capture);
                        if let Some(e) = c.error.take() {
                            return Err(unavailable(e));
                        }
                        row["tick"] = json!(tick);
                        row["shots"] = json!(std::mem::take(&mut c.shots));
                        rows.push(row);
                        Ok(())
                    })();
                    if let Err(e) = sample {
                        return Some(Err(e));
                    }
                    (rows.len() as u64 == p.ticks)
                        .then(|| Ok(json!({"initial":initial,"ticks":rows})))
                }),
            );
            Ok(Value::Null)
        });

        let session = sim.clone();
        agent.add_game_method(probe::method::ANIMATION, Access::Act, move |p, ctx| {
            session.writable_session(ctx)?;
            let p: Animation = params(p)?;
            let queue = session.probe_queue()?;
            let [mode, class, kind, action] = &p.path;
            let path = [
                session.string_id(mode)?,
                session.string_id(class)?,
                session.string_id(kind)?,
                session.string_id(action)?,
            ];
            let ticket = calls::submit(
                &queue,
                calls::Call::Animation {
                    unit: datum(&p.unit)?,
                    path,
                    overlay: p.overlay,
                },
            )
            .map_err(unavailable)?;
            let session = session.clone();
            ctx.defer(
                30,
                Box::new(move |_| {
                    let out = ticket.outcome()?;
                    Some(out.and_then(|mut v| {
                        let graph = v["graph"]
                            .as_u64()
                            .ok_or_else(|| unavailable("missing graph"))?
                            as u32;
                        v["graph_name"] =
                            json!(session.probe_tag(&format!("0x{graph:08x}"), "jmad")?.name);
                        v["clip"] = json!(
                            session.string(
                                v["animation"]
                                    .as_u64()
                                    .ok_or_else(|| unavailable("missing animation"))?
                                    as u32
                            )?
                        );
                        v["request"] = json!(p.path);
                        let mut names = Vec::new();
                        for id in v["selected"]
                            .as_array()
                            .ok_or_else(|| unavailable("missing selected path"))?
                        {
                            let id = id
                                .as_u64()
                                .ok_or_else(|| unavailable("invalid selected path"))?
                                as u32;
                            names.push(if id == u32::MAX {
                                None
                            } else {
                                Some(session.string(id)?)
                            });
                        }
                        v["selected_names"] = json!(names);
                        Ok(v)
                    }))
                }),
            );
            Ok(Value::Null)
        });

        let (probes, session) = (self.clone(), sim.clone());
        agent.add_game_method(probe::method::HIT, Access::Act, move |p, ctx| {
            session.writable_session(ctx)?;
            let p: Hit = params(p)?;
            let target = datum(&p.target)?;
            let dir: [f32; 3] = std::array::from_fn(|i| p.to[i] - p.from[i]);
            let len = dir.iter().map(|n| n * n).sum::<f32>().sqrt();
            if !len.is_finite()
                || !(0.0001..1e6).contains(&len)
                || !p.from.iter().chain(&p.to).all(|v| v.abs() < 1e6)
            {
                return Err(invalid("expected two distinct finite world coordinates"));
            }
            ctx.with_guest(
                |m| match objects::get(m, DatumHandle(target)).map_err(fault)? {
                    Some((objects::BIPED | objects::VEHICLE, _)) => Ok(()),
                    _ => Err(unavailable("target unit is gone")),
                },
            )?;
            let tag = session.probe_tag(&p.projectile, "proj")?;
            if tag.group != "proj" {
                return Err(invalid("expected a projectile"));
            }
            if p.shield.is_some_and(|s| !(0.0..=1.0).contains(&s)) {
                return Err(invalid("shield must be 0..1"));
            }
            let queue = session.probe_queue()?;
            let capture = probes.begin(p.ticks, None)?;
            let ticket = calls::submit(
                &queue,
                calls::Call::Projectile {
                    tag: tag.datum,
                    target,
                    node: p.node,
                    from: p.from,
                    to: p.to,
                    shield: p.shield,
                },
            )
            .map_err(unavailable)?;
            let mut projectile = None;
            let mut elapsed = 0;
            ctx.defer(
                p.ticks + 2,
                Box::new(move |ctx| {
                    if ctx.at.boundary != Boundary::Tick {
                        return None;
                    }
                    if projectile.is_none() {
                        let result = ticket.outcome()?;
                        match result.and_then(|v| datum(v["object"].as_str().unwrap_or("")))
                        {
                            Ok(id) => projectile = Some(id),
                            Err(e) => return Some(Err(e)),
                        }
                    }
                    elapsed += 1;
                    let mut c = lock(&capture);
                    if let Some(e) = c.error.take() {
                        return Some(Err(unavailable(e)));
                    }
                    if let Some(hit) = c
                        .impacts
                        .iter()
                        .find(|v| v["projectile"].as_u64() == projectile.map(u64::from))
                    {
                        let damage: Vec<_> = c
                            .damage
                            .iter()
                            .filter(|v| {
                                v["projectile"] == hit["projectile"]
                                    && v["victim"].as_u64() == Some(target.into())
                            })
                            .collect();
                        return Some(Ok(json!({"hit":hit, "target":target, "damage":damage})));
                    }
                    (elapsed >= p.ticks)
                        .then(|| Err(unavailable("no impact in the requested tick window")))
                }),
            );
            Ok(Value::Null)
        });
        let probes = self.clone();
        agent.add_game_method(probe::method::ROUND, Access::Read, move |p, _| {
            if !p.is_null() && p != json!({}) {
                return Err(invalid("probe.round takes no arguments"));
            }
            Ok(lock(&probes.round).view())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aborted_captures_release_the_slot() {
        let p = Probes::default();
        let active = p.begin(1, Some(42)).unwrap();
        assert!(p.begin(1, None).is_err());
        drop(active);
        assert!(p.begin(290, None).is_ok());
        assert!(p.begin(291, None).is_err());
    }
    fn body(kind: u8) -> (guest_layout::testing::FakeGuest, u32, u32) {
        use guest_layout::view::fake;
        let mut m = guest_layout::testing::FakeGuest::new();
        m.put_u32(guest_layout::points::OBJECTS.base, 0x2000);
        let table = fake::datums::<objects::Header>(&mut m, 0x2000, 4);
        let (id, h) = fake::push(&mut m, &table);
        h.set_kind(&mut m, kind).unwrap();
        h.set_body(&mut m, 0x4000).unwrap();
        m.map(0x4000, 0x400);
        (m, id.raw(), 0x4000)
    }

    #[test]
    fn weapon_samples_use_the_salted_weapon_instance() {
        let (mut m, id, at) = body(objects::WEAPON);
        m.put_f32(at + 0x180, 0.375)
            .put_u16(at + l::WEAPON_MAGAZINE_LOADED, 5)
            .put_u16(at + l::WEAPON_MAGAZINE_RESERVE, 32)
            .put_u8(at + 0x228, 2);
        assert_eq!(
            weapon_state(&m, id).unwrap(),
            json!({"heat":0.375,"loaded":5,"reserve":32,"magazine_state":2})
        );
        assert!(weapon_state(&m, id ^ 0x10000).is_err());
    }

    #[test]
    fn posed_ray_uses_node_scale_axes_and_translation() {
        let (mut m, id, at) = body(objects::BIPED);
        m.put_u16(at + 0x114, 0x34)
            .put_u16(at + 0x116, 0x200)
            .put_f32(at + 0x200, 2.)
            .put_f32(at + 0x208, 1.)
            .put_f32(at + 0x228, 5.)
            .put_f32(at + 0x22c, 7.);
        let (origin, dir, _) = line(&m, id, Some(0), [1., 0., 0.], [2., 0., 0.])
            .unwrap()
            .unwrap();
        assert_eq!(origin, [5., 9., 0.]);
        assert_eq!(dir, [0., 1., 0.]);
        assert!(line(&m, id, Some(1), [1.; 3], [2.; 3]).unwrap().is_none());
    }
}
