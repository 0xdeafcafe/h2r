//! Retain slowly changing source fields at the sender. Equality is checked
//! before reuse; first observations and every changed cold field go through
//! the ordinary v7 validator/quantizer. The receiver needs no raw-float cache.
use super::*;
#[derive(Default)]
pub(super) struct Projection {
    src: Lists,
    players: Vec<FramePlayer>,
    vehicles: Vec<FrameVehicle>,
    items: Vec<FrameItem>,
    objects: Vec<FrameObject>,
    machines: Vec<FrameMachine>,
}
fn same_player(a: &FramePlayer, b: &FramePlayer) -> bool {
    a.aim.is_some() == b.aim.is_some()
        && a.vel.is_some() == b.vel.is_some()
        && a.shield.is_some() == b.shield.is_some()
        && a.index == b.index
        && a.alive == b.alive
        && a.vehicle == b.vehicle
        && a.species == b.species
        && a.weapon == b.weapon
        && a.stance == b.stance
        && a.speed == b.speed
        && a.grenade == b.grenade
        && a.body == b.body
        && a.seat == b.seat
        && a.actions == b.actions
        && a.actions_ago == b.actions_ago
        && a.anim.as_ref().map(|a| &a.name) == b.anim.as_ref().map(|a| &a.name)
}
fn player_hot(p: &FramePlayer, e: &mut Entity) -> Result<()> {
    e[2] = Val::Pos(qpos(p.pos)?);
    e[3] = Val::Int(qyaw(p.yaw)?);
    e[6] = opt(p.aim, unit_aim)?;
    e[8] = shots(p.shots);
    e[13] = opt(p.vel, |v| qvel(v).map(Val::Vel))?;
    e[14] = shots(p.fire);
    e[16] = opt(p.shield, |v| {
        qfixed(v, SHIELD_SCALE, "shield").map(Val::Int)
    })?;
    e[20] = opt(p.anim.as_ref(), |a| anim_frame(a.frame))?;
    Ok(())
}
fn same_vehicle(a: &FrameVehicle, b: &FrameVehicle) -> bool {
    a.id == b.id && a.kind == b.kind && a.parent == b.parent && a.occupied == b.occupied
}
fn vehicle_hot(v: &FrameVehicle, e: &mut Entity) -> Result<()> {
    e[2] = Val::Pos(qpos(v.pos)?);
    e[3] = Val::Dir(oct(v.forward)?);
    e[4] = Val::Dir(oct(v.up)?);
    e[7] = opt(v.aim, |a| oct(a).map(Val::Dir))?;
    e[8] = opt(v.vel, |v| qvel(v).map(Val::Vel))?;
    Ok(())
}
fn project_rows<T: Clone>(
    raw: &[T],
    old: &mut Vec<T>,
    out: &mut Vec<Entity>,
    max: usize,
    same: fn(&T, &T) -> bool,
    hot: fn(&T, &mut Entity) -> Result<()>,
    full: fn(&T) -> Result<Entity>,
) -> Result<u32> {
    if raw.len() > max {
        return Err(PackError::Value("too many entities"));
    }
    old.truncate(raw.len());
    out.truncate(raw.len());
    let mut held = 0u32;
    for (i, row) in raw.iter().enumerate() {
        if let Some(prev) = old.get_mut(i) {
            if same(row, prev) {
                hot(row, &mut out[i])?;
                if i < 32 {
                    held |= 1 << i;
                }
            } else {
                out[i] = full(row)?;
                prev.clone_from(row);
            }
        } else {
            out.push(full(row)?);
            old.push(row.clone());
        }
    }
    Ok(held)
}
fn static_list<T: Clone + PartialEq>(
    raw: &[T],
    old: &mut Vec<T>,
    out: &mut Vec<Entity>,
    max: usize,
    full: fn(&T) -> Result<Entity>,
) -> Result<bool> {
    if old == raw {
        return Ok(true);
    }
    *out = collect(raw, full, max)?;
    raw.clone_into(old);
    Ok(false)
}
impl Projection {
    pub(super) fn get(&mut self, f: &Frame) -> Result<(&Lists, u8, u32)> {
        let lists = &mut self.src.lists;
        let held_players = project_rows(
            &f.players,
            &mut self.players,
            &mut lists[0],
            LISTS[0].1,
            same_player,
            player_hot,
            player_vals,
        )?;
        project_rows(
            &f.vehicles,
            &mut self.vehicles,
            &mut lists[1],
            LISTS[1].1,
            same_vehicle,
            vehicle_hot,
            vehicle_vals,
        )?;
        lists[2] = collect(&f.actors, actor_vals, LISTS[2].1)?;
        let m = static_list(
            &f.machines,
            &mut self.machines,
            &mut lists[3],
            LISTS[3].1,
            machine_vals,
        )?;
        let o = static_list(
            &f.objects,
            &mut self.objects,
            &mut lists[4],
            LISTS[4].1,
            object_vals,
        )?;
        lists[5] = collect(&f.projectiles, projectile_vals, LISTS[5].1)?;
        let i = static_list(
            &f.items,
            &mut self.items,
            &mut lists[6],
            LISTS[6].1,
            item_vals,
        )?;
        self.src.hits = collect(&f.hits, hit_vals, MAX_FRAME_HITS)?;
        Ok((
            &self.src,
            (u8::from(m) << 3) | (u8::from(o) << 4) | (u8::from(i) << 6),
            held_players,
        ))
    }
}
