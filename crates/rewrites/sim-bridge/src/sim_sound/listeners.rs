//! `0x127400`: the listener update (docs/engine/sound-instances.md,
//! "Entries"). For each of the four local users it fills the user's
//! listener row in the sound globals (`+0x88`, `0x48` bytes a user) from the
//! user's camera: the position, the BSP location and the orientation basis
//! (the third person camera's orientation is the nlerp of the camera's
//! orientation toward the unit's attachment). With its stack argument set it
//! then hands the environment words to `0x220790`.
//!
//! | Guest | Rust |
//! |---|---|
//! | `0x127400` | [`update`] |
//! | `0x155760` | [`camera_mode`] |
//! | `0x11d6a0` | [`rotation_between`] (the degenerate axis is `0x11d000`) |
//! | `0x11d950` | [`nlerp`] |
//! | `0x11d610` | [`normalize`] |
//!
//! Left as guest calls: `0xcaf60` (the unit's attachment position, the
//! object node walk), `0xba1d0` (the unit's ear), `0x11bed0` (the BSP
//! leaf, itself a Rust hook), `0x11d000` (a perpendicular axis, only for
//! antiparallel vectors) and `0x220790` (the environment record's copy).
//! The scalar code is `f32` in the lifted order; the normalisation runs on
//! [`X87`] as the original does.

use guest_layout::entry::{Cx, EntryError, GuestCall};
use guest_layout::{GuestMemory, MemFault};
use sim_core::det::X87;
use sim_sound::instances::FPU_CONTROL;

/// `[0x4e6948]`: the sound manager; byte `+0x1120` is its on flag.
const MANAGER: u32 = 0x004e_6948;
const MANAGER_ON: u32 = 0x1120;
/// `[0x4e8c20] + 0xc`: the four local users' player handles.
const USERS: u32 = 0x004e_8c20;
/// `[0x4e8c24] + 0x44`: the player table (`0x21c` bytes a row, unit at `+0x2c`).
const PLAYERS: u32 = 0x004e_8c24;
const PLAYER_ROW: u32 = 0x21c;
/// `[0x4e0300] + 0x44`: the object table (12 bytes a row, the object at `+8`).
const OBJECTS: u32 = 0x004e_0300;
/// `[0x4e3b44]`: the 16-byte rows of the object definitions (`+8`).
const DEFINITIONS: u32 = 0x004e_3b44;
/// `[0x4e6380]`: the sound globals.
const GLOBALS: u32 = 0x004e_6380;
/// The users' cameras, `0x358` bytes each, the first at `0x4e9c8c`, used
/// when the word at `0x4686c4` is not `0xffff` (else the camera is at 0).
const CAMERA_ROWS: u32 = 0x004e_9c8c;
const CAMERA_ROW: u32 = 0x358;
const CAMERA_SWITCH: u32 = 0x0046_86c4;
/// `[0x4e0348]`: the structure BSP (`+0xa0` leaves of `0xb0`, `+0x68` planes).
const BSP: u32 = 0x004e_0348;
/// `0x45dbc0`, `0x45dc0c`, `0x45dbbc`, `0x45dc70`: 1, the pair of the
/// half-angle form, the angle scale and the smallest sine.
const ONE: u32 = 0x0045_dbc0;
const TWO: u32 = 0x0045_dc0c;
const ANGLE: u32 = 0x0045_dbbc;
const SMALLEST: u32 = 0x0045_dc70;
/// `[0x4687cc]`: the quaternion the third person camera blends toward.
const BLEND_TO: u32 = 0x0046_87cc;
/// The environment's vectors (`0x4687a8`, `0x4687b0`, `0x468788`, `0x4687a4`).
const ENV_VECTORS: [u32; 4] = [0x0046_87a8, 0x0046_87b0, 0x0046_8788, 0x0046_87a4];
/// `0x44a2a0`: the definition the environment names when it has none.
const NO_DEFINITION: u32 = 0x0044_a2a0;

pub const CAMERAS: u32 = 0x004e_8c44;
pub const CAMERA_LEN: u32 = 0x140;
const CAMERA_FIRST_PERSON: u32 = 0x0023_c110;
const CAMERA_FOLLOWING: u32 = 0x0023_cbb0;
const CAMERA_DEAD: u32 = 0x0016_c840;
const CAMERA_FLYING: u32 = 0x0023_d090;
const BLEND_FIRST_PERSON: u32 = 0x0045_dbd8;
const FLYING_MODE: u32 = 0x0051_ec10;

/// The call frame below the entry `ESP`: the scratch the guest calls read
/// and write, and the environment record `0x220790` reads.
const FRAME: u32 = 0x200;
/// Scratch offsets from the frame's base.
const AT_POSITION: u32 = 0x00;
const AT_QUAT: u32 = 0x10;
const AT_LOCATION: u32 = 0x20;
const AT_POINT: u32 = 0x30;
const AT_EAR: u32 = 0x40;
/// The environment record's frame offsets `0x84..0xec` of the original.
const AT_ENV: u32 = 0x80;
const ENV_FIRST: u32 = 0x84;

fn f(m: &dyn GuestMemory, at: u32) -> Result<f32, MemFault> {
    Ok(f32::from_bits(m.u32(at)?))
}

/// `0x155760`: the user's camera mode (0 first person with no blend, 1
/// following, 2 dead or flying with `[0x51ec10]` set, else 3).
pub fn camera_mode(m: &dyn GuestMemory, user: u32) -> Result<u32, MemFault> {
    let camera = CAMERAS.wrapping_add(user.wrapping_mul(CAMERA_LEN));
    Ok(match m.u32(camera.wrapping_add(0xc))? {
        CAMERA_FIRST_PERSON => {
            if f(m, camera.wrapping_add(4))? == f(m, BLEND_FIRST_PERSON)? { 0 } else { 3 }
        }
        CAMERA_FOLLOWING => 1,
        CAMERA_DEAD => 2,
        CAMERA_FLYING if m.u8(FLYING_MODE)? != 0 => 2,
        _ => 3,
    })
}

/// The constants the quaternion code reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constants {
    pub one: f32,
    pub two: f32,
    pub angle: f32,
    pub smallest: f32,
}

impl Constants {
    fn read(m: &dyn GuestMemory) -> Result<Self, MemFault> {
        Ok(Self { one: f(m, ONE)?, two: f(m, TWO)?, angle: f(m, ANGLE)?, smallest: f(m, SMALLEST)? })
    }
}

/// `0x11d6a0(a, b)` unless the half-angle sine is under the smallest: the
/// quaternion (axis, scale) turning `a` toward `b`. `None` is the guest's
/// degenerate branch (`0x11d000` for the axis, the fourth word 0).
pub fn rotation_between(k: Constants, a: [f32; 3], b: [f32; 3]) -> Option<[f32; 4]> {
    let mut t = a[1] * b[1];
    t += a[0] * b[0];
    t += b[2] * a[2];
    t += k.one;
    t *= k.two;
    let squared = if t > 0.0 { t } else { 0.0 };
    let sine = squared.sqrt();
    if !(sine >= k.smallest) {
        return None;
    }
    let z = a[0] * b[1] - a[1] * b[0];
    let y = b[0] * a[2] - b[2] * a[0];
    let scale = k.one / sine;
    let w = sine * k.angle;
    let x = (a[1] * b[2] - a[2] * b[1]) * scale;
    Some([x, y * scale, z * scale, w])
}

/// `0x11d950(q, to, t)` in place on `q` (before the normalisation): the
/// blend toward `to` by `t`, the shorter way round.
pub fn nlerp(q: [f32; 4], to: [f32; 4], t: f32, one: f32) -> [f32; 4] {
    let mut dot = q[2] * to[2];
    dot += q[1] * to[1];
    dot += to[3] * q[3];
    dot += to[0] * q[0];
    let from = one - t;
    let t = if 0.0 > dot { 0.0 - t } else { t };
    [
        to[0] * from + q[0] * t,
        q[1] * t + from * to[1],
        q[2] * t + from * to[2],
        to[3] * from + t * q[3],
    ]
}

/// `0x11d610`: the quaternion over its length, on the x87 as the original.
pub fn normalize(q: [f32; 4], one: f32) -> [f32; 4] {
    let mut sum = q[0] * q[0];
    sum += q[1] * q[1];
    sum += q[2] * q[2];
    sum += q[3] * q[3];
    if !(sum > 0.0) {
        return q;
    }
    let x = X87::new(FPU_CONTROL);
    let inverse = x.div(x.load32(one.to_bits()), x.sqrt(x.load32(sum.to_bits())));
    q.map(|c| f32::from_bits(x.store32(x.mul(inverse, x.load32(c.to_bits())))))
}

/// The camera basis a listener takes: its two axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Basis {
    pub forward: [f32; 3],
    pub up: [f32; 3],
}

/// The camera's own axes (`+0x20`, `+0x2c`) turned by the quaternion
/// (`0x1275d8` to `0x1277da`).
pub fn rotate(k: Constants, q: [f32; 4], n: [f32; 3], c: [f32; 3]) -> Basis {
    let (q0, q1, q2, q3) = (q[0], q[1], q[2], q[3]);
    let ratio = q3 * q3 * k.two - k.one;
    let w2 = q3 * k.two;
    let d = (n[1] * q1 + n[2] * q2) + q0 * n[0];
    let d2 = d * k.two;
    let a0 = (n[2] * q1 - n[1] * q2) * w2 + q0 * d2 + ratio * n[0];
    let a1 = (q2 * n[0] - n[2] * q0) * w2 + q1 * d2 + n[1] * ratio;
    let a2 = (n[1] * q0 - q1 * n[0]) * w2 + q2 * d2 + n[2] * ratio;
    let e = ((c[1] * q1 + c[2] * q2) + c[0] * q0) * k.two;
    let b0 = (c[2] * q1 - c[1] * q2) * w2 + e * q0 + c[0] * ratio;
    let b1 = (c[0] * q2 - c[2] * q0) * w2 + q1 * e + c[1] * ratio;
    let b2 = (c[1] * q0 - c[0] * q1) * w2 + q2 * e + c[2] * ratio;
    Basis { forward: [a0, a1, a2], up: [b0, b1, b2] }
}

/// A listener row's words after `+8`, from the basis (`0x127917` on):
/// the scale 1, the forward axis, `up x forward`, the up axis, and the
/// ear's projections on them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    pub forward: [f32; 3],
    pub cross: [f32; 3],
    pub up: [f32; 3],
    pub position: [f32; 3],
    pub ear: [f32; 3],
}

pub fn row(basis: Basis, position: [f32; 3], ear: [f32; 3]) -> Row {
    let (a, b) = (basis.forward, basis.up);
    let cross = [b[1] * a[2] - b[2] * a[1], b[2] * a[0] - a[2] * b[0], a[1] * b[0] - b[1] * a[0]];
    let p0 = (a[1] * ear[1] + a[2] * ear[2]) + ear[0] * a[0];
    let p1 = (cross[1] * ear[1] + cross[0] * ear[0]) + cross[2] * ear[2];
    let p2 = (b[1] * ear[1] + b[2] * ear[2]) + b[0] * ear[0];
    Row { forward: a, cross, up: b, position, ear: [p0, p1, p2] }
}

/// The BSP's plane test (`0x12793e` to `0x12799f`): whether the point is
/// behind the leaf's plane, the listener row's byte `+7`.
fn behind(m: &dyn GuestMemory, leaf: u16, p: [f32; 3]) -> Result<bool, MemFault> {
    if leaf == 0xffff {
        return Ok(false);
    }
    let bsp = m.u32(BSP)?;
    let leaves = m.u32(bsp.wrapping_add(0xa0))?;
    let at = leaves.wrapping_add((leaf as i16 as i32 as u32).wrapping_mul(0xb0));
    let byte = m.u8(at.wrapping_add(0x70))?;
    if byte == 0xff {
        return Ok(false);
    }
    let planes = m.u32(bsp.wrapping_add(0x68))?;
    let plane = planes.wrapping_add(u32::from(byte & 0x7f) * 24);
    if m.u16(plane.wrapping_add(2))? == 0xffff {
        return Ok(false);
    }
    if byte & 0x80 == 0 {
        return Ok(true);
    }
    let s = f(m, plane.wrapping_add(0xc))? * p[2] + f(m, plane.wrapping_add(8))? * p[1];
    let s = s + f(m, plane.wrapping_add(4))? * p[0];
    Ok(s - f(m, plane.wrapping_add(0x10))? < 0.0)
}

/// One user's inputs once the camera and the unit are read.
struct Pose {
    position: [f32; 3],
    location: [u32; 2],
    ear: [f32; 3],
    basis: Basis,
}

fn vec3(m: &dyn GuestMemory, at: u32) -> Result<[f32; 3], MemFault> {
    Ok([f(m, at)?, f(m, at.wrapping_add(4))?, f(m, at.wrapping_add(8))?])
}

fn put3(m: &mut dyn GuestMemory, at: u32, v: [f32; 3]) -> Result<(), MemFault> {
    for (i, c) in v.iter().enumerate() {
        m.set_u32(at.wrapping_add(4 * i as u32), c.to_bits())?;
    }
    Ok(())
}

/// The pose a camera alone gives: its position, location, ear and axes
/// (the paths without a unit, or with the first person camera).
fn from_camera(m: &dyn GuestMemory, camera: u32) -> Result<Pose, MemFault> {
    let at = |o: u32| camera.wrapping_add(o);
    Ok(Pose {
        position: vec3(m, camera)?,
        location: [m.u32(at(0xc))?, m.u32(at(0x10))?],
        ear: vec3(m, at(0x14))?,
        basis: Basis { forward: vec3(m, at(0x20))?, up: vec3(m, at(0x2c))? },
    })
}

/// The listener row `user` of the globals.
fn listener(globals: u32, user: u32) -> u32 {
    globals.wrapping_add(0x88).wrapping_add(user.wrapping_mul(0x48))
}

fn write_row(m: &mut dyn GuestMemory, at: u32, pose: &Pose, behind: bool, one: f32) -> Result<(), MemFault> {
    let row = row(pose.basis, pose.position, pose.ear);
    m.set_u8(at.wrapping_add(6), 1)?;
    m.set_u8(at.wrapping_add(7), u8::from(behind))?;
    m.set_u32(at.wrapping_add(8), one.to_bits())?;
    put3(m, at.wrapping_add(0xc), row.forward)?;
    put3(m, at.wrapping_add(0x18), row.cross)?;
    put3(m, at.wrapping_add(0x24), row.up)?;
    put3(m, at.wrapping_add(0x30), row.position)?;
    put3(m, at.wrapping_add(0x3c), row.ear)?;
    m.set_u32(at, pose.location[0])?;
    m.set_u16(at.wrapping_add(4), pose.location[1] as u16)
}

/// `0x127400(update_environment)`.
pub async fn update(cx: &Cx, update_environment: bool) -> Result<(), EntryError> {
    let on = cx.read(|m| -> Result<bool, MemFault> {
        let manager = m.u32(MANAGER)?;
        Ok(manager != 0 && m.u8(manager.wrapping_add(MANAGER_ON))? != 0)
    })?;
    if !on {
        return Ok(());
    }
    let base = cx.entry().esp.wrapping_sub(FRAME);
    for user in 0..4u32 {
        step(cx, base, user).await?;
    }
    if update_environment {
        environment(cx, base).await?;
    }
    Ok(())
}

/// What the first read of a user finds.
enum Seat {
    /// No player: the row holds -1 and `0xffff`.
    Empty,
    /// A player with no unit in a mode that follows one: the camera alone.
    Camera { camera: u32 },
    /// A player on a unit (`unit`, the attachment `node`, the object `obj`).
    Unit { camera: u32, unit: u32, obj: u32, node: u32 },
}

async fn step(cx: &Cx, base: u32, user: u32) -> Result<(), EntryError> {
    let k = cx.read(Constants::read)?;
    let (seat, listener_at) = cx.read(|m| -> Result<(Seat, u32), MemFault> {
        let globals = m.u32(GLOBALS)?;
        let at = listener(globals, user);
        let handle = m.u32(m.u32(USERS)?.wrapping_add(0xc + 4 * user))?;
        if handle == u32::MAX {
            return Ok((Seat::Empty, at));
        }
        let camera = if m.u16(CAMERA_SWITCH)? != 0xffff {
            CAMERA_ROWS.wrapping_add(user.wrapping_mul(CAMERA_ROW))
        } else {
            0
        };
        let players = m.u32(m.u32(PLAYERS)?.wrapping_add(0x44))?;
        let unit = m.u32(players.wrapping_add((handle & 0xffff).wrapping_mul(PLAYER_ROW)).wrapping_add(0x2c))?;
        let mode = camera_mode(m, user)?;
        if unit == u32::MAX || (1u32 << mode) & 3 == 0 {
            return Ok((Seat::Camera { camera }, at));
        }
        let objects = m.u32(m.u32(OBJECTS)?.wrapping_add(0x44))?;
        let obj = m.u32(objects.wrapping_add((unit & 0xffff).wrapping_mul(12)).wrapping_add(8))?;
        let parent = m.u32(obj.wrapping_add(0x14))?;
        let node = m.u16(obj.wrapping_add(0x1fc))?;
        if parent == u32::MAX || node == 0xffff || mode == 0 {
            return Ok((Seat::Camera { camera }, at));
        }
        let parent_obj = m.u32(objects.wrapping_add((parent & 0xffff).wrapping_mul(12)).wrapping_add(8))?;
        let definition = m.u32(m.u32(DEFINITIONS)?.wrapping_add((m.u32(parent_obj)? & 0xffff) << 4).wrapping_add(8))?;
        let nodes = m.u32(definition.wrapping_add(0x1cc))?;
        let node_at = nodes.wrapping_add((node as i16 as i32 as u32).wrapping_mul(0xb0));
        Ok((Seat::Unit { camera, unit, obj, node: node_at }, at))
    })?;
    let pose = match seat {
        Seat::Empty => {
            return cx.write(|m| {
                m.set_u32(listener_at, u32::MAX)?;
                m.set_u16(listener_at.wrapping_add(4), 0xffff)
            }).map_err(Into::into);
        }
        Seat::Camera { camera } => cx.read(|m| from_camera(m, camera))?,
        Seat::Unit { camera, unit, obj, node } => {
            cx.call(GuestCall::new(0xcaf60).eax(unit).esi(base.wrapping_add(AT_POSITION)).reserve(FRAME)).await?;
            let (blend, quat) = cx.write(|m| -> Result<(f32, Option<[f32; 4]>), MemFault> {
                let blend = f(m, node.wrapping_add(0x40))?;
                let moved = vec3(m, base.wrapping_add(AT_POSITION))?;
                let c = vec3(m, camera)?;
                let lerp = |i: usize| (moved[i] - c[i]) * blend + c[i];
                put3(m, base.wrapping_add(AT_POINT), [lerp(0), lerp(1), lerp(2)])?;
                let n = vec3(m, camera.wrapping_add(0x20))?;
                let from = vec3(m, obj.wrapping_add(0x180))?;
                Ok((blend, rotation_between(Constants::read(m)?, n, from)))
            })?;
            let q = match quat {
                Some(q) => q,
                None => {
                    // The degenerate axis (`0x11d000`), the fourth word 0.
                    let quat_at = base.wrapping_add(AT_QUAT);
                    cx.call(GuestCall::new(0x11d000).eax(quat_at).ecx(camera.wrapping_add(0x20)).reserve(FRAME)).await?;
                    cx.write(|m| m.set_u32(quat_at.wrapping_add(0xc), 0))?;
                    cx.read(|m| -> Result<[f32; 4], MemFault> {
                        let v = vec3(m, quat_at)?;
                        Ok([v[0], v[1], v[2], 0.0])
                    })?
                }
            };
            let (basis, p) = cx.read(|m| -> Result<_, MemFault> {
                let to = m.u32(BLEND_TO)?;
                let to = [f(m, to)?, f(m, to.wrapping_add(4))?, f(m, to.wrapping_add(8))?, f(m, to.wrapping_add(12))?];
                let q = normalize(nlerp(q, to, blend, k.one), k.one);
                let basis = rotate(k, q, vec3(m, camera.wrapping_add(0x20))?, vec3(m, camera.wrapping_add(0x2c))?);
                Ok((basis, vec3(m, base.wrapping_add(AT_POINT))?))
            })?;
            let ear = base.wrapping_add(AT_EAR);
            cx.call(GuestCall::new(0xba1d0).eax(unit).ecx(ear).args([0]).reserve(FRAME)).await?;
            cx.call(GuestCall::new(0x11bed0).edx(base.wrapping_add(AT_POINT)).esi(base.wrapping_add(AT_LOCATION)).reserve(FRAME)).await?;
            cx.read(|m| -> Result<Pose, MemFault> {
                let at = base.wrapping_add(AT_LOCATION);
                let (leaf, cluster) = (m.u32(at)?, m.u32(at.wrapping_add(4))?);
                let location = if cluster as u16 == 0xffff {
                    [m.u32(camera.wrapping_add(0xc))?, m.u32(camera.wrapping_add(0x10))?]
                } else {
                    [leaf, cluster]
                };
                Ok(Pose { position: p, location, ear: vec3(m, ear)?, basis })
            })?
        }
    };
    cx.write(|m| {
        let plane = behind(m, pose.location[1] as u16, pose.position)?;
        write_row(m, listener_at, &pose, plane, k.one)
    })?;
    Ok(())
}

/// The environment words to `0x220790` (`0x127b53` on).
async fn environment(cx: &Cx, base: u32) -> Result<(), EntryError> {
    let record = |o: u32| base.wrapping_add(AT_ENV).wrapping_add(o - ENV_FIRST);
    let arg = cx.write(|m| -> Result<u32, MemFault> {
        let globals = m.u32(GLOBALS)?;
        let definition = |m: &dyn GuestMemory, word: u32| -> Result<u32, MemFault> {
            let id = m.u32(globals.wrapping_add(word))?;
            if id == u32::MAX {
                return Ok(NO_DEFINITION);
            }
            m.u32(m.u32(DEFINITIONS)?.wrapping_add((id & 0xffff) << 4).wrapping_add(8))
        };
        let copy = |m: &mut dyn GuestMemory, from: u32, to: u32, words: u32| -> Result<(), MemFault> {
            for i in 0..words {
                let v = m.u32(from.wrapping_add(4 * i))?;
                m.set_u32(to.wrapping_add(4 * i), v)?;
            }
            Ok(())
        };
        let first = definition(m, 0x1b0)?;
        m.set_u32(record(0x84), first)?;
        copy(m, globals.wrapping_add(0x1b4), record(0x88), 4)?;
        let v = m.u32(globals.wrapping_add(0x1ac))?;
        m.set_u32(record(0x98), v)?;
        let second = definition(m, 0x1cc)?;
        m.set_u32(record(0x9c), second)?;
        copy(m, globals.wrapping_add(0x1d0), record(0xa0), 4)?;
        let v = m.u32(globals.wrapping_add(0x1c8))?;
        m.set_u32(record(0xb0), v)?;
        for (word, o) in ENV_VECTORS.iter().zip([0xc0u32, 0xcc, 0xb4, 0xd8]) {
            let at = m.u32(*word)?;
            copy(m, at, record(o), 3)?;
        }
        m.set_u32(record(0xe4), 2)?;
        m.set_u32(record(0xe8), record(0x84))?;
        Ok(record(0xb4))
    })?;
    cx.call(GuestCall::new(0x220790).edx(record(0x84)).args([arg]).reserve(FRAME)).await?;
    Ok(())
}

#[cfg(test)]
mod tests;
