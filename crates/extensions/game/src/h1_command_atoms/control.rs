//! The command-list status protocol. The engine's own HS cells hold the
//! state. Reusing an AI slot, replacing a command or restoring a checkpoint
//! cannot transfer an advance to a different unit or command instance.
use super::*;

const GLOBALS: u32 = 0x004f_9380;
const LISTS: u32 = 0x004f_55d8;
const LINKS: u32 = 0x004f_55d4;
const CURRENT: u32 = 0x0050_2410;
const NONE: u32 = u32::MAX;
const RUNNING: i32 = -3;
const WAITING: i32 = -2;
const ADVANCED: i32 = -4;
const CELLS: u32 = 3 + 4 * 256;

#[derive(Clone, Copy, Debug, PartialEq)]
struct ActorState {
    unit: u32,
    command: u32,
    phase: i32,
    fall: u32,
}
impl ActorState {
    fn start(&mut self, unit: u32, command: u32) {
        if self.unit != unit || self.command != command {
            *self = Self {
                unit,
                command,
                phase: RUNNING,
                fall: NONE,
            };
        }
    }
    fn apply(&mut self, op: u16, tick: i32) -> u16 {
        match op {
            2 => self.phase = tick,
            3 if self.phase == ADVANCED => {
                self.phase = RUNNING;
                return 1;
            }
            3 => self.phase = WAITING,
            4 if matches!(self.phase, RUNNING | WAITING | ADVANCED) => self.phase = ADVANCED,
            5 => {
                return match self.phase {
                    WAITING => 2,
                    RUNNING | ADVANCED => 3,
                    n if n >= 0 && (0..=150).contains(&tick.wrapping_sub(n)) => 1,
                    _ => 0,
                };
            }
            _ => {}
        }
        0
    }
    fn read(m: &dyn GuestMemory, at: u32) -> R<Self> {
        Ok(Self {
            unit: m.u32(at)?,
            command: m.u32(at + 8)?,
            phase: m.i32(at + 16)?,
            fall: m.u32(at + 24)?,
        })
    }
    fn store(self, m: &mut dyn GuestMemory, at: u32) -> R<()> {
        m.set_u32(at, self.unit)?;
        m.set_u32(at + 8, self.command)?;
        m.set_i32(at + 16, self.phase)?;
        m.set_u32(at + 24, self.fall)
    }
}

// H1 0x16a603/0x16b23c brackets the list with a falling-damage
// exemption. H2 0xe3727 tests unit flags bit 9 before applying its
// globals/distance damage (0xe3883). Remember only the bit we acquired.
fn restore_fall(m: &mut dyn GuestMemory, s: &mut ActorState) -> R<()> {
    if s.fall != NONE && s.fall == s.command {
        if let Some((_, body)) = objects::get(m, DatumHandle(s.unit))? {
            let flags = m.u32(body.addr + 308)?;
            m.set_u32(body.addr + 308, flags & !(1 << 9))?;
        }
        s.fall = NONE;
    }
    Ok(())
}

fn begin_instance(m: &mut dyn GuestMemory, s: &mut ActorState, unit: u32, command: u32) -> R<()> {
    if s.unit != unit || s.command != command {
        restore_fall(m, s)?;
        s.start(unit, command);
    }
    Ok(())
}

pub(super) fn executing(m: &dyn GuestMemory, unit: u32) -> R<bool> {
    let id = m.u32(CURRENT)?;
    if id == NONE {
        return Ok(false);
    }
    let Some((_, body)) = objects::get(m, DatumHandle(unit))? else {
        return Ok(false);
    };
    let actor = m.u32(body.addr + 0x12c)?;
    if actor == NONE || m.u32(datum(m, ACTORS, actor, ACTOR_SIZE)? + 0x858)? != id {
        return Ok(false);
    }
    generated(m, m.u32(points::SCENARIO.base)?, id)
}

fn generated(m: &dyn GuestMemory, sc: u32, id: u32) -> R<bool> {
    let c = datum(m, COMMANDS, id, COMMAND_SIZE)?;
    let i = u32::from(m.u16(c + 52)?);
    if i >= m.u32(sc + 0x1b8)? {
        return Ok(false);
    }
    let def = m.u32(sc + 0x1bc)? + 40 * i;
    Ok(m.u16(def + 32)? == 5
        && m.u32(def)? == u32::from_le_bytes(*b"h1_c")
        && m.cstr(def, 31)?
            .strip_prefix(b"h1_cmd_")
            .is_some_and(|s| !s.is_empty() && s.iter().all(u8::is_ascii_digit)))
}

fn cells(m: &dyn GuestMemory, sc: u32, i: u32) -> R<Option<u32>> {
    let n = m.u32(sc + 0x1c0)?;
    let pool = m.u32(GLOBALS)?;
    if pool == 0 || i.saturating_add(CELLS) > n.min(m.u32(pool + guest_layout::datum::layout::MAXIMUM_COUNT)?.saturating_sub(1053)) {
        return Ok(None);
    }
    let defs = m.u32(sc + 0x1c4)?;
    let def = defs + i * 40;
    if m.u32(def)? != u32::from_le_bytes(*b"h1_c")
        || m.cstr(def, 31)?.as_slice() != b"h1_cmd_operation"
        || m.u16(def + 32)? != 7
        || m.cstr(def + (CELLS - 1) * 40, 31)?.as_slice() != b"h1_cmd_fall_255"
        || m.u16(def + 40 + 32)? != 31
        || m.u16(def + 80 + 32)? != 7
    {
        return Ok(None);
    }
    Ok(Some(m.u32(m.u32(GLOBALS)? + 68)? + (1053 + i) * 8 + 4))
}

fn model_reference(m: &dyn GuestMemory, unit: u32, offset: u32) -> R<u32> {
    let Some((_, body)) = objects::get(m, DatumHandle(unit))? else {
        return Ok(NONE);
    };
    let Some(def) = body.definition(m)? else {
        return Ok(NONE);
    };
    let Some(tag) = Tags.entry(m, def)? else {
        return Ok(NONE);
    };
    let object = tag.address(m)?;
    if object == 0 {
        return Ok(NONE);
    }
    let model = DatumHandle(m.u32(object + 0x38)?);
    let Some(tag) = Tags.entry(m, model)? else {
        return Ok(NONE);
    };
    let address = tag.address(m)?;
    if address == 0 {
        return Ok(NONE);
    }
    m.u32(address + offset)
}

pub(super) struct Tuning {
    idle: [f32; 2],
    pub(super) recovery: f32,
    pub(super) flying: bool,
    pub(super) gaze_cosine: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            idle: [0.0; 2],
            recovery: 2.0,
            flying: false,
            gaze_cosine: 0.0,
        }
    }
}

fn timing(m: &dyn GuestMemory, sc: u32, i: u32, unit: u32) -> R<Tuning> {
    let defs = m.u32(sc + 0x1c4)?;
    let globals = m.u32(m.u32(GLOBALS)? + 68)? + 1053 * 8 + 4;

    let mut selected = Tuning::default();
    if let Some((kind, body)) = objects::get(m, DatumHandle(unit))?
        && matches!(kind, objects::BIPED | objects::VEHICLE)
    {
        let actor = m.u32(body.addr + 0x12c)?;
        if actor != NONE {
            let birth = m.u32(datum(m, ACTORS, actor, ACTOR_SIZE)? + 56)?;
            let mut j = i + 4;
            let count = m.u32(sc + 0x1c0)?;
            'groups: while j + 3 <= count
                && m.u16(defs + j * 40 + 32)? == 6
                && m.cstr(defs + j * 40, 31)?.starts_with(b"h1_cmd_lo_")
            {
                let group = globals + j * 8;
                j += 3;
                let extended = j + 2 <= count
                    && m.cstr(defs + j * 40, 31)?.starts_with(b"h1_cmd_fly_");
                if extended {
                    j += 2;
                }
                while j < count && m.u16(defs + j * 40 + 32)? == 11 {
                    if birth != 0 && m.u32(globals + j * 8)? == birth {
                        selected.idle = [m.f32(group)?, m.f32(group + 8)?];
                        selected.recovery = m.f32(group + 16)?;
                        if extended {
                            selected.flying = m.f32(group + 24)? != 0.0;
                            selected.gaze_cosine = m.f32(group + 32)?;
                        }
                        break 'groups;
                    }
                    j += 1;
                }
            }
        }
    }
    Ok(selected)
}

pub(super) fn tuning(m: &dyn GuestMemory, sc: u32, unit: u32) -> R<Tuning> {
    let defs = m.u32(sc + 0x1c4)?;
    for i in 0..m.u32(sc + 0x1c0)?.saturating_sub(3) {
        let def = defs + i * 40;
        if m.u16(def + 32)? == 51 && m.cstr(def, 31)?.as_slice() == b"h1_cmd_timing_unit" {
            return timing(m, sc, i, unit);
        }
    }
    Ok(Tuning::default())
}

fn actor_timing(m: &mut dyn GuestMemory, sc: u32, i: u32) -> R<()> {
    let port = m.u32(m.u32(GLOBALS)? + 68)? + (1053 + i) * 8 + 4;
    let selected = timing(m, sc, i, m.u32(port)?)?;
    let values = [selected.idle[0], selected.idle[1], selected.recovery];
    for (field, v) in values.into_iter().enumerate() {
        m.set_f32(port + (field as u32 + 1) * 8, v)?;
    }
    Ok(())
}

fn run(m: &mut dyn GuestMemory, index: u16) -> R<()> {
    if index & 0x8000 != 0 {
        return Ok(());
    }
    let sc = m.u32(points::SCENARIO.base)?;
    if sc == 0 {
        return Ok(());
    }
    let i = u32::from(index);
    let n = m.u32(sc + 0x1c0)?;
    if i + 1 < n {
        let def = m.u32(sc + 0x1c4)? + i * 40;
        if m.u16(def + 32)? == 51
            && m.u32(def)? == u32::from_le_bytes(*b"h1_c")
            && i + 3 < n
            && m.cstr(def, 31)?.as_slice() == b"h1_cmd_timing_unit"
            && m.cstr(def + 120, 31)?.as_slice() == b"h1_cmd_recovery"
        {
            return actor_timing(m, sc, i);
        }
        if m.u16(def + 32)? == 51 && m.u32(def)? == u32::from_le_bytes(*b"h1_c") {
            let addr = m.u32(m.u32(GLOBALS)? + 68)? + (1053 + i) * 8 + 4;
            let offset = match (
                m.cstr(def, 31)?.as_slice(),
                m.u16(def + 72)?,
                m.cstr(def + 40, 31)?.as_slice(),
            ) {
                (b"h1_cmd_graph_unit", 36, b"h1_cmd_unit_graph") => Some(0x14),
                (b"h1_cmd_voice_unit", 8, b"h1_cmd_voice_tag") => {
                    let unit = m.u32(addr)?;
                    let voice = crate::h1_conversation::dialogue::active(m, unit)?;
                    return m.set_u32(addr + 8, voice);
                }
                _ => None,
            };
            if let Some(offset) = offset {
                let reference = model_reference(m, m.u32(addr)?, offset)?;
                return m.set_u32(addr + 8, reference);
            }
        }
    }
    let Some(base) = cells(m, sc, index.into())? else {
        return Ok(());
    };
    let op = m.u16(base)?;
    if !(1..=10).contains(&op) {
        return Ok(());
    }
    let list = m.u32(base + 8)?;
    let tick = m.i32(m.u32(points::GAME_TIME.base)? + 8)?;
    let mut max = 0;
    if list != NONE {
        let header = datum(m, LISTS, list, 12)?;
        let count = m.u16(header + 6)?;
        let mut link = m.u32(header + 8)?;
        let stride = if count > 0 {
            m.u32(m.u32(LINKS)? + 36)?
        } else {
            0
        };
        for _ in 0..count {
            if link == NONE {
                break;
            }
            let node = datum(m, LINKS, link, stride)?;
            let unit = m.u32(node + 4)?;
            link = m.u32(node + 8)?;
            let Some((ty, body)) = objects::get(m, DatumHandle(unit))? else {
                continue;
            };
            if !matches!(ty, objects::BIPED | objects::VEHICLE) {
                continue;
            }
            let actor = m.u32(body.addr + 0x12c)?;
            if actor == NONE || actor & 0xffff >= 256 {
                continue;
            }
            let at = base + 24 + (actor & 255) * 32;
            let mut s = ActorState::read(m, at)?;
            if op <= 3 || op >= 6 {
                let id = m.u32(CURRENT)?;
                if id == NONE {
                    continue;
                }
                begin_instance(m, &mut s, unit, id)?;
            } else {
                // A just-issued command may be polled or advanced before
                // the scheduler enters its first HS expression.
                let ac = datum(m, ACTORS, actor, ACTOR_SIZE)?;
                let id = m.u32(ac + 2136)?;
                if id != NONE && generated(m, sc, id)? {
                    begin_instance(m, &mut s, unit, id)?;
                }
            }
            if s.unit == unit {
                if (7..=10).contains(&op) {
                    if i < 257 {
                        continue;
                    }
                    let previous = m.u32(sc + 0x1c4)? + (i - 1) * 40;
                    if m.cstr(previous, 31)?.as_slice() != b"h1_cmd_pc_value" {
                        continue;
                    }
                    let pc = base - (257 - (actor & 255)) * 8;
                    if op == 7 {
                        max = m.u16(pc)?;
                    } else if op == 8 {
                        let next = m.u16(base - 8)?;
                        m.set_u16(pc, next)?;
                    } else if i >= 514 {
                        let radius = pc - 257 * 8;
                        let value = base - 258 * 8;
                        if op == 9 {
                            let r = m.f32(radius)?;
                            m.set_f32(value, r)?;
                        } else {
                            let r = m.f32(value)?;
                            m.set_f32(radius, r)?;
                        }
                    }
                }
                if op == 6 && s.fall == NONE {
                    let flags = m.u32(body.addr + 308)?;
                    if flags & (1 << 9) == 0 {
                        m.set_u32(body.addr + 308, flags | (1 << 9))?;
                        s.fall = s.command;
                    }
                } else if op == 2 {
                    restore_fall(m, &mut s)?;
                }
                max = max.max(s.apply(op, tick));
                s.store(m, at)?;
            }
        }
    }
    m.set_u16(base + 16, max)
}

// Queue removal also covers commands aborted by alert, death or replacement.
fn removed(m: &mut dyn GuestMemory, actor: u32, id: u32) -> R<()> {
    if actor & 0xffff >= 256 || id == NONE {
        return Ok(());
    }
    let sc = m.u32(points::SCENARIO.base)?;
    if sc == 0 || !generated(m, sc, id)? {
        return Ok(());
    }
    let n = m.u32(sc + 0x1c0)?;
    // The compiler appends this bank last, after all authored globals.
    if let Some(i) = n.checked_sub(CELLS)
        && let Some(base) = cells(m, sc, i)?
    {
        let at = base + 24 + (actor & 255) * 32;
        let mut s = ActorState::read(m, at)?;
        if s.command == id && matches!(s.phase, RUNNING | WAITING | ADVANCED) {
            restore_fall(m, &mut s)?;
            s.phase = m.i32(m.u32(points::GAME_TIME.base)? + 8)?;
            s.store(m, at)?;
        }
    }
    Ok(())
}

pub(super) fn fault(f: MemFault) -> runtime::GuestFault {
    runtime::GuestFault {
        addr: f.addr,
        len: f.len as usize,
        access: if f.write {
            runtime::guest::Access::Write
        } else {
            runtime::guest::Access::Read
        },
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

pub(super) fn install(b: &mut hooks::Builder) {
    b.raw()
        .before(&points::HS_GLOBAL_STORED, "command-list control", |c| {
            let index = c.cpu.eax as u16;
            run(&mut *c.ctx.runtime().memory(), index).map_err(fault)?;
            Ok(Flow::Continue)
        });
    b.raw().before(
        &points::AI_COMMAND_REMOVE,
        "finish the removed command",
        |c| {
            let id = c.arg_u32("command").unwrap_or(NONE);
            let actor = c.cpu.ecx;
            removed(&mut *c.ctx.runtime().memory(), actor, id).map_err(fault)?;
            Ok(Flow::Continue)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn advance_before_wait_is_consumed_once_by_the_same_instance() {
        let mut a = ActorState {
            unit: 1,
            command: 3,
            phase: RUNNING,
            fall: NONE,
        };
        let mut b = a;
        a.apply(4, 10);
        a.start(1, 3);
        assert_eq!(a.apply(3, 11), 1);
        assert_eq!(b.apply(3, 11), 0);
        assert_eq!(b.apply(5, 11), 2);
        assert_eq!(a.apply(3, 12), 0);
        a.apply(4, 12);
        a.start(1, 4);
        assert_eq!(a.apply(3, 13), 0);
        a.apply(4, 13);
        a.start(2, 4);
        assert_eq!(a.apply(3, 13), 0);
    }
    #[test]
    fn changing_instance_restores_only_the_owned_falling_damage_flag() {
        let mut m = guest_layout::FlatMemory::new(0x004e_0000, 0x40000);
        let array = 0x004e_2000;
        let header = 0x004e_2100;
        let body = 0x004e_2200;
        m.set_u32(points::OBJECTS.base, array).unwrap();
        m.set_u32(array + 56, 1).unwrap();
        m.set_u32(array + 36, 12).unwrap();
        m.set_u32(array + 68, header).unwrap();
        m.set_u16(header, 0x4000).unwrap();
        m.set_u32(header + 8, body).unwrap();
        m.set_u32(body + 308, 0x204).unwrap();
        let unit = 0x4000_0000;
        let mut s = ActorState {
            unit,
            command: 3,
            phase: RUNNING,
            fall: 3,
        };
        begin_instance(&mut m, &mut s, unit, 4).unwrap();
        assert_eq!(m.u32(body + 308).unwrap(), 4);
        m.set_u32(body + 308, 0x204).unwrap();
        begin_instance(&mut m, &mut s, unit, 5).unwrap();
        assert_eq!(m.u32(body + 308).unwrap(), 0x204);
    }

    #[test]
    fn completion_includes_tick_150() {
        let mut a = ActorState {
            unit: 1,
            command: 3,
            phase: RUNNING,
            fall: NONE,
        };
        assert_eq!(a.apply(5, 10), 3);
        a.apply(2, 10);
        assert_eq!(a.apply(5, 160), 1);
        assert_eq!(a.apply(5, 161), 0);
    }
}
