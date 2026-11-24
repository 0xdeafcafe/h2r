//! The main view's records built in Rust (docs/graphics/native-visibility.md,
//! "The view records"): the render camera from the observer camera
//! (`0x00223080` and `0x0002f800`), the projection's scales and offsets
//! (`0x0002fb70`), the start cluster from the camera's position
//! (`0x0011bed0`, the walk `0x0014a280`), and the mode (`0x00132b80`). Each
//! is compared with what the guest wrote, field by field and bit for bit;
//! the scene step gives the visibility the Rust camera only when every
//! field equals the guest's.
//!
//! Every value the guest reads from its own data (the near and far planes,
//! the field-of-view factors, the lens constants, the mode's radius) is
//! read from the running image each frame, never committed.

use std::fmt;

use frame_scene::RenderCamera;

use super::read::{at, f32_at, ptr_at, u8_at, u16_at, u32_at, vec3_at};
use crate::GuestRead;

/// The view index `render_game_view` stores (`0x0002b7bd`), and the view
/// kind in the visibility struct.
pub use engine::render_loop::occlusion::guest::{VIEW_INDEX, VIS_KIND};
/// The view records `0x002234e0` fills, one `0x00223080` each.
pub const VIEW_RECORDS: u32 = 0x0047_f210;
pub const VIEW_RECORD_SIZE: u32 = 0x118;
/// In a record: `+0` nonzero for a cinematic view, `+8` the local player
/// (`-1` for none), `+0xc` the render camera (the layout of
/// `crate::layout::camera`).
pub mod record {
    pub const CINEMATIC: u32 = 0x0;
    pub const PLAYER: u32 = 0x8;
    pub const CAMERA: u32 = 0xc;
    /// The camera's viewport (`i16` top, left, bottom, right).
    pub const VIEWPORT: u32 = CAMERA + 0x30;
}
/// The observer cameras, one per local player.
pub const OBSERVER_CAMERAS: u32 = 0x004e_9c8c;
pub const OBSERVER_CAMERA_SIZE: u32 = 0x358;
/// In an observer camera.
pub mod observer {
    pub const POSITION: u32 = 0x0;
    pub const FORWARD: u32 = 0x20;
    pub const UP: u32 = 0x2c;
    pub const FOV: u32 = 0x4c;
}
/// The active structure BSP index (`0xffff`: none).
pub const ACTIVE_BSP_INDEX: u32 = 0x0046_86c4;
/// The camera override `0x00223080` and `0x0002fb70` test.
pub const CAMERA_OVERRIDE: u32 = 0x004e_6948;
/// The near and far planes `0x0002f800` stores.
pub const NEAR: u32 = 0x0048_5ad4;
pub const FAR: u32 = 0x0048_5ad8;
const PLAYER_GLOBALS: u32 = 0x004e_8c20;
/// The whole screen's viewport (`i16` top, left, bottom, right).
pub const SCREEN: u32 = 0x0048_5a8a;
/// `0x0002f800`'s field-of-view factors: by default, and for a view whose
/// shape ratio is 200 or 50 (two views split one way or the other).
pub const FOV_FACTOR: u32 = 0x0045_083c;
pub const FOV_FACTOR_200: u32 = 0x0045_dbbc;
pub const FOV_FACTOR_50: u32 = 0x0044_ae88;
/// `0x0002fb70`'s constants and switches.
pub mod lens {
    /// The default extents: `[min, one, min, one]`.
    pub const EXTENT_MIN: u32 = 0x0045_dbcc;
    pub const ONE: u32 = 0x0045_dbc0;
    pub const HALF: u32 = 0x0045_dbbc;
    pub const OFFSET_SCALE: u32 = 0x0045_dbc4;
    /// The anamorphic aspect factor and the tangent factor it takes with
    /// the display's flag.
    pub const ANAMORPHIC: u32 = 0x0045_dfdc;
    pub const ANAMORPHIC_TAN: u32 = 0x0045_e108;
    /// The widescreen byte.
    pub const WIDESCREEN: u32 = 0x0048_5ac2;
    /// The display settings pointer; its byte `+5` is the flag.
    pub const DISPLAY: u32 = 0x0051_0c50;
}
/// The structure's collision BSP the location walks.
pub const STRUCTURE_BSP: u32 = 0x004e_033c;
/// In the active structure: the leaf table (count, pointer), 8-byte leaves
/// whose `i16` at `+0` is the cluster.
pub const LEAF_COUNT: u32 = 0x2c;
pub const LEAVES: u32 = 0x30;
/// The radius under which `0x00132b80` may choose the sphere mode.
pub const MODE_RADIUS: u32 = 0x0045_dbd4;

/// In the visibility struct: the sphere radius `0x00132b80` stores.
pub const VIS_RADIUS: u32 = super::guest::VIS_SPHERE_RADIUS;

/// Why no record was built. The guest's stay in use, and `render.scene`
/// counts the reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    Unread,
    /// A count, index or link outside the guest's arrays.
    Layout,
    /// A cinematic view: its camera is the cinematic's.
    Cinematic,
    /// No local player or no structure: `0x0002f800`'s default camera.
    NoObserver,
    /// The camera override is active.
    Override,
}

impl Decline {
    pub const ALL: [Decline; 5] = [
        Self::Unread,
        Self::Layout,
        Self::Cinematic,
        Self::NoObserver,
        Self::Override,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::Layout => "layout",
            Self::Cinematic => "cinematic",
            Self::NoObserver => "no_observer",
            Self::Override => "override",
        }
    }
}

/// A view's records as Rust builds them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewRecord {
    pub camera: RenderCamera,
    /// The start cluster, `0xffff` outside the structure.
    pub start: u16,
    /// 0 portal flood, 1 sphere, 2 the potentially visible set.
    pub mode: u32,
}

/// The field a record differs in first, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Position,
    Forward,
    Up,
    Fov,
    Viewport,
    Near,
    Far,
    Projection,
    Start,
    Mode,
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Position => "position",
            Self::Forward => "forward",
            Self::Up => "up",
            Self::Fov => "fov",
            Self::Viewport => "viewport",
            Self::Near => "near",
            Self::Far => "far",
            Self::Projection => "projection",
            Self::Start => "start cluster",
            Self::Mode => "mode",
        })
    }
}

/// How a Rust record compares with the guest's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecordParity {
    /// Fields that differ.
    pub differ: u32,
    pub first: Option<Field>,
}

impl RecordParity {
    pub fn holds(&self) -> bool {
        self.first.is_none()
    }
}

fn bits3(v: [f32; 3]) -> [u32; 3] {
    v.map(f32::to_bits)
}

/// Compares bit for bit (a NaN equals the same NaN; `-0` is not `+0`).
pub fn compare(rust: &ViewRecord, guest: &ViewRecord) -> RecordParity {
    let (r, g) = (&rust.camera, &guest.camera);
    let fields = [
        (Field::Position, bits3(r.position) == bits3(g.position)),
        (Field::Forward, bits3(r.forward) == bits3(g.forward)),
        (Field::Up, bits3(r.up) == bits3(g.up)),
        (Field::Fov, r.fov.to_bits() == g.fov.to_bits()),
        (Field::Viewport, r.viewport == g.viewport),
        (Field::Near, r.near.to_bits() == g.near.to_bits()),
        (Field::Far, r.far.to_bits() == g.far.to_bits()),
        (
            Field::Projection,
            r.projection_xy.map(|p| p.map(f32::to_bits))
                == g.projection_xy.map(|p| p.map(f32::to_bits)),
        ),
        (Field::Start, rust.start == guest.start),
        (Field::Mode, rust.mode == guest.mode),
    ];
    let mut p = RecordParity::default();
    for (field, equal) in fields {
        if !equal {
            p.differ += 1;
            p.first.get_or_insert(field);
        }
    }
    p
}

fn half_at(mem: &dyn GuestRead, addr: u32) -> Result<i16, Decline> {
    u16_at(mem, addr).map(|h| h as i16).ok_or(Decline::Unread)
}

fn rect_at(mem: &dyn GuestRead, addr: u32) -> Result<[i16; 4], Decline> {
    let mut r = [0; 4];
    for (k, v) in r.iter_mut().enumerate() {
        *v = half_at(mem, at(addr, 2 * k as u32).ok_or(Decline::Layout)?)?;
    }
    Ok(r)
}

fn f32_read(mem: &dyn GuestRead, addr: u32) -> Result<f32, Decline> {
    f32_at(mem, addr).ok_or(Decline::Unread)
}

/// Whether the camera override holds (`0x00223080`'s test, less its last
/// call `0x0014a250`, which only narrows it).
fn override_active(mem: &dyn GuestRead) -> Result<bool, Decline> {
    let Some(o) = ptr_at(mem, CAMERA_OVERRIDE) else {
        return Ok(false);
    };
    let on = u8_at(mem, at(o, 0x1120).ok_or(Decline::Layout)?).ok_or(Decline::Unread)?;
    let kind = u32_at(mem, at(o, 8).ok_or(Decline::Layout)?).ok_or(Decline::Unread)?;
    Ok(on != 0 && kind == 3)
}

/// The camera viewport uses the outer screen at its edges and splits the
/// inner rectangle (SCREEN + 8) at whole-cell boundaries (0x0002f6b0).
/// The FOV ratio uses cell shares, before any rounding to pixels.
fn viewport_layout(
    mem: &dyn GuestRead,
    index: u32,
    lens: &LensInputs,
) -> Result<([i16; 4], i32), Decline> {
    let count = if lens.display_flag {
        1
    } else {
        let globals = ptr_at(mem, PLAYER_GLOBALS).ok_or(Decline::Unread)?;
        i32::from(half_at(mem, at(globals, 8).ok_or(Decline::Layout)?)?).clamp(1, 4)
    };
    if index >= count as u32 {
        return Err(Decline::Layout);
    }
    let index = index as i32;
    let stacked = lens.override_lens || !lens.widescreen;
    let (cols, rows) = match count {
        1 => (1, 1),
        2 if stacked => (1, 2),
        2 => (2, 1),
        _ => (2, 2),
    };
    // Three players share four cells. View zero spans two; the other
    // players occupy the remaining column or row (0x0002f640).
    let (col, row, width, height) = if count == 3 && stacked {
        if index == 0 {
            (0, 0, 2, 1)
        } else {
            (index - 1, 1, 1, 1)
        }
    } else if count == 3 {
        if index == 0 {
            (0, 0, 1, 2)
        } else {
            (1, index - 1, 1, 1)
        }
    } else {
        (index % cols, index / cols, 1, 1)
    };
    let outer = rect_at(mem, SCREEN)?;
    let [top, left, bottom, right] = rect_at(mem, SCREEN + 8)?.map(i32::from);
    let cell_width = (right - left) / cols;
    let cell_height = (bottom - top) / rows;
    let viewport = [
        if row == 0 { outer[0] } else { (top + row * cell_height) as i16 },
        if col == 0 { outer[1] } else { (left + col * cell_width) as i16 },
        if row + height == rows { outer[2] } else { (top + (row + height) * cell_height) as i16 },
        if col + width == cols { outer[3] } else { (left + (col + width) * cell_width) as i16 },
    ];
    Ok((viewport, width * rows * 100 / (height * cols)))
}

fn fov_factor(mem: &dyn GuestRead, ratio: i32) -> Result<f32, Decline> {
    f32_read(
        mem,
        match ratio {
            200 => FOV_FACTOR_200,
            50 => FOV_FACTOR_50,
            _ => FOV_FACTOR,
        },
    )
}

/// What `0x0002fb70` reads besides the camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LensInputs {
    pub extent_min: f32,
    pub one: f32,
    pub half: f32,
    pub offset_scale: f32,
    pub anamorphic: f32,
    pub anamorphic_tan: f32,
    pub widescreen: bool,
    pub display_flag: bool,
    /// The override's own test: a set byte `+1`, a word `+2` not `0xffff`
    /// and a kind `+8` of 3 skip the widescreen aspect.
    pub override_lens: bool,
}

impl LensInputs {
    pub fn read(mem: &dyn GuestRead) -> Result<Self, Decline> {
        let u = Decline::Unread;
        let override_lens = match ptr_at(mem, CAMERA_OVERRIDE) {
            Some(o) => {
                u8_at(mem, at(o, 1).ok_or(Decline::Layout)?).ok_or(u)? != 0
                    && u16_at(mem, at(o, 2).ok_or(Decline::Layout)?).ok_or(u)? != u16::MAX
                    && u32_at(mem, at(o, 8).ok_or(Decline::Layout)?).ok_or(u)? == 3
            }
            None => false,
        };
        let display_flag = match ptr_at(mem, lens::DISPLAY) {
            Some(d) => u8_at(mem, at(d, 5).ok_or(Decline::Layout)?).ok_or(u)? != 0,
            None => false,
        };
        Ok(Self {
            extent_min: f32_read(mem, lens::EXTENT_MIN)?,
            one: f32_read(mem, lens::ONE)?,
            half: f32_read(mem, lens::HALF)?,
            offset_scale: f32_read(mem, lens::OFFSET_SCALE)?,
            anamorphic: f32_read(mem, lens::ANAMORPHIC)?,
            anamorphic_tan: f32_read(mem, lens::ANAMORPHIC_TAN)?,
            widescreen: u8_at(mem, lens::WIDESCREEN).ok_or(u)? != 0,
            display_flag,
            override_lens,
        })
    }
}

/// `0x0002fb70` with the default extents: the projection's x and y scales
/// and its two offset terms as `0x0002fd90` stores them (`+0x78`, `+0x8c`,
/// `+0x98`, `+0x9c`), in the guest's single-precision order. The tangent
/// is the x87's (`fptan` of `fov · half`), rounded once to single.
pub fn projection_terms(fov: f32, viewport: [i16; 4], k: &LensInputs) -> [f32; 4] {
    let [top, left, bottom, right] = viewport.map(i32::from);
    let width = (right - left) as f32;
    let height = (bottom - top) as f32;
    let e = [k.extent_min, k.one, k.extent_min, k.one];
    let xr = (e[1] - e[0]) * k.half;
    let yr = (e[3] - e[2]) * k.half;
    let ox = (e[1] + e[0]) / xr * k.offset_scale;
    let oy = (e[2] + e[3]) / yr * k.offset_scale;
    let tan = (f64::from(fov) * f64::from(k.half)).tan() as f32;
    let mut aspect = width / height;
    let mut t = tan;
    if !k.override_lens && k.widescreen {
        aspect *= k.anamorphic;
        if k.display_flag {
            t = tan * k.anamorphic * k.anamorphic_tan;
        }
    }
    let sx = k.one / ((xr * aspect) * t);
    let sy = k.one / (yr * t);
    [sx, sy, 0.0 - ox, 0.0 - oy]
}

/// `0x0011bed0` over `0x0014a280`: the cluster holding `p`. The walk from
/// node 0 takes the front child when `((nz·z + ny·y) + nx·x) − d ≥ 0`; a
/// leaf's cluster is the structure's leaf table's. Solid, or no structure,
/// is `0xffff`.
pub fn locate(mem: &dyn GuestRead, p: [f32; 3]) -> Result<u16, Decline> {
    const LEAF: u32 = 0x0080_0000;
    const MAX_DEPTH: usize = 256;
    let (u, l) = (Decline::Unread, Decline::Layout);
    if u16_at(mem, ACTIVE_BSP_INDEX).ok_or(u)? == u16::MAX {
        return Ok(u16::MAX);
    }
    let bsp = ptr_at(mem, STRUCTURE_BSP).ok_or(u)?;
    let word = |o: u32| u32_at(mem, at(bsp, o).ok_or(l)?).ok_or(u);
    let (node_count, nodes) = (word(0)?, word(4)?);
    let (plane_count, planes) = (word(8)?, word(0xc)?);
    let mut index = 0u32;
    for _ in 0..MAX_DEPTH {
        if index >= node_count {
            return Err(l);
        }
        let node = at(nodes, 8 * index).ok_or(l)?;
        let bytes: Vec<u8> = (0..2)
            .map(|w| u32_at(mem, at(node, 4 * w).ok_or(l)?).ok_or(u))
            .collect::<Result<Vec<u32>, Decline>>()?
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        let plane = i16::from_le_bytes([bytes[0], bytes[1]]);
        if plane < 0 || plane as u32 >= plane_count {
            return Err(l);
        }
        let at_plane = at(planes, 16 * plane as u32).ok_or(l)?;
        let [nx, ny, nz] = vec3_at(mem, at_plane).ok_or(u)?;
        let d = f32_at(mem, at(at_plane, 12).ok_or(l)?).ok_or(u)?;
        let distance = ((nz * p[2] + ny * p[1]) + nx * p[0]) - d;
        if distance.is_nan() {
            return Err(l);
        }
        let side = if distance >= 0.0 { 5 } else { 2 };
        let raw = u32::from(bytes[side])
            | u32::from(bytes[side + 1]) << 8
            | u32::from(bytes[side + 2]) << 16;
        let child = ((raw << 8) as i32 >> 8) as u32;
        if child & LEAF == 0 {
            index = child;
            continue;
        }
        if child == u32::MAX {
            return Ok(u16::MAX);
        }
        let leaf = child & 0x7f_ffff;
        let structure = ptr_at(mem, super::guest::ACTIVE_STRUCTURE).ok_or(u)?;
        let leaves = u32_at(mem, at(structure, LEAF_COUNT).ok_or(l)?).ok_or(u)?;
        if leaf >= leaves {
            return Err(l);
        }
        let table = ptr_at(mem, at(structure, LEAVES).ok_or(l)?).ok_or(u)?;
        return u16_at(mem, at(table, 8 * leaf).ok_or(l)?).ok_or(u);
    }
    Err(l)
}

/// `0x00132b80`'s choice of mode: 2 for a view of kind 0 under flag `0x80`;
/// 1 with no frusta, or with a radius under `mode_radius` and no flag
/// `0x100`; else 0. A NaN radius is not under it.
pub fn mode(kind: u32, flags: u32, frusta: u16, radius: f32, mode_radius: f32) -> u32 {
    if kind == 0 && flags & 0x80 != 0 {
        2
    } else if frusta == 0 || (mode_radius > radius && flags & 0x100 == 0) {
        1
    } else {
        0
    }
}

/// The main view's records, built from the observer camera, and the
/// guest's for the same view.
pub fn build(mem: &dyn GuestRead, guest: &RenderCamera) -> Result<(ViewRecord, ViewRecord), Decline> {
    let (u, l) = (Decline::Unread, Decline::Layout);
    let index = u32_at(mem, VIEW_INDEX).ok_or(u)?;
    if index >= 4 {
        return Err(l);
    }
    let rec = VIEW_RECORDS + VIEW_RECORD_SIZE * index;
    if u32_at(mem, rec + record::CINEMATIC).ok_or(u)? != 0 {
        return Err(Decline::Cinematic);
    }
    let player = u32_at(mem, rec + record::PLAYER).ok_or(u)? as i32;
    if !(0..4).contains(&player) || u16_at(mem, ACTIVE_BSP_INDEX).ok_or(u)? == u16::MAX {
        return Err(Decline::NoObserver);
    }
    if override_active(mem)? {
        return Err(Decline::Override);
    }
    let obs = OBSERVER_CAMERAS + OBSERVER_CAMERA_SIZE * player as u32;
    let v3 = |o: u32| vec3_at(mem, obs + o).ok_or(u);
    let lens = LensInputs::read(mem)?;
    let (viewport, ratio) = viewport_layout(mem, index, &lens)?;
    let fov = f32_read(mem, obs + observer::FOV)? * fov_factor(mem, ratio)?;
    let camera = RenderCamera {
        position: v3(observer::POSITION)?,
        forward: v3(observer::FORWARD)?,
        up: v3(observer::UP)?,
        fov,
        projection_xy: Some(projection_terms(fov, viewport, &lens)),
        viewport,
        near: f32_read(mem, NEAR)?,
        far: f32_read(mem, FAR)?,
    };
    let vis = engine::render_loop::occlusion::guest::MAIN_VIS;
    let field = |o: u32| at(vis, o).ok_or(l);
    let (kind, flags, frusta) = (
        u32_at(mem, field(VIS_KIND)?).ok_or(u)?,
        u32_at(mem, field(super::guest::vis::FLAGS)?).ok_or(u)?,
        u16_at(mem, field(super::guest::vis::FRUSTA)?).ok_or(u)?,
    );
    let radius = f32_read(mem, field(VIS_RADIUS)?)?;
    let rust = ViewRecord {
        camera,
        start: locate(mem, camera.position)?,
        mode: mode(kind, flags, frusta, radius, f32_read(mem, MODE_RADIUS)?),
    };
    let guest = ViewRecord {
        camera: *guest,
        start: u16_at(mem, field(super::guest::vis::START_CLUSTER)?).ok_or(u)?,
        mode: u32_at(mem, field(super::guest::vis::MODE)?).ok_or(u)?,
    };
    Ok((rust, guest))
}

/// The camera the visibility uses for the view: the Rust one when the
/// whole record equals the guest's, else the guest's, and the comparison.
pub fn arm(rust: &ViewRecord, guest: &ViewRecord) -> (RenderCamera, RecordParity) {
    let parity = compare(rust, guest);
    let camera = if parity.holds() {
        rust.camera
    } else {
        guest.camera
    };
    (camera, parity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;
    use engine::render_loop::occlusion::guest::MAIN_VIS;

    /// Invented lens constants: extents -1 and 1, half 0.5, offset scale 1,
    /// anamorphic 4/3 and a tangent factor of 2.
    fn lens_inputs() -> LensInputs {
        LensInputs {
            extent_min: -1.0,
            one: 1.0,
            half: 0.5,
            offset_scale: 1.0,
            anamorphic: 4.0 / 3.0,
            anamorphic_tan: 2.0,
            widescreen: false,
            display_flag: false,
            override_lens: false,
        }
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1e-5
    }

    #[test]
    fn the_projection_terms_take_the_aspect_and_the_widescreen_factors() {
        let vp = [0, 0, 480, 640];
        let right = std::f32::consts::FRAC_PI_2;
        let [sx, sy, ox, oy] = projection_terms(right, vp, &lens_inputs());
        assert!(near(sx, 0.75) && near(sy, 1.0), "{sx} {sy}");
        assert_eq!((ox.to_bits(), oy.to_bits()), (0, 0), "centred: +0, not -0");
        let wide = LensInputs {
            widescreen: true,
            ..lens_inputs()
        };
        let [sx, sy, ..] = projection_terms(right, vp, &wide);
        assert!(near(sx, 0.5625) && near(sy, 1.0), "{sx} {sy}");
        let flagged = LensInputs {
            display_flag: true,
            ..wide
        };
        let [sx, sy, ..] = projection_terms(right, vp, &flagged);
        assert!(near(sx, 0.5625 / (4.0 / 3.0 * 2.0)) && near(sy, 1.0 / (4.0 / 3.0 * 2.0)));
        let overridden = LensInputs {
            override_lens: true,
            ..flagged
        };
        assert_eq!(
            projection_terms(right, vp, &overridden),
            projection_terms(right, vp, &lens_inputs())
        );
    }

    #[test]
    fn the_mode_follows_the_kind_the_flags_the_frusta_and_the_radius() {
        assert_eq!(mode(0, 0x80, 1, 10.0, 1.0), 2);
        assert_eq!(mode(1, 0x80, 1, 10.0, 1.0), 0, "only kind 0 takes the set");
        assert_eq!(mode(0, 0, 0, 10.0, 1.0), 1, "no frusta");
        assert_eq!(mode(0, 0, 1, 0.5, 1.0), 1, "a small radius");
        assert_eq!(mode(0, 0x100, 1, 0.5, 1.0), 0, "unless flag 0x100");
        assert_eq!(mode(0, 0, 1, f32::NAN, 1.0), 0);
    }

    const BSP: u32 = 0x6000;
    const STRUCTURE: u32 = 0x8000;

    /// One node on the plane x = 0: behind it leaf 0 (cluster 5), in front
    /// leaf 1 (cluster 7).
    fn bsp(m: &mut Mem) {
        let (nodes, planes, leaves) = (0x6100, 0x6200, 0x6300);
        m.w(ACTIVE_BSP_INDEX & !3, 0);
        m.w(STRUCTURE_BSP, BSP);
        m.w(BSP, 1);
        m.w(BSP + 4, nodes);
        m.w(BSP + 8, 1);
        m.w(BSP + 0xc, planes);
        // Plane 0, back 0x800000 (leaf 0), front 0x800001 (leaf 1).
        m.w(nodes, 0);
        m.w(nodes + 4, 0x8000_0180);
        for (i, v) in [1.0, 0.0, 0.0, 0.0].into_iter().enumerate() {
            m.f(planes + 4 * i as u32, v);
        }
        m.w(super::super::guest::ACTIVE_STRUCTURE, STRUCTURE);
        m.w(STRUCTURE + LEAF_COUNT, 2);
        m.w(STRUCTURE + LEAVES, leaves);
        m.w(leaves, 5);
        m.w(leaves + 8, 7);
    }

    #[test]
    fn the_location_walks_the_bsp_to_a_leaf_and_its_cluster() {
        let mut m = Mem::default();
        bsp(&mut m);
        assert_eq!(locate(&m, [1.0, 0.0, 0.0]), Ok(7));
        assert_eq!(locate(&m, [-1.0, 0.0, 0.0]), Ok(5));
        assert_eq!(locate(&m, [0.0, 0.0, 0.0]), Ok(7), "on the plane is in front");
        m.w(0x6104, 0xffff_ff80);
        assert_eq!(locate(&m, [1.0, 0.0, 0.0]), Ok(0xffff), "solid");
        m.w(0x6104, 0x8000_0280);
        assert_eq!(locate(&m, [1.0, 0.0, 0.0]), Err(Decline::Layout), "leaf 2 of 2");
        m.w(0x6100, 1);
        assert_eq!(locate(&m, [1.0, 0.0, 0.0]), Err(Decline::Layout), "plane 1 of 1");
        m.w(ACTIVE_BSP_INDEX & !3, 0xffff);
        assert_eq!(locate(&m, [1.0, 0.0, 0.0]), Ok(0xffff), "no structure");
    }

    /// The main view of player 0, full screen 640 by 480, at (1, 2, 3)
    /// looking down +x, in front of the BSP's plane (cluster 7).
    fn scene() -> Mem {
        let mut m = Mem::default();
        bsp(&mut m);
        m.w(VIEW_INDEX, 0);
        m.w(VIEW_RECORDS + record::CINEMATIC, 0);
        m.w(VIEW_RECORDS + record::PLAYER, 0);
        m.w(PLAYER_GLOBALS, 0x7100);
        m.w(0x7108, 1);
        m.w(CAMERA_OVERRIDE, 0);
        for (i, v) in [1.0, 2.0, 3.0].into_iter().enumerate() {
            m.f(OBSERVER_CAMERAS + observer::POSITION + 4 * i as u32, v);
        }
        for (i, v) in [1.0, 0.0, 0.0].into_iter().enumerate() {
            m.f(OBSERVER_CAMERAS + observer::FORWARD + 4 * i as u32, v);
        }
        for (i, v) in [0.0, 0.0, 1.0].into_iter().enumerate() {
            m.f(OBSERVER_CAMERAS + observer::UP + 4 * i as u32, v);
        }
        m.f(OBSERVER_CAMERAS + observer::FOV, 1.2);
        // The screen at 0x485a8a: top, left, bottom, right.
        m.w(SCREEN & !3, 0);
        m.w((SCREEN & !3) + 4, 480 << 16);
        m.w((SCREEN & !3) + 8, 640 | (9 << 16));
        m.w((SCREEN & !3) + 12, 7 | (470 << 16));
        m.w((SCREEN & !3) + 16, 630);
        m.f(FOV_FACTOR, 1.0);
        m.f(FOV_FACTOR_50, 0.75);
        m.f(NEAR, 0.0625);
        m.f(FAR, 1024.0);
        for (at, v) in [
            (lens::EXTENT_MIN, -1.0),
            (lens::ONE, 1.0),
            (lens::HALF, 0.5),
            (lens::OFFSET_SCALE, 1.0),
            (lens::ANAMORPHIC, 4.0 / 3.0),
            (lens::ANAMORPHIC_TAN, 2.0),
            (MODE_RADIUS, 1.0),
        ] {
            m.f(at, v);
        }
        m.w(lens::WIDESCREEN & !3, 0);
        m.w(lens::DISPLAY, 0);
        m.w(MAIN_VIS + VIS_KIND, 0);
        m.w(MAIN_VIS + super::super::guest::vis::FLAGS, 0);
        m.w(MAIN_VIS + super::super::guest::vis::FRUSTA, 1);
        m.f(MAIN_VIS + VIS_RADIUS, 10.0);
        m.w(MAIN_VIS + super::super::guest::vis::START_CLUSTER, 7);
        m.w(MAIN_VIS + super::super::guest::vis::MODE, 0);
        m
    }

    fn guest_camera() -> RenderCamera {
        let fov = 1.2;
        RenderCamera {
            position: [1.0, 2.0, 3.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov,
            projection_xy: Some(projection_terms(fov, [0, 0, 480, 640], &lens_inputs())),
            viewport: [0, 0, 480, 640],
            near: 0.0625,
            far: 1024.0,
        }
    }

    #[test]
    fn the_record_built_from_the_observer_equals_the_guests_and_is_taken() {
        let m = scene();
        let (rust, guest) = build(&m, &guest_camera()).unwrap();
        assert_eq!((rust.start, rust.mode), (7, 0));
        let (camera, parity) = arm(&rust, &guest);
        assert!(parity.holds(), "{parity:?}");
        assert_eq!(camera, rust.camera);
    }

    #[test]
    fn a_deliberate_mismatch_names_the_first_field_and_keeps_the_guests_camera() {
        let m = scene();
        let guest_cam = RenderCamera {
            fov: 1.25,
            ..guest_camera()
        };
        let (rust, guest) = build(&m, &guest_cam).unwrap();
        let (camera, parity) = arm(&rust, &guest);
        assert_eq!(parity.first, Some(Field::Fov));
        assert_eq!(parity.differ, 1);
        assert_eq!(camera, guest_cam, "the guest's camera, untouched");
        let mut m = scene();
        m.w(MAIN_VIS + super::super::guest::vis::START_CLUSTER, 5);
        let (rust, guest) = build(&m, &guest_camera()).unwrap();
        let p = compare(&rust, &guest);
        assert_eq!((p.first, p.first.unwrap().to_string()), (Some(Field::Start), String::from("start cluster")));
    }

    #[test]
    fn three_players_split_the_inner_screen_and_keep_the_outer_edges() {
        let mut m = scene();
        m.w(0x7108, 3);
        let stacked = lens_inputs();
        let wide = LensInputs { widescreen: true, ..stacked };
        assert_eq!(viewport_layout(&m, 0, &stacked), Ok(([0, 0, 239, 640], 200)));
        assert_eq!(viewport_layout(&m, 2, &stacked), Ok(([239, 318, 480, 640], 100)));
        assert_eq!(viewport_layout(&m, 0, &wide), Ok(([0, 0, 480, 318], 50)));
        assert_eq!(viewport_layout(&m, 1, &wide), Ok(([0, 318, 239, 640], 100)));
        assert_eq!(viewport_layout(&m, 3, &wide), Err(Decline::Layout));
        m.w(0x7108, 5); // The guest caps the player count at four.
        assert_eq!(viewport_layout(&m, 0, &wide), Ok(([0, 0, 239, 318], 100)));
        assert_eq!(viewport_layout(&m, 3, &wide), Ok(([239, 318, 480, 640], 100)));
        let single = LensInputs { display_flag: true, ..wide };
        assert_eq!(viewport_layout(&m, 0, &single), Ok(([0, 0, 480, 640], 100)));
    }

    #[test]
    fn a_wrong_guest_viewport_is_named_and_kept() {
        let bad = RenderCamera { viewport: [2, 3, 411, 527], ..guest_camera() };
        let (rust, guest) = build(&scene(), &bad).unwrap();
        let (camera, parity) = arm(&rust, &guest);
        assert_eq!((parity.first, parity.differ), (Some(Field::Viewport), 1));
        assert_eq!(camera, bad);
    }

    #[test]
    fn a_split_view_takes_its_factor_and_views_it_does_not_build_decline() {
        let mut m = scene();
        // Side by side: half the width, the whole height: ratio 50.
        m.w(0x7108, 2);
        m.w(lens::WIDESCREEN & !3, 1 << 16);
        let (rust, _) = build(&m, &guest_camera()).unwrap();
        assert_eq!(rust.camera.fov, 1.2 * 0.75);
        assert_eq!(rust.camera.viewport, [0, 0, 480, 318]);
        let mut m = scene();
        m.w(VIEW_RECORDS + record::CINEMATIC, 1);
        assert_eq!(build(&m, &guest_camera()), Err(Decline::Cinematic));
        let mut m = scene();
        m.w(VIEW_RECORDS + record::PLAYER, u32::MAX);
        assert_eq!(build(&m, &guest_camera()), Err(Decline::NoObserver));
        let mut m = scene();
        m.w(CAMERA_OVERRIDE, 0x7000);
        m.w(0x7000 + 0x1120, 1);
        m.w(0x7008, 3);
        assert_eq!(build(&m, &guest_camera()), Err(Decline::Override));
        assert_eq!(Decline::ALL.map(Decline::name).len(), 5);
    }
}
