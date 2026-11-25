//! The decorator grid walk at 0x0003f830 and 0x0003f970. Its cells are
//! distinct from the BSP clusters used for a group's portal sphere test.

use super::*;
use crate::layout::skip::MAX_ITEMS;
use engine::render_loop::occlusion::guest::MAIN_VIS;

pub const VISIBLE_COUNT: u32 = 0x0050_9438;
const DRAW_DISTANCE: u32 = 0x0052_5930;
const LEAF_HALF_SIZE: u32 = 0x0052_5924;
const POSITIVE_EPSILON: u32 = 0x0045_dbdc;
const NEGATIVE_EPSILON: u32 = 0x0045_ddfc;
const PLANE_SCALE: u32 = 0x0043_f9d0;

#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub index: u32,
    pub origin: [f32; 3],
}

fn floats<const N: usize>(mem: &dyn GuestRead, at: u32) -> Result<[f32; N], Refusal> {
    let mut out = [0.0; N];
    for (i, v) in out.iter_mut().enumerate() {
        *v = float(mem, at, i as u32 * 4)?;
    }
    Ok(out)
}

struct Grid<'a> {
    mem: &'a dyn GuestRead,
    block: u32,
    bounds: [f32; 6],
    planes: [[f32; 4]; 6],
    plane_scale: f32,
    leaf: f32,
    min_delta: f32,
    max_delta: f32,
    child_scale: f32,
    visited: u32,
    cells: Vec<Cell>,
}

impl Grid<'_> {
    fn visit(&mut self, cell: Cell, half: f32, depth: u32) -> Result<(), Refusal> {
        if self.cells.len() == 256 || cell.index == u32::from(u16::MAX) {
            return Ok(());
        }
        self.visited += 1;
        if depth > 32 || self.visited > MAX_ITEMS {
            return Err(Refusal::Unread);
        }
        for (axis, position) in cell.origin.into_iter().enumerate() {
            if self.bounds[axis * 2] > position + half
                || position - half > self.bounds[axis * 2 + 1]
            {
                return Ok(());
            }
        }
        let record = block_record(self.mem, self.block, 0x20, cell.index, 0x18)?;
        let delta = half - self.leaf;
        if self.max_delta > delta && delta > self.min_delta {
            let [x, y, z] = cell.origin;
            let radius = half * self.plane_scale;
            // 0x0003fd20 rejects on equality and tests X, Z, Y in that order.
            if !self
                .planes
                .iter()
                .any(|&[nx, ny, nz, d]| ((nx * x + nz * z) + ny * y) - d >= radius)
            {
                self.cells.push(cell);
            }
            return Ok(());
        }
        let extent = half * self.child_scale;
        for child in 0..8 {
            let index = u32::from(super::half(self.mem, record, 2 * child)?);
            let origin = [0, 1, 2].map(|axis| {
                if child & (4 >> axis) == 0 {
                    cell.origin[axis] - extent
                } else {
                    cell.origin[axis] + extent
                }
            });
            self.visit(Cell { index, origin }, extent, depth + 1)?;
        }
        Ok(())
    }
}

/// Rebuild the visible grid from the input view and the BSP's octree.
pub fn build(mem: &dyn GuestRead, block: u32, camera: [f32; 3]) -> Result<Vec<Cell>, Refusal> {
    let count = word(mem, block, 0x20)? as i32;
    let frustum = word(mem, MAIN_VIS, 0)?;
    let mut bounds: [f32; 6] = floats(mem, frustum.checked_add(0xf4).ok_or(Refusal::Unread)?)?;
    let distance = float(mem, DRAW_DISTANCE, 0)?;
    for (axis, position) in camera.into_iter().enumerate() {
        let min = position - distance;
        let max = position + distance;
        if min > bounds[2 * axis] {
            bounds[2 * axis] = min;
        }
        if bounds[2 * axis + 1] > max {
            bounds[2 * axis + 1] = max;
        }
    }
    if count <= 0 {
        return Ok(Vec::new());
    }
    let mut planes = [[0.0; 4]; 6];
    for (i, plane) in planes.iter_mut().enumerate() {
        *plane = floats(
            mem,
            frustum
                .checked_add(0x10c + 16 * i as u32)
                .ok_or(Refusal::Unread)?,
        )?;
    }
    let mut grid = Grid {
        mem,
        block,
        bounds,
        planes,
        plane_scale: float(mem, PLANE_SCALE, 0)?,
        leaf: float(mem, LEAF_HALF_SIZE, 0)?,
        min_delta: float(mem, NEGATIVE_EPSILON, 0)?,
        max_delta: float(mem, POSITIVE_EPSILON, 0)?,
        child_scale: float(mem, constant::COLOUR_FLOOR, 0)?,
        visited: 0,
        cells: Vec::new(),
    };
    let root = Cell {
        index: 0,
        origin: floats(mem, block)?,
    };
    let half = (word(mem, block, 0xc)? as i32 as f32 * float(mem, constant::POSITION, 0)?)
        * grid.child_scale;
    grid.visit(root, half, 0)?;
    Ok(grid.cells)
}

/// Check slot indices and all origin bits. Every draw addresses a slot by
/// its low eight argument bits.
pub fn matches(mem: &dyn GuestRead, cells: &[Cell]) -> Result<bool, Refusal> {
    if cells.len() as u32 != word(mem, VISIBLE_COUNT, 0)? {
        return Ok(false);
    }
    for (slot, cell) in cells.iter().enumerate() {
        let at = VISIBLE_CLUSTERS + 0x10 * slot as u32;
        if word(mem, at, 0)? != cell.index
            || floats::<3>(mem, at + 4)?.map(f32::to_bits) != cell.origin.map(f32::to_bits)
        {
            return Ok(false);
        }
    }
    Ok(true)
}
