//! The transient values have already disappeared by the next agent tick.
use super::*;
use guest_layout::points;
use ::hooks::{BuildError, Builder, Feature, Manifest, Obs, SnapshotPolicy};
use std::collections::BTreeMap;

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Round {
    start: Option<u32>,
    last: u32,
    error: Option<String>,
    players: BTreeMap<u32, (u32, Option<u32>)>,
}
impl Round {
    fn control(&mut self, start: u32, tick: u32, player: u32, acted: bool) {
        if self.start != Some(start) || tick < self.last {
            self.players.clear();
            self.error = None;
        }
        self.start = Some(start);
        self.last = tick;
        let entry = self.players.entry(player).or_insert((tick, None));
        if acted {
            entry.1.get_or_insert(tick);
        }
    }
    pub(super) fn view(&self) -> Value {
        json!({"error":self.error,"round_start":self.start, "players": self.players.iter().map(|(p,(control,action))|
            json!({"player":p & 0xffff, "datum":p, "first_control":control, "first_action":action})).collect::<Vec<_>>()})
    }
}

fn control(o: &Obs, round: &Mutex<Round>) -> Result<(), MemFault> {
    let m = o.guest();
    let m: &dyn GuestMemory = &*m;
    // The two real per-player control packet submissions. 0x150808 is the
    // neutral packet path and does not demonstrate control.
    if !matches!(m.u32(o.cpu.esp)?, 0x150760 | 0x15077a) {
        return Ok(());
    }
    let Some((objects::BIPED, body)) = objects::get(m, DatumHandle(o.cpu.eax))? else {
        return Ok(());
    };
    let player = m.u32(body.addr + l::UNIT_PLAYER)?;
    let engine = m.u32(l::ENGINE_GLOBALS_PTR)?;
    let time = m.u32(l::GAME_TIME_PTR)?;
    if player == u32::MAX || engine == 0 || time == 0 {
        return Ok(());
    }
    let start = m.u32(engine + 0x70)?;
    let tick = m.u32(time + l::TIME_TICK)?;
    let packet = o.cpu.ecx;
    let acted = m.vec3(packet + 0x14)?.iter().any(|v| v.abs() > 0.)
        || m.f32(packet + 0x20)? > 0.
        || m.f32(packet + 0x24)? > 0.;
    lock(round).control(start, tick, player, acted);
    Ok(())
}

fn vitality(m: &dyn GuestMemory, datum: u32) -> Result<Option<[f32; 2]>, MemFault> {
    let Some((_, body)) = objects::get(m, DatumHandle(datum))? else {
        return Ok(None);
    };
    Ok(Some([body.health(m)?, body.shield(m)?]))
}

fn collision(m: &dyn GuestMemory, at: u32, projectile: u32) -> Result<Value, MemFault> {
    let kind = m.u32(at)?;
    let object = kind == 4;
    Ok(
        json!({"projectile": projectile, "kind":kind, "position":m.vec3(at + 8)?,
        "object":if object { Some(m.u32(at + 0x40)?) } else { None },
        "region":if object { Some(m.u16(at + 0x44)? as i16) } else { None },
        "node":if object { Some(m.u16(at + 0x46)? as i16) } else { None },
        "material":m.u16(at + 0x24)? as i16}),
    )
}

fn projectile(o: &Obs, c: &mut Capture) -> Result<(), MemFault> {
    let guard = o.guest();
    let m: &dyn GuestMemory = &*guard;
    let sp = o.cpu.esp;
    if m.u32(sp)? != 0x1052ca {
        return Ok(());
    }
    // The caller has pushed only the return address. These are the barrel
    // fire routine's original arguments and live projectile-loop locals.
    let frame = sp + 4;
    if Some(m.u32(frame + 0x1eac)?) != c.weapon {
        return Ok(());
    }
    let Some(shot) = c.shots.last_mut() else {
        c.error = Some("projectile arrived without its barrel-fire event".into());
        return Ok(());
    };
    let before = m.vec3(frame + 0x28)?;
    let after = m.vec3(frame + 0x12c)?;
    let angle = direction_angle(before, after);
    let error = m.f32(frame + 0x54)?;
    let bloom = m.f32(m.u32(frame + 0x34)? + 0x1c)?;
    let tag = objects::get(m, DatumHandle(o.cpu.ecx))?
        .map(|(_, body)| m.u32(body.addr))
        .transpose()?;
    if angle.is_none() || !error.is_finite() || !bloom.is_finite() {
        c.error = Some("invalid projectile direction or error".into());
        return Ok(());
    }
    let pellets = shot["projectiles"]
        .as_array_mut()
        .expect("shot owns an array");
    pellets.push(json!({"datum":o.cpu.ecx,"definition":tag,"bloom":bloom,"cone":error,"angle":angle,"direction":after}));
    Ok(())
}

fn direction_angle(a: [f32; 3], b: [f32; 3]) -> Option<f64> {
    let a = a.map(f64::from);
    let b = b.map(f64::from);
    let norm = |v: [f64; 3]| v.iter().map(|x| x * x).sum::<f64>().sqrt();
    let d = a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>() / (norm(a) * norm(b));
    d.is_finite().then(|| d.clamp(-1., 1.).acos())
}

impl Feature for Probes {
    fn manifest(&self) -> Manifest {
        Manifest::new("agent-probes", "Agent behaviour probes").doc("docs/runtime/agent-api.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let round = self.round.clone();
        b.game().on_start("clear round history", move |_| {
            *lock(&round) = Round::default()
        });
        let round = self.round.clone();
        b.raw().observe_before(
            &points::UNIT_APPLY_CONTROL,
            "first applied player control",
            move |o| {
                if let Err(e) = control(o, &round) {
                    lock(&round).error = Some(format!("control packet read at {:#x}", e.addr));
                }
            },
        );
        let active = self.active.clone();
        b.raw()
            .observe_before(&points::WEAPON_FIRE, "capture barrel fire", move |o| {
                let Some(c) = lock(&active).upgrade() else {
                    return;
                };
                let mut c = lock(&c);
                if c.weapon.is_some() && o.arg_u32("weapon") == c.weapon {
                    c.shots.push(
                        json!({"barrel":o.arg_u32("barrel").map(|b| b as u16),"projectiles":[]}),
                    );
                }
            });
        let active = self.active.clone();
        b.raw().observe_before(
            &points::BARREL_PROJECTILE,
            "capture launched projectile",
            move |o| {
                let Some(c) = lock(&active).upgrade() else {
                    return;
                };
                let mut c = lock(&c);
                if c.weapon.is_some()
                    && let Err(e) = projectile(o, &mut c)
                {
                    c.error = Some(format!("projectile read at {:#x}", e.addr));
                }
            },
        );
        let active = self.active.clone();
        // An impact may call another impact while applying effects. Only
        // attribute a damage report to the innermost call on this thread.
        let stack = Arc::new(Mutex::new(BTreeMap::<u32, Vec<u32>>::new()));
        let scopes = stack.clone();
        b.raw()
            .observe_before(&points::PROJECTILE_IMPACT, "capture collision", move |o| {
                let Some(c) = lock(&active).upgrade() else {
                    return;
                };
                let mut c = lock(&c);
                if c.weapon.is_some() {
                    return;
                }
                let g = o.guest();
                let m: &dyn GuestMemory = &*g;
                let result = (|| {
                    let projectile = m.u32(o.cpu.esp + 4)?;
                    lock(&scopes)
                        .entry(o.thread_id)
                        .or_default()
                        .push(projectile);
                    collision(m, m.u32(o.cpu.esp + 8)?, projectile)
                })();
                match result {
                    Ok(mut hit) => {
                        if let Some(unit) = hit["object"].as_u64() {
                            match vitality(m, unit as u32) {
                                Ok(v) => hit["before"] = json!(v),
                                Err(e) => {
                                    c.error = Some(format!("impact vitality at {:#x}", e.addr))
                                }
                            }
                        }
                        c.impacts.push(hit);
                    }
                    Err(e) => c.error = Some(format!("impact read at {:#x}", e.addr)),
                }
            });
        let scopes = stack.clone();
        let active_after = self.active.clone();
        b.raw()
            .observe_after(&points::PROJECTILE_IMPACT, "finish impact", move |o| {
                let Some(id) = lock(&scopes).get_mut(&o.thread_id).and_then(Vec::pop) else {
                    return;
                };
                let Some(c) = lock(&active_after).upgrade() else {
                    return;
                };
                let mut c = lock(&c);
                if let Some(hit) = c
                    .impacts
                    .iter_mut()
                    .rev()
                    .find(|v| v["projectile"] == id && v.get("after").is_none())
                    && let Some(unit) = hit["object"].as_u64()
                {
                    let g = o.guest();
                    match vitality(&*g, unit as u32) {
                        Ok(v) => hit["after"] = json!(v),
                        Err(e) => c.error = Some(format!("post-impact vitality at {:#x}", e.addr)),
                    }
                }
            });
        let active = self.active.clone();
        b.raw()
            .observe_before(&points::DAMAGE_REPORT, "capture impact damage", move |o| {
                let Some(c) = lock(&active).upgrade() else {
                    return;
                };
                let Some(projectile) = lock(&stack)
                    .get(&o.thread_id)
                    .and_then(|v| v.last().copied())
                else {
                    return;
                };
                let (Some(report), Some(victim)) = (o.arg_u32("report"), o.arg_u32("victim"))
                else {
                    return;
                };
                let mut c = lock(&c);
                match live_telemetry::hit(&o.guest(), report, victim) {
                    Ok(hit) => c
                        .damage
                        .push(json!({"projectile":projectile,"victim":victim,
                    "effect":hit.effect,"shield":hit.shield,"body":hit.body})),
                    Err(e) => c.error = Some(format!("damage report: {e:?}")),
                }
            });
        let active = self.active.clone();
        let save = self.round.clone();
        let load = self.round.clone();
        b.snapshot(SnapshotPolicy::section_forbid_while(
            move || {
                lock(&active)
                    .upgrade()
                    .map(|_| "behaviour capture in progress".into())
            },
            move || serde_json::to_vec(&*lock(&save)).expect("round history serializes"),
            move |bytes, _| {
                *lock(&load) = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
                Ok(())
            },
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separates_control_from_action_across_rounds_and_rewinds() {
        let mut r = Round::default();
        r.control(30, 32, 0, false);
        r.control(30, 40, 0, true);
        assert_eq!(r.players[&0], (32, Some(40)));
        r.control(30, 33, 1, true);
        assert!(!r.players.contains_key(&0));
        r.control(90, 91, 0, false);
        assert_eq!(r.players.len(), 1);
    }
    #[test]
    fn measures_angle_from_launched_direction() {
        assert_eq!(direction_angle([0.; 3], [1.; 3]), None);
        assert_eq!(
            direction_angle([1., 0., 0.], [0., 2., 0.]),
            Some(std::f64::consts::FRAC_PI_2)
        );
    }
    #[test]
    fn structure_collision_does_not_read_object_fields() {
        let mut m = guest_layout::testing::FakeGuest::new();
        m.map(0x1000, 0x26);
        m.put_u32(0x1000, 1).put_u16(0x1024, 7);
        let v = collision(&m, 0x1000, 88).unwrap();
        assert!(v["node"].is_null());
        assert_eq!(v["material"], 7);
    }
}
