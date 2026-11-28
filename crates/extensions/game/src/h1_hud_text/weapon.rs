//! Source weapon states absent from the native HUD input switch. Time is
//! sampled on simulation ticks. The render fork only reads the saved ticks.
mod origin;

use guest_layout::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
use guest_layout::tags::GroupTag;
use guest_layout::view::{Tags, objects};
use hooks::{BuildError, Builder, Flow};
use tags::weapon_hud as input;

use super::flash;
use crate::h1_particle_physics::fault;
use crate::touch_glue::{PC_CONE, PC_HEADER, PC_PULL, PC_STRIDE, PC_TARGET, PLAYER_CONTROL};

const USERS: u32 = 4;
const TYPES: u32 = 19;
// Two rows per user: their personal weapon and their vehicle's weapon.
const ROW: u32 = 4 + TYPES * 4;
const INACTIVE: u32 = u32::MAX;
const BITMAP_GATE: u32 = 0x0013_a602;
const TEXT_GATE: u32 = 0x0013_a659;

#[derive(Default)]
struct Magazine {
    reserve: i16,
    loaded: i16,
    reloading: bool,
    ready: bool,
}

impl Magazine {
    fn read(mem: &dyn GuestMemory, weapon: u32, index: u32, count: u32) -> Result<Self, MemFault> {
        if index >= count {
            return Ok(Self::default());
        }
        let at = weapon + 0x224 + index * 0x10;
        let state = mem.u16(at)?;
        Ok(Self {
            reserve: mem.u16(at + 6)? as i16,
            loaded: mem.u16(at + 8)? as i16,
            reloading: (1..=6).contains(&state),
            ready: state == 0,
        })
    }
}

#[derive(Default)]
struct Weapon {
    magazines: [Magazine; 2],
    heat: f32,
    age: f32,
    grenades: bool,
    firing: bool,
    throwing: bool,
}

impl Weapon {
    // H1's switch at 0xda70a. Type 2 branches directly to false.
    fn warning(&self, kind: u8, low: [i16; 4]) -> bool {
        let m = &self.magazines[usize::from((11..=15).contains(&kind) || kind == 17)];
        match kind {
            3 | 11 => m.reserve != 0 && m.loaded <= low[1],
            4 => self.heat * 100.0 >= f32::from(low[2]),
            5 | 12 => m.reserve <= low[0] && !m.reloading,
            6 => self.age < 1.0 && (1.0 - self.age) * 100.0 <= f32::from(low[3]),
            7 | 13 => m.reloading,
            8 | 14 => m.loaded == 0 && m.reserve == 0 && self.firing,
            9 => !self.grenades && self.throwing,
            10 | 15 => m.reserve == 0 && m.loaded != 0 && m.loaded <= low[1],
            16 | 17 => m.ready,
            18 => self.age == 1.0 && self.firing,
            _ => false,
        }
    }
}

fn control(mem: &dyn GuestMemory, user: u32) -> Result<u32, MemFault> {
    Ok(mem.u32(PLAYER_CONTROL)? + PC_HEADER + user * PC_STRIDE)
}

fn profile(mem: &dyn GuestMemory, widget: u32) -> Result<u32, MemFault> {
    let effect = mem.u32(widget + 0x5c)?;
    mem.u32(effect + 0x3c)
}

fn marked(mem: &dyn GuestMemory, widget: u32) -> Result<bool, MemFault> {
    Ok((input::SCREEN..=input::SCREEN + 2).contains(&mem.u8(widget + 0x1b)?))
}

fn clock_start(previous: u32, active: bool, keep_tail: bool, tick: u32, duration: u32) -> u32 {
    // Most types jump directly to 0xdb094 and keep their first tick as
    // long as their condition holds. Only trigger/throw types have a tail.
    if active {
        if previous == INACTIVE { tick } else { previous }
    } else if keep_tail && previous != INACTIVE && tick.wrapping_sub(previous) < duration {
        previous
    } else {
        INACTIVE
    }
}

fn primary(mem: &dyn GuestMemory, owner: objects::Body) -> Result<u32, MemFault> {
    let slot = mem.u8(owner.addr + 0x212)?;
    if slot < 4 {
        mem.u32(owner.addr + 0x218 + 4 * u32::from(slot))
    } else {
        Ok(INACTIVE)
    }
}

fn vehicle_weapon(mem: &dyn GuestMemory, vehicle: objects::Body) -> Result<u32, MemFault> {
    let gun = primary(mem, vehicle)?;
    if gun != INACTIVE {
        return Ok(gun);
    }
    // 0x139be3..0x139c70 also checks the mounted turret children.
    let mut child = mem.u32(vehicle.addr + 0x10)?;
    while let Some((kind, body)) = objects::get(mem, DatumHandle(child))? {
        if matches!(kind, objects::BIPED | objects::VEHICLE)
            && mem.u32(body.addr + 4)? & (1 << 26) != 0
            && let Some(def) = Tags.entry(mem, DatumHandle(mem.u32(body.addr)?))?
            && mem.u32(def.address(mem)? + 0xbc)? & (1 << 26) != 0
        {
            let gun = primary(mem, body)?;
            if gun != INACTIVE {
                return Ok(gun);
            }
        }
        child = mem.u32(body.addr + 0xc)?;
    }
    Ok(INACTIVE)
}

// H1 0xda8bb checks the idle grenade action as well as the trigger.
// H2 0xe775b resolves the biped action block through a signed displacement.
fn throw_requested(mem: &dyn GuestMemory, unit: u32, flags: u32) -> Result<bool, MemFault> {
    if flags & (1 << 26) == 0 {
        return Ok(false);
    }
    let action = unit.wrapping_add_signed(i32::from(mem.u16(unit + 0x346)? as i16));
    Ok(mem.u8(action + 0x0c)? == 0)
}

fn sample_weapon(
    mem: &mut dyn GuestMemory,
    row: u32,
    unit: objects::Body,
    id: u32,
    tick: u32,
    dt: f32,
) -> Result<(), MemFault> {
    if id != mem.u32(row)? {
        mem.fill(row + 4, ROW - 4, 0xff)?;
        mem.set_u32(row, id)?;
    }
    let Some((objects::WEAPON, body)) = objects::get(mem, DatumHandle(id))? else {
        return Ok(());
    };
    let Some(def) = Tags.get(
        mem,
        DatumHandle(mem.u32(body.addr)?),
        GroupTag::from_chars(*b"weap"),
    )?
    else {
        return Ok(());
    };
    let Some(hud) = Tags.get(
        mem,
        DatumHandle(mem.u32(def + 0x2b4)?),
        GroupTag::from_chars(*b"nhdt"),
    )?
    else {
        return Ok(());
    };
    let count = mem.u32(hud + 8)?;
    let widgets = mem.u32(hud + 12)?;
    if count == 0 || !marked(mem, widgets)? {
        return Ok(());
    }
    let flags = mem.u32(unit.addr + 0x148)?;
    let n = mem.u32(def + 0x2c0)?;
    let state = Weapon {
        magazines: [
            Magazine::read(mem, body.addr, 0, n)?,
            Magazine::read(mem, body.addr, 1, n)?,
        ],
        heat: mem.f32(body.addr + 0x180)?,
        age: mem.f32(body.addr + 0x184)?,
        grenades: mem.u16(unit.addr + 0x23e)? != 0,
        firing: flags & (1 << 16) != 0,
        throwing: throw_requested(mem, unit.addr, flags)?,
    };
    for i in 0..count {
        let widget = widgets + 0x64 * i;
        if mem.u8(widget + 4)? != input::WARNING {
            continue;
        }
        let kind = mem.u8(widget + 5)?;
        if u32::from(kind) >= TYPES {
            continue;
        }
        let p = profile(mem, widget)?;
        let mut low = [0; 4];
        for (i, value) in low.iter_mut().enumerate() {
            *value = mem.u16(p + i as u32 * 2)? as i16;
        }
        let duration = (mem.f32(p + input::FLASH as u32 + 8)? / dt).round_ties_even() as u32;
        let at = row + 4 + 4 * u32::from(kind);
        let start = clock_start(
            mem.u32(at)?,
            state.warning(kind, low),
            matches!(kind, 8 | 9 | 14 | 18),
            tick,
            duration,
        );
        mem.set_u32(at, start)?;
    }
    Ok(())
}

fn sample(mem: &mut dyn GuestMemory, rows: u32) -> Result<(), MemFault> {
    let controls = mem.u32(PLAYER_CONTROL)?;
    let time = mem.u32(points::GAME_TIME.base)?;
    if controls == 0 || time == 0 {
        return Ok(());
    }
    let tick = mem.u32(time + 8)?;
    let dt = mem.f32(time + 4)?;
    for user in 0..USERS {
        let row = rows + user * 2 * ROW;
        let id = mem.u32(controls + PC_HEADER + PC_STRIDE * user)?;
        let unit = objects::get(mem, DatumHandle(id))?;
        if let Some((objects::BIPED, unit)) = unit {
            let personal = primary(mem, unit)?;
            sample_weapon(mem, row, unit, personal, tick, dt)?;
            if let Some(parent) = unit.parent(mem)?
                && let Some((objects::VEHICLE, vehicle)) = objects::get(mem, parent)?
            {
                let gun = vehicle_weapon(mem, vehicle)?;
                sample_weapon(mem, row + ROW, unit, gun, tick, dt)?;
                continue;
            }
        } else {
            mem.set_u32(row, INACTIVE)?;
        }
        mem.set_u32(row + ROW, INACTIVE)?;
    }
    Ok(())
}

fn acquired(mem: &dyn GuestMemory, user: u32) -> Result<bool, MemFault> {
    let pc = control(mem, user)?;
    Ok(mem.u32(pc + PC_TARGET)? != INACTIVE
        && (mem.f32(pc + PC_CONE)? == 1.0 || mem.f32(pc + PC_PULL)? == 1.0))
}

fn target_value(
    mem: &dyn GuestMemory,
    user: u32,
    elevation: bool,
) -> Result<Option<f32>, MemFault> {
    let pc = control(mem, user)?;
    if !acquired(mem, user)? {
        return Ok(None);
    }
    let Some((_, target)) = objects::get(mem, DatumHandle(mem.u32(pc + PC_TARGET)?))? else {
        return Ok(None);
    };
    let Some(from) = origin::eye(mem, DatumHandle(mem.u32(pc)?))? else {
        return Ok(None);
    };
    let Some(to) = origin::position(mem, target)? else {
        return Ok(None);
    };
    let delta: [f32; 3] = std::array::from_fn(|i| to[i] - from[i]);
    // The source XBE's world-units to metres constant at 0x1f1160.
    Ok(Some(
        3.048
            * if elevation {
                delta[2]
            } else {
                delta.iter().map(|v| v * v).sum::<f32>().sqrt()
            },
    ))
}

fn split_screen(mem: &dyn GuestMemory) -> Result<bool, MemFault> {
    let globals = mem.u32(points::PLAYER_GLOBALS.base)?;
    Ok(globals != 0 && mem.u16(globals + 8)? > 1)
}

fn warning_start(
    mem: &dyn GuestMemory,
    rows: u32,
    user: u32,
    id: u32,
    kind: u8,
) -> Result<u32, MemFault> {
    for slot in 0..2 {
        let row = rows + (user * 2 + slot) * ROW;
        if mem.u32(row)? == id && u32::from(kind) < TYPES {
            return mem.u32(row + 4 + 4 * u32::from(kind));
        }
    }
    Ok(INACTIVE)
}

// Read the arguments of the enclosing 0x13a532 draw only at its verified calls.
fn hidden(
    mem: &dyn GuestMemory,
    widget: u32,
    ws: u32,
    sp: u32,
    bp: u32,
    rows: u32,
) -> Result<bool, MemFault> {
    if !matches!(mem.u32(sp)?, BITMAP_GATE | TEXT_GATE) || !marked(mem, widget)? {
        return Ok(false);
    }
    let screen = mem.u8(widget + 0x1b)? - input::SCREEN;
    if screen != 0 {
        let split = split_screen(mem)?;
        if split != (screen == 2) {
            return Ok(true);
        }
    }
    let user = mem.u32(bp + 8)?;
    if ws == 0 || user >= USERS {
        return Ok(true);
    }
    match mem.u8(widget + 4)? {
        input::WARNING => {
            Ok(warning_start(mem, rows, user, mem.u32(ws)?, mem.u8(widget + 5)?)? == INACTIVE)
        }
        input::RANGE | input::ELEVATION => Ok(target_value(mem, user, false)?.is_none()),
        input::MAGAZINES => Ok(mem.u16(ws + 0xa)? == 0),
        _ => Ok(false),
    }
}

fn magazine(mem: &dyn GuestMemory, ws: u32, index: u32) -> Result<Magazine, MemFault> {
    let Some((objects::WEAPON, body)) = objects::get(mem, DatumHandle(mem.u32(ws)?))? else {
        return Ok(Magazine::default());
    };
    let Some(def) = Tags.get(
        mem,
        DatumHandle(mem.u32(body.addr)?),
        GroupTag::from_chars(*b"weap"),
    )?
    else {
        return Ok(Magazine::default());
    };
    Magazine::read(mem, body.addr, index, mem.u32(def + 0x2c0)?)
}

fn number(
    mem: &dyn GuestMemory,
    ws: u32,
    user: u32,
    selector: u8,
    at: u32,
) -> Result<f32, MemFault> {
    Ok(match selector {
        0x33 => f32::from(magazine(mem, ws, 0)?.reserve),
        0x30 => f32::from(mem.u16(ws + 6)? as i16),
        0x32 => ((1.0 - mem.f32(ws + 0x10)?) * 100.0).trunc(),
        input::HEAT_BYTE => (mem.f32(ws + 0x14)? * 255.0).trunc(),
        input::SECOND_RESERVE => f32::from(magazine(mem, ws, 1)?.reserve),
        input::SECOND_LOADED => f32::from(magazine(mem, ws, 1)?.loaded),
        input::RANGE | input::ELEVATION => {
            target_value(mem, user, selector == input::ELEVATION)?.unwrap_or(f32::NAN)
        }
        _ => {
            return Err(MemFault {
                addr: at,
                len: 1,
                write: false,
            });
        }
    })
}

fn supply(mem: &mut dyn GuestMemory, sp: u32, ws: u32) -> Result<bool, MemFault> {
    if !matches!(mem.u32(sp)?, 0x0013_a61c | 0x0013_a673) {
        return Ok(false);
    }
    let selectors = mem.u32(sp + 8)?;
    if !marked(mem, selectors - 4)? {
        return Ok(false);
    }
    let user = mem.u32(sp + 4)?;
    let mut inputs = [0u8; 4];
    mem.read(selectors, &mut inputs)?;
    let [kind, a, b, c] = inputs;
    const LAST_METER: u8 = input::METER + 5;
    let value = match kind {
        input::WARNING => 0.0,
        input::MAGAZINES => (number(mem, ws, user, a, selectors + 1)?
            / f32::from(mem.u16(ws + 0xa)? as i16))
        .trunc(),
        input::METER..=LAST_METER => {
            let source = [
                0x33,
                0x30,
                input::HEAT_BYTE,
                0x32,
                input::SECOND_RESERVE,
                input::SECOND_LOADED,
            ];
            let raw = number(
                mem,
                ws,
                user,
                source[usize::from(kind - input::METER)],
                selectors,
            )?;
            input::meter_fill(raw as i16, [a, b, c])
        }
        0x33
        | 0x32
        | input::RANGE
        | input::ELEVATION
        | input::SECOND_RESERVE
        | input::SECOND_LOADED
        | input::HEAT_BYTE => number(mem, ws, user, kind, selectors)?,
        _ => return Ok(false),
    };
    let out = mem.u32(sp + 12)?;
    mem.set_f32(out, value)?;
    mem.fill(out + 4, 12, 0)?;
    Ok(true)
}

fn target_text(value: f32, digits: u8, fraction: u8, flags: u8) -> String {
    if !value.is_finite() {
        return String::new();
    }
    let digits = usize::from(digits.min(9));
    let whole = value.abs().trunc() as u32 % 10u32.pow(digits as u32);
    let sign = if value.trunc() < 0.0 { "-" } else { "" };
    let width = if flags & 1 != 0 { digits } else { 0 };
    let mut text = format!("{sign}{whole:0width$}");
    if fraction != 0 {
        let places = usize::from(fraction.min(4));
        let sub = ((value.abs().fract() * 10000.0).round_ties_even() as u32 % 10000)
            / 10u32.pow(4 - places as u32);
        text.push_str(&format!(".{sub:0places$}"));
    }
    text
}

fn format_text(mem: &mut dyn GuestMemory, sp: u32, widget: u32) -> Result<bool, MemFault> {
    if mem.u32(sp)? != 0x0022_aa5b || !marked(mem, widget)? {
        return Ok(false);
    }
    if !matches!(mem.u8(widget + 4)?, input::RANGE | input::ELEVATION) {
        return Ok(false);
    }
    let values = mem.u32(sp + 8)?;
    let text = target_text(
        mem.f32(values)?,
        mem.u8(widget + 6)?,
        mem.u8(widget + 5)?,
        mem.u8(widget + 7)?,
    );
    let bytes: Vec<u8> = text
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    mem.write(mem.u32(sp + 12)?, &bytes)?;
    mem.set_u32(mem.u32(sp + 16)?, 6)?; // the converted widget's font is 0 in every mode
    // 0x22a895..0x22a8a3 publishes the same four inputs to the HUD material.
    let mut values_copy = [0; 16];
    mem.read(values, &mut values_copy)?;
    mem.write(0x004e_6980, &values_copy)?;
    Ok(true)
}

// The bitmap routine saves the real caller's EBP at +0x68 (0x22aff2).
fn draw_owner(mem: &dyn GuestMemory, frame: u32) -> Result<Option<(u32, u32)>, MemFault> {
    if mem.u32(frame + 0x6c)? != 0x0013_a62a {
        return Ok(None);
    }
    let outer = mem.u32(frame + 0x68)?;
    let user = mem.u32(outer + 8)?;
    let ws = mem.u32(outer + 0x10)?;
    Ok((user < USERS && ws != 0).then_some((user, ws)))
}

fn aim_profile(mem: &dyn GuestMemory, widget: u32) -> Result<Option<u32>, MemFault> {
    if mem.u32(widget + 0x58)? == 0 {
        return Ok(None);
    }
    let effect = mem.u32(widget + 0x5c)?;
    if mem.u32(effect + 0x4c)? != 26 {
        return Ok(None);
    }
    Ok(Some(mem.u32(effect + 0x50)?))
}

pub(super) fn colour(
    mem: &dyn GuestMemory,
    widget: u32,
    rows: u32,
    frame: u32,
) -> Result<Option<u32>, MemFault> {
    if !marked(mem, widget)? {
        return Ok(None);
    }
    let Some((user, ws)) = draw_owner(mem, frame)? else {
        return Ok(None);
    };
    let (record_at, elapsed, aim) = if mem.u8(widget + 4)? == input::WARNING {
        let start = warning_start(mem, rows, user, mem.u32(ws)?, mem.u8(widget + 5)?)?;
        if start == INACTIVE {
            return Ok(None);
        }
        let elapsed = if mem.u8(widget + 6)? & 1 != 0 {
            Some(flash::elapsed(mem, start)?)
        } else {
            None
        };
        (profile(mem, widget)? + input::FLASH as u32, elapsed, false)
    } else if let Some(p) = aim_profile(mem, widget)? {
        let elapsed = if mem.u8(p + 1)? & 1 != 0 && acquired(mem, user)? {
            Some(flash::elapsed(mem, 0)?)
        } else {
            None
        };
        (p + 2, elapsed, true)
    } else {
        return Ok(None);
    };
    let mut record = [0; 24];
    mem.read(record_at, &mut record)?;
    let mut argb = if let (true, Some(seconds)) = (aim, elapsed) {
        let f = |at| f32::from_le_bytes(record[at..at + 4].try_into().unwrap());
        let count = i16::from_le_bytes(record[16..18].try_into().unwrap());
        let in_window = seconds % f(8) < (f(12) + f(20)) * f32::from(count);
        let at = 4 * usize::from(in_window ^ (record[18] & 1 != 0));
        u32::from_le_bytes(record[at..at + 4].try_into().unwrap())
            .to_be_bytes()
            .map(|c| f32::from(c) / 255.0)
    } else if elapsed.is_some() {
        flash::flash_colour(&record, elapsed)
    } else {
        u32::from_le_bytes(record[..4].try_into().unwrap())
            .to_be_bytes()
            .map(|c| f32::from(c) / 255.0)
    };
    // H1's additive RGB (zero source alpha) is visible. The native alpha
    // combiner needs coverage even for these warning sheets.
    if argb[0] == 0.0 && argb[1..].iter().any(|&c| c > 0.0) {
        argb[0] = 1.0;
    }
    Ok(Some(u32::from_be_bytes(
        argb.map(|c| (c * 255.0).round() as u8),
    )))
}

// A crosshair's split-screen factor multiplies both native basis axes
// before the authored per-axis effect (0x22b055, 0x22b198).
fn split_basis(mem: &mut dyn GuestMemory, sp: u32, frame: u32) -> Result<(), MemFault> {
    if mem.u32(sp)? != 0x0022_b19d {
        return Ok(());
    }
    let widget = mem.u32(frame + 0x78)?;
    if marked(mem, widget)? && mem.u8(widget + 0x33)? != 0 && split_screen(mem)? {
        mem.set_f32(frame, 0.5)?;
        mem.set_f32(frame + 0x34, 0.5)?;
    }
    Ok(())
}

// Resolve a source sprite within the converted native sequence. Direct image
// overlays and level-specific zoom copies already have exactly one image.
fn select_sprite(
    mem: &mut dyn GuestMemory,
    sp: u32,
    frame: u32,
    widget: u32,
    bounds: u32,
    rows: u32,
) -> Result<bool, MemFault> {
    if mem.u32(sp)? != 0x0022_b03a || !marked(mem, widget)? {
        return Ok(false);
    }
    let Some((user, ws)) = draw_owner(mem, frame)? else {
        return Ok(false);
    };
    let index = if mem.u8(widget + 4)? == input::WARNING {
        let period = mem.u16(widget + 0x62)? as i16;
        if period <= 0 {
            0
        } else {
            let start = warning_start(mem, rows, user, mem.u32(ws)?, mem.u8(widget + 5)?)?;
            (flash::elapsed(mem, start)? / f32::from(period)) as u32
        }
    } else if let Some(p) = aim_profile(mem, widget)? {
        let flags = mem.u8(p + 1)?;
        if mem.u8(p)? != 0 || flags & 2 != 0 {
            return Ok(false);
        }
        u32::from(flags & 1 == 0 && acquired(mem, user)?)
    } else {
        return Ok(false);
    };
    let Some(bitmap) = Tags.get(
        mem,
        DatumHandle(mem.u32(widget + 0x24)?),
        GroupTag::from_chars(*b"bitm"),
    )?
    else {
        return Ok(false);
    };
    // Every mode of a converted bitmap names the source sequence.
    let seq = u32::from(mem.u8(widget + 0x30)?);
    let seq = mem.u32(bitmap + 0x40)? + 0x3c * seq;
    let count = mem.u32(seq + 0x34)?;
    if count == 0 {
        return Ok(false);
    }
    let sprite = mem.u32(seq + 0x38)? + 0x20 * (index % count);
    let image = u32::from(mem.u16(sprite)?);
    let data = mem.u32(bitmap + 0x48)? + 0x74 * image;
    let mut uv = [0; 16];
    mem.read(sprite + 8, &mut uv)?;
    mem.write(bounds, &uv)?;
    mem.set_u32(mem.u32(sp + 8)?, image)?;
    mem.set_u32(mem.u32(sp + 12)?, u32::from(mem.u16(data + 4)?))?;
    mem.set_u32(mem.u32(sp + 16)?, u32::from(mem.u16(data + 6)?))?;
    Ok(true)
}

pub(super) fn register(b: &mut Builder) -> Result<u32, BuildError> {
    let rows = b.guest_alloc(USERS * 2 * ROW, runtime::guest::Permissions::RW)?;
    b.raw()
        .before(
            &points::GAME_ACTIVATE,
            "clear weapon warning clocks",
            move |c| {
                GuestMemory::fill(&mut *c.ctx.memory(), rows, USERS * 2 * ROW, 0xff)
                    .map_err(fault)?;
                Ok(Flow::Continue)
            },
        )
        .context_only()
        // The match-start befores (variant options, aim assist, precision
        // items, the HUD swap) run first; clearing these clocks reads none
        // of what they write.
        .after_feature(crate::variant_ext::FEATURE)
        .after_feature(crate::aim_assist::FEATURE)
        .after_feature(crate::precision::FEATURE)
        .after_feature(crate::hud_swap::FEATURE);
    // Cheats, our skulls and the touch glue sit inside this hook at the
    // tick's end (they name this feature), so the sample sees the tick's
    // final ammo, heat and charge, a bottomless-clip refill included.
    b.raw().after(
        &points::GAME_TICK,
        "sample weapon warning conditions",
        move |c| sample(&mut *c.ctx.memory(), rows).map_err(fault),
    );
    // A marked widget's draw is changed at these hooks: it stays the
    // guest's (docs/graphics/native-hud-overlay.md, "The walk in Rust").
    #[cfg(feature = "desktop-runtime")]
    render_bridge::native_hud::walk::add_foreign(|mem, widget| marked(mem, widget).unwrap_or(true));
    b.raw()
        .before(
            &points::HUD_WIDGET_GATE,
            "gate source weapon widgets",
            move |c| {
                let hide = hidden(
                    &*c.ctx.memory(),
                    c.cpu.eax.wrapping_sub(8),
                    c.cpu.ecx,
                    c.cpu.esp,
                    c.cpu.ebp,
                    rows,
                )
                .map_err(fault)?;
                if hide {
                    c.cpu.eax = 0;
                    return c.ret(12).map(Flow::Return);
                }
                Ok(Flow::Continue)
            },
        )
        .context_only();
    b.raw()
        .before(
            &points::HUD_WIDGET_INPUTS,
            "read source weapon inputs",
            |c| {
                let supplied = supply(&mut *c.ctx.memory(), c.cpu.esp, c.cpu.ecx).map_err(fault)?;
                if supplied {
                    return c.ret(12).map(Flow::Return);
                }
                Ok(Flow::Continue)
            },
        )
        .context_only();
    b.raw()
        .before(
            &points::HUD_WIDGET_STRING,
            "format source range and elevation",
            |c| {
                let formatted =
                    format_text(&mut *c.ctx.memory(), c.cpu.esp, c.cpu.ebx).map_err(fault)?;
                if formatted {
                    return c.ret(16).map(Flow::Return);
                }
                Ok(Flow::Continue)
            },
        )
        .context_only();
    b.raw()
        .before(
            &points::HUD_WIDGET_EFFECTS,
            "scale split-screen crosshairs",
            |c| {
                split_basis(&mut *c.ctx.memory(), c.cpu.esp, c.cpu.ebp).map_err(fault)?;
                Ok(Flow::Continue)
            },
        )
        .context_only();
    b.raw()
        .before(
            &points::HUD_WIDGET_SPRITE,
            "select the source crosshair frame",
            move |c| {
                let selected = select_sprite(
                    &mut *c.ctx.memory(),
                    c.cpu.esp,
                    c.cpu.ebp,
                    c.cpu.ecx,
                    c.cpu.eax,
                    rows,
                )
                .map_err(fault)?;
                if selected {
                    return c.ret(16).map(Flow::Return);
                }
                Ok(Flow::Continue)
            },
        )
        .context_only();
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn warning_conditions_use_the_owning_weapon_and_local_inputs() {
        let mut w = Weapon::default();
        let low = [30, 10, 75, 25];
        w.magazines[0].reserve = 90;
        w.magazines[0].loaded = 10;
        assert!(w.warning(3, low));
        assert!(!w.warning(10, low));
        w.magazines[0].reserve = 0;
        assert!(w.warning(10, low));
        assert!(!w.warning(3, low));
        w.magazines[1].reloading = true;
        assert!(w.warning(13, low));
        assert!(!w.warning(7, low));
        w.magazines[0].loaded = 0;
        assert!(!w.warning(8, low));
        w.firing = true;
        assert!(w.warning(8, low));
        w.age = 0.75;
        assert!(w.warning(6, low));
        w.age = 1.0;
        assert!(!w.warning(6, low));
        assert!(w.warning(18, low));
        w.throwing = true;
        assert!(w.warning(9, low));
        w.grenades = true;
        assert!(!w.warning(9, low));
    }

    #[test]
    fn pressed_throw_requires_the_bipeds_idle_grenade_action() {
        let mut mem = FlatMemory::new(0x1000, 0x900);
        let unit = 0x1400;
        for offset in [-0x40_i16, 0x380] {
            mem.set_u16(unit + 0x346, offset as u16).unwrap();
            let state = unit.wrapping_add_signed(i32::from(offset)) + 0xc;
            for action in 0..=3 {
                mem.set_u8(state, action).unwrap();
                assert_eq!(throw_requested(&mem, unit, 1 << 26).unwrap(), action == 0);
                assert!(!throw_requested(&mem, unit, 0).unwrap());
            }
        }
    }

    #[test]
    fn warning_clock_keeps_the_first_active_tick_and_finishes_the_trigger_flash() {
        assert_eq!(clock_start(INACTIVE, true, false, 9, 30), 9);
        assert_eq!(clock_start(9, true, false, 69, 30), 9);
        assert_eq!(clock_start(9, false, false, 10, 30), INACTIVE);
        assert_eq!(clock_start(9, false, true, 38, 30), 9);
        assert_eq!(clock_start(9, false, true, 39, 30), INACTIVE);
    }

    #[test]
    fn inputs_use_the_source_ramp_and_whole_magazines() {
        let mut mem = FlatMemory::new(0x1000, 0x2000);
        let (sp, widget, ws, out) = (0x1100, 0x1200, 0x1300, 0x1400);
        mem.set_u32(sp, 0x0013_a61c).unwrap();
        mem.set_u32(sp + 8, widget + 4).unwrap();
        mem.set_u32(sp + 12, out).unwrap();
        mem.set_u8(widget + 0x1b, input::SCREEN + 2).unwrap();
        mem.write(widget + 4, &[input::METER + 1, 0, 60, 1])
            .unwrap();
        mem.set_u16(ws + 6, 4).unwrap();
        assert!(supply(&mut mem, sp, ws).unwrap());
        assert_eq!(mem.f32(out).unwrap(), 241.0 / 255.0);
        mem.write(widget + 4, &[input::MAGAZINES, 0x30, 2, 1])
            .unwrap();
        mem.set_u16(ws + 6, 29).unwrap();
        mem.set_u16(ws + 0xa, 10).unwrap();
        assert!(supply(&mut mem, sp, ws).unwrap());
        assert_eq!(mem.f32(out).unwrap(), 2.0);
        mem.set_u8(widget + 0x1b, 0).unwrap();
        assert!(!supply(&mut mem, sp, ws).unwrap());
    }

    #[test]
    fn split_size_uses_the_native_axes_and_keeps_the_anchor() {
        let mut mem = FlatMemory::new(0, (points::PLAYER_GLOBALS.base + 4) as usize);
        let (sp, frame, widget) = (0x200, 0x300, 0x400);
        mem.set_u32(points::PLAYER_GLOBALS.base, 0x100).unwrap();
        mem.set_u16(0x108, 2).unwrap();
        mem.set_u32(sp, 0x0022_b19d).unwrap();
        mem.set_u32(frame + 0x78, widget).unwrap();
        mem.set_u8(widget + 0x1b, input::SCREEN).unwrap();
        mem.set_u8(widget + 0x33, 1).unwrap();
        mem.set_f32(frame + 0x48, 91.0).unwrap();
        split_basis(&mut mem, sp, frame).unwrap();
        assert_eq!(mem.f32(frame).unwrap(), 0.5);
        assert_eq!(mem.f32(frame + 0x34).unwrap(), 0.5);
        assert_eq!(mem.f32(frame + 0x48).unwrap(), 91.0);
        mem.set_u8(widget + 0x33, 0).unwrap();
        mem.set_f32(frame, 1.0).unwrap();
        split_basis(&mut mem, sp, frame).unwrap();
        assert_eq!(mem.f32(frame).unwrap(), 1.0);
    }

    #[test]
    fn bitmap_draw_resolves_the_vehicle_snapshot_for_its_actual_user() {
        let mut mem = FlatMemory::new(0x1000, 0x2000);
        let (frame, caller, ws, rows) = (0x1100, 0x1300, 0x1600, 0x1800);
        mem.set_f32(frame, 1.0).unwrap();
        mem.set_u32(frame + 0x68, caller).unwrap();
        mem.set_u32(frame + 0x6c, 0x0013_a62a).unwrap();
        mem.set_u32(caller + 8, 2).unwrap();
        mem.set_u32(caller + 0x10, ws).unwrap();
        mem.set_u32(ws, 0xe001_0002).unwrap();
        let (user, actual) = draw_owner(&mem, frame).unwrap().unwrap();
        assert_eq!((user, actual), (2, ws));
        let row = rows + 5 * ROW;
        mem.set_u32(row, 0xe001_0002).unwrap();
        mem.set_u32(row + 4 + 7 * 4, 123).unwrap();
        assert_eq!(
            warning_start(&mem, rows, user, mem.u32(actual).unwrap(), 7).unwrap(),
            123
        );
        mem.set_u32(frame + 0x6c, 0).unwrap();
        assert_eq!(draw_owner(&mem, frame).unwrap(), None);
    }

    #[test]
    fn sniper_readouts_keep_the_sign_and_authored_precision() {
        assert_eq!(target_text(-12.875, 4, 3, 3), "-0012.875");
        assert_eq!(target_text(3.75, 4, 3, 3), "0003.750");
        assert!(target_text(f32::NAN, 4, 3, 3).is_empty());
    }
}
