//! The sky strip's node rows made in Rust from the sky's transform
//! (docs/graphics/retire-d3d-r5.md, "The sky's node rows"), for the rows
//! arm beside the guest's (`SkyItem::rows`).
//!
//! The guest's chain, read with `h2 disasm`:
//!
//! | Step | Code | What it makes |
//! |---|---|---|
//! | default pose | `0x0003ea60` (`0x0016d940`, `0x0016d9c0`) | the model's node matrices at rest, at `0x004c5040`; `0x004c5038` set, the node count at `0x004c503c` |
//! | sky matrix | `0x0003ebd0` | scale `sky +0x14`; forward `(cos a cos k, sin a cos k, sin k)` with `a` at `sky +0x80` and `k` the double at `0x0045dbe0`; up the global up (`[0x004687b0]`); left `up × forward`; position `sky +0x18` times the camera (`0x004b9da0`) |
//! | per node | `0x0003ebd0`'s loop | the rest matrix, its position plus `[0x004687a4]`, under the sky matrix (`0x00142a60`) |
//! | rows | `0x001376c0` | the node matrix under the node's inverse bind (`mode` node `+0x28`, nodes at `mode +0x4c`, `0x60` each), as three rows `(s·forward_i, s·left_i, s·up_i, position_i)` |
//!
//! Only a section drawn by one node (`SECTION_NODE` not -1) is made; a
//! region palette (`0x00137800`'s weighted blend) is declined, `palette`.

use super::decorators::SCENARIO;
use super::object_nodes::{MODEL_SECTIONS, Row, SECTION_NODE, SECTION_STRIDE, TAG_INSTANCES};
use super::world_layers::SKY_INDEXED_RETURN;
use crate::GuestRead;

const SKY_INDEX: u32 = 0x004b_9eec;
/// The rest pose `0x0003ea60` prepares: matrices, whether made, node count.
pub const REST_MATRICES: u32 = 0x004c_5040;
pub const REST_MADE: u32 = 0x004c_5038;
pub const REST_COUNT: u32 = 0x004c_503c;
/// The elevation constant of the sky's forward axis (a double).
pub const SKY_ELEVATION: u32 = 0x0045_dbe0;
/// Pointers to the global up vector and the per-node position offset.
pub const GLOBAL_UP: u32 = 0x0046_87b0;
pub const NODE_OFFSET: u32 = 0x0046_87a4;
/// The camera position the sky's translation scales.
pub const CAMERA_POSITION: u32 = 0x004b_9da0;
const MATRIX_SIZE: u32 = 0x34;
const MODE_NODE_COUNT: u32 = 0x48;
const MODE_NODES: u32 = 0x4c;
const NODE_SIZE: u32 = 0x60;
const NODE_INVERSE_BIND: u32 = 0x28;

/// A `real_matrix4x3`: scale, forward, left, up, position (13 floats).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Matrix {
    pub scale: f32,
    pub forward: [f32; 3],
    pub left: [f32; 3],
    pub up: [f32; 3],
    pub position: [f32; 3],
}

impl Matrix {
    pub const IDENTITY: Self =
        Self { scale: 1.0, forward: [1.0, 0.0, 0.0], left: [0.0, 1.0, 0.0], up: [0.0, 0.0, 1.0], position: [0.0; 3] };

    fn read(mem: &dyn GuestRead, at: u32) -> Option<Self> {
        let f = |k: u32| mem.read_f32(at.checked_add(4 * k)?);
        let v = |k: u32| Some([f(k)?, f(k + 1)?, f(k + 2)?]);
        Some(Self { scale: f(0)?, forward: v(1)?, left: v(4)?, up: v(7)?, position: v(10)? })
    }

    /// `self` applied after `b` (`0x00142a60`, `ecx` = `self`, `edx` = `b`),
    /// in single precision in the guest's order of operations.
    pub fn times(&self, b: &Self) -> Self {
        let a = self;
        let axis = |v: [f32; 3]| [0, 1, 2].map(|i| v[0] * a.forward[i] + (v[1] * a.left[i] + v[2] * a.up[i]));
        let position = [0, 1, 2].map(|i| {
            ((b.position[0] * a.forward[i] + b.position[1] * a.left[i]) + b.position[2] * a.up[i]) * a.scale + a.position[i]
        });
        Self { scale: b.scale * a.scale, forward: axis(b.forward), left: axis(b.left), up: axis(b.up), position }
    }

    /// The three hardware rows (`0x001376c0`).
    pub fn rows(&self) -> [Row; 3] {
        let s = self.scale;
        [0, 1, 2].map(|i| [s * self.forward[i], s * self.left[i], s * self.up[i], self.position[i]])
    }
}

/// The sky's matrix (`0x0003ebd0`): `scale`, rotation `angle` about the
/// up axis with the forward axis raised by `elevation`, positioned at
/// `translation_scale` times `camera`. The trigonometry is the x87's,
/// rounded to single precision as the guest stores it.
pub fn sky_matrix(scale: f32, translation_scale: f32, angle: f32, elevation: f64, up: [f32; 3], camera: [f32; 3]) -> Matrix {
    let (a, k) = (f64::from(angle), elevation);
    let forward = [(a.cos() * k.cos()) as f32, (a.sin() * k.cos()) as f32, k.sin() as f32];
    let wide = |x: f32, y: f32| f64::from(x) * f64::from(y);
    let left = [
        (wide(forward[2], up[1]) - wide(forward[1], up[2])) as f32,
        (wide(forward[0], up[2]) - wide(forward[2], up[0])) as f32,
        (wide(forward[1], up[0]) - wide(forward[0], up[1])) as f32,
    ];
    Matrix { scale, forward, left, up, position: camera.map(|c| translation_scale * c) }
}

/// The rows of rest node `node` under `sky` (`0x0003ebd0`'s loop, then
/// `0x001376c0`).
pub fn node_rows(sky: &Matrix, rest: &Matrix, offset: [f32; 3], inverse_bind: &Matrix) -> [Row; 3] {
    let mut rest = *rest;
    for i in 0..3 {
        rest.position[i] += offset[i];
    }
    sky.times(&rest).times(inverse_bind).rows()
}

fn field(mem: &dyn GuestRead, base: u32, offset: u32) -> Option<u32> {
    if base == 0 {
        return None;
    }
    mem.read_u32(base.checked_add(offset)?)
}

fn tag(mem: &dyn GuestRead, table: u32, datum: u32) -> Option<u32> {
    if datum == u32::MAX {
        return None;
    }
    field(mem, table, (datum & 0xffff) * 16 + 8)
}

fn vector(mem: &dyn GuestRead, at: u32) -> Option<[f32; 3]> {
    Some([mem.read_f32(at)?, mem.read_f32(at.checked_add(4)?)?, mem.read_f32(at.checked_add(8)?)?])
}

/// The rows of the strip at the sky's indexed draw (`esp` as
/// `SkyItem::read` takes it), made from the sky's transform; or why not:
/// `site`, `sky` (the sky or its model does not read), `rest` (the rest
/// pose is not made for this model), `palette` (a region palette),
/// `node`, `constants`.
pub fn native_rows(mem: &dyn GuestRead, esp: u32) -> Result<Vec<Row>, &'static str> {
    let stack = |offset| field(mem, esp, offset);
    if stack(0) != Some(SKY_INDEXED_RETURN) || stack(0xcc) != Some(0x0003_eeb1) {
        return Err("site");
    }
    let table = mem.read_u32(TAG_INSTANCES).ok_or("sky")?;
    let scenario = mem.read_u32(SCENARIO).ok_or("sky")?;
    let index = u32::from(mem.read_u32(SKY_INDEX).ok_or("sky")? as u16);
    if index >= field(mem, scenario, 8).ok_or("sky")? || index > i16::MAX as u32 {
        return Err("sky");
    }
    let skies = field(mem, scenario, 12).ok_or("sky")?;
    let sky = tag(mem, table, field(mem, skies, index * 8 + 4).ok_or("sky")?).ok_or("sky")?;
    let model = tag(mem, table, field(mem, sky, 4).ok_or("sky")?).ok_or("sky")?;
    if stack(0x28) != Some(model) {
        return Err("sky");
    }
    let section = stack(0x24).ok_or("sky")?;
    let sections = field(mem, model, MODEL_SECTIONS).ok_or("sky")?;
    if section.checked_sub(sections).is_none_or(|o| o % SECTION_STRIDE != 0) {
        return Err("sky");
    }
    let node = field(mem, section, SECTION_NODE).ok_or("sky")? as i16;
    let Ok(node) = u32::try_from(node) else { return Err("palette") };
    let count = field(mem, model, MODE_NODE_COUNT).ok_or("node")?;
    if mem.read_u32(REST_MADE).is_none_or(|w| w & 0xff == 0) || mem.read_u32(REST_COUNT) != Some(count) {
        return Err("rest");
    }
    if node >= count {
        return Err("node");
    }
    let constants = || -> Option<_> {
        let elevation = f64::from_bits(u64::from(mem.read_u32(SKY_ELEVATION)?) | u64::from(mem.read_u32(SKY_ELEVATION + 4)?) << 32);
        let up = vector(mem, mem.read_u32(GLOBAL_UP)?)?;
        let offset = vector(mem, mem.read_u32(NODE_OFFSET)?)?;
        let camera = vector(mem, CAMERA_POSITION)?;
        let scale = mem.read_f32(sky.checked_add(0x14)?)?;
        let translation = mem.read_f32(sky.checked_add(0x18)?)?;
        let angle = mem.read_f32(sky.checked_add(0x80)?)?;
        Some((sky_matrix(scale, translation, angle, elevation, up, camera), offset))
    };
    let (matrix, offset) = constants().ok_or("constants")?;
    let rest = Matrix::read(mem, REST_MATRICES + node * MATRIX_SIZE).ok_or("rest")?;
    let nodes = field(mem, model, MODE_NODES).ok_or("node")?;
    let bind = Matrix::read(mem, nodes.checked_add(node * NODE_SIZE + NODE_INVERSE_BIND).ok_or("node")?).ok_or("node")?;
    let rows = node_rows(&matrix, &rest, offset, &bind);
    if !rows.iter().flatten().all(|v| v.is_finite()) {
        return Err("constants");
    }
    Ok(rows.to_vec())
}

/// How the made rows compare with the guest's: `exact` bit for bit,
/// `close` within the x87's rounding of the trigonometry (a relative
/// `1e-5`, at least `1e-5`), or `differ`.
pub fn compare(native: &[Row], guest: &[Row]) -> &'static str {
    if native.len() != guest.len() {
        return "differ";
    }
    let pairs = || native.iter().flatten().zip(guest.iter().flatten());
    if pairs().all(|(a, b)| a.to_bits() == b.to_bits()) {
        "exact"
    } else if pairs().all(|(a, b)| (a - b).abs() <= 1e-5 * b.abs().max(1.0)) {
        "close"
    } else {
        "differ"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;

    fn put(m: &mut Mem, at: u32, x: &Matrix) {
        let words = [x.scale, x.forward[0], x.forward[1], x.forward[2], x.left[0], x.left[1], x.left[2], x.up[0], x.up[1], x.up[2]];
        for (k, v) in words.into_iter().chain(x.position).enumerate() {
            m.f(at + 4 * k as u32, v);
        }
    }

    #[test]
    fn the_product_is_the_guests_composition() {
        let a = Matrix { scale: 2.0, forward: [0.0, 1.0, 0.0], left: [-1.0, 0.0, 0.0], up: [0.0, 0.0, 1.0], position: [10.0, 0.0, 5.0] };
        let b = Matrix { scale: 3.0, position: [1.0, 2.0, 3.0], ..Matrix::IDENTITY };
        let c = a.times(&b);
        assert_eq!(c.scale, 6.0);
        // b's position through a's axes (x to +y, y to -x), scaled, moved.
        assert_eq!(c.position, [10.0 - 4.0, 2.0, 5.0 + 6.0]);
        assert_eq!((c.forward, c.left, c.up), (a.forward, a.left, a.up), "b's axes are the identity's");
        assert_eq!(c.rows(), [[0.0, -6.0, 0.0, 6.0], [6.0, 0.0, 0.0, 2.0], [0.0, 0.0, 6.0, 11.0]]);
        assert_eq!(Matrix::IDENTITY.times(&b), b);
    }

    #[test]
    fn the_sky_turns_about_up_and_follows_the_camera_by_its_scale() {
        let up = [0.0, 0.0, 1.0];
        let m = sky_matrix(400.0, 1.0, 0.0, 0.0, up, [3.0, 4.0, 5.0]);
        assert_eq!((m.forward, m.left, m.up), ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], up));
        assert_eq!((m.scale, m.position), (400.0, [3.0, 4.0, 5.0]));
        let turned = sky_matrix(1.0, -1.0, std::f32::consts::FRAC_PI_2, 0.0, up, [3.0, 4.0, 5.0]);
        assert!(turned.forward[0].abs() < 1e-7 && turned.forward[1] == 1.0);
        assert!((turned.left[0] + 1.0).abs() < 1e-7, "left is up × forward");
        assert_eq!(turned.position, [-3.0, -4.0, -5.0]);
        let rows = node_rows(&m, &Matrix::IDENTITY, [0.0; 3], &Matrix::IDENTITY);
        assert_eq!(rows, [[400.0, 0.0, 0.0, 3.0], [0.0, 400.0, 0.0, 4.0], [0.0, 0.0, 400.0, 5.0]]);
        let moved = node_rows(&m, &Matrix::IDENTITY, [1.0, 0.0, 0.0], &Matrix::IDENTITY);
        assert_eq!(moved[0][3], 403.0, "the offset is in the node's frame, under the sky's scale");
    }

    #[test]
    fn the_strips_rows_are_made_from_the_skys_tag_and_the_rest_pose() {
        let mut mem = Mem::default();
        let (esp, scenario, skies, table, sky, model) = (0x1000, 0x2000, 0x3000, 0x4000, 0x5000, 0x6000);
        let (sections, nodes, up, offset) = (0x7000, 0x8000, 0x9000, 0x9100);
        let section = sections + SECTION_STRIDE;
        for (at, value) in [
            (esp, SKY_INDEXED_RETURN),
            (esp + 0xcc, 0x0003_eeb1),
            (SCENARIO, scenario),
            (SKY_INDEX, 0),
            (scenario + 8, 1),
            (scenario + 12, skies),
            (skies + 4, 0xab00_0002),
            (TAG_INSTANCES, table),
            (table + 2 * 16 + 8, sky),
            (sky + 4, 0xbc00_0003),
            (table + 3 * 16 + 8, model),
            (esp + 0x28, model),
            (esp + 0x24, section),
            (model + MODEL_SECTIONS, sections),
            (section + SECTION_NODE, 1),
            (model + MODE_NODE_COUNT, 2),
            (model + MODE_NODES, nodes),
            (REST_MADE, 1),
            (REST_COUNT, 2),
            (GLOBAL_UP, up),
            (NODE_OFFSET, offset),
            (SKY_ELEVATION, 0),
            (SKY_ELEVATION + 4, 0),
        ] {
            mem.w(at, value);
        }
        for (k, v) in [0.0, 0.0, 1.0].into_iter().enumerate() {
            mem.f(up + 4 * k as u32, v);
            mem.f(offset + 4 * k as u32, 0.0);
        }
        mem.f(sky + 0x80, 0.0);
        mem.f(sky + 0x14, 2.0);
        mem.f(sky + 0x18, 1.0);
        for (k, v) in [10.0, 20.0, 30.0].into_iter().enumerate() {
            mem.f(CAMERA_POSITION + 4 * k as u32, v);
        }
        put(&mut mem, REST_MATRICES + MATRIX_SIZE, &Matrix { position: [0.0, 0.0, 1.0], ..Matrix::IDENTITY });
        put(&mut mem, nodes + NODE_SIZE + NODE_INVERSE_BIND, &Matrix { position: [0.0, 0.0, -1.0], ..Matrix::IDENTITY });
        let rows = native_rows(&mem, esp).unwrap();
        assert_eq!(rows, vec![[2.0, 0.0, 0.0, 10.0], [0.0, 2.0, 0.0, 20.0], [0.0, 0.0, 2.0, 30.0]]);
        assert_eq!(compare(&rows, &rows), "exact");
        let mut near = rows.clone();
        near[0][3] = 10.000_01;
        assert_eq!(compare(&rows, &near), "close");
        near[0][3] = 11.0;
        assert_eq!(compare(&rows, &near), "differ");
        mem.w(REST_COUNT, 3);
        assert_eq!(native_rows(&mem, esp), Err("rest"), "the rest pose is another model's");
        mem.w(REST_COUNT, 2);
        mem.w(section + SECTION_NODE, 0xffff);
        assert_eq!(native_rows(&mem, esp), Err("palette"));
        mem.w(esp + 0xcc, 0);
        assert_eq!(native_rows(&mem, esp), Err("site"));
    }
}
