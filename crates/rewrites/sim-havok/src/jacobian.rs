//! Motion data on the two sides of an island solve. The accumulator's
//! last three vectors hold the transposed body basis while building the
//! Jacobians. During the solve that same space holds iteration sums.
use crate::mem::Journal;
use crate::motion::{self, ForceInput, Pose, PoseConstants, PoseInput, Vector};
use sim_core::det::{self, Chain, Checked};

type Basis = [Vector; 3];

pub struct Accumulator {
    pub linear: Vector,
    pub angular: Vector,
    pub inverse_mass_inertia: Vector,
    pub center: Vector,
    pub world_to_body: Basis,
}

/// Force integration precedes the snapshot into solver space. Both
/// dynamic motion types use this builder.
pub fn build(
    force: ForceInput,
    center: Vector,
    one: f32,
    zero: f32,
    control: u16,
) -> Option<(motion::Velocities, Accumulator)> {
    let velocities = motion::apply_forces(force, one, zero, control)?;
    let world_to_body =
        std::array::from_fn(|i| [force.basis[0][i], force.basis[1][i], force.basis[2][i], 0.0]);
    let angular = det::checked(|fp| Some(motion::rotate(fp, &world_to_body, velocities.angular)))?;
    let accumulator = Accumulator {
        linear: velocities.linear,
        angular,
        inverse_mass_inertia: force.inverse_inertia,
        center,
        world_to_body,
    };
    Some((velocities, accumulator))
}

/// A constraint class with a ported `vt[+0x24]` (docs/engine/havok-step.md,
/// "Constraints"), told apart by that slot's target. None is ported yet:
/// the contact class (`0x0031bdf0`, which calls `0x0031f620`) and the six
/// joint classes make their island fall back to the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {}

/// The contact class's `vt[+0x24]`.
pub const CONTACT_BUILD: u32 = 0x0031_bdf0;

impl Class {
    pub fn of(build: u32) -> Option<Self> {
        let _ = build;
        None
    }

    /// Append this constraint's rows and schemas at the cursor.
    pub fn build(self, j: &mut Journal, constraint: u32, cursor: &mut Cursor) -> Option<()> {
        let _ = (j, constraint, cursor);
        match self {}
    }
}

/// The Jacobian cursor `0x0031c100` passes to each `vt[+0x24]` (its
/// frame's `+0x1c..+0x2c`): the next row, the next schema, the row limit
/// and the overflow block. Before each constraint, a cursor at or past the
/// limit moves to the overflow block and the limit becomes `u32::MAX`
/// (`0x0031c36f`). The limit is the end of the row area; when the rows
/// spill into a LIFO block it is the buffer's end less `I+0x08`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub rows: u32,
    pub schemas: u32,
    pub limit: u32,
    pub overflow: u32,
}

impl Cursor {
    pub fn before_constraint(&mut self) {
        if self.rows >= self.limit {
            self.rows = self.overflow;
            self.limit = u32::MAX;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Export {
    /// Integrate the velocities already in the motion.
    Keyframed,
    /// Integrate the masked iteration sums and keep those as velocity.
    Spherical,
    /// Integrate the iteration sums, then restore the solved velocity
    /// using the basis at the end of the step.
    Dynamic,
}

#[derive(Clone, Copy)]
pub struct Solved {
    pub linear_sum: Vector,
    pub angular_sum: Vector,
    pub linear: Vector,
    pub angular: Vector,
}

pub struct Writeback {
    pub pose: Pose,
    pub linear: Vector,
    pub angular: Vector,
}

fn masked(v: Vector, mask: [u32; 4]) -> Vector {
    std::array::from_fn(|i| f32::from_bits(v[i].to_bits() & mask[i]))
}

/// Two motion classes compute the translation directly from the
/// quaternion (0x2db7a0). Using the rounded basis would change the answer.
fn quaternion_offset(fp: &Checked, control: u16, q: Vector, v: Vector, one: f32) -> Option<Vector> {
    let load = |x| Chain::load(fp, control, x);
    let w = load(q[3])?;
    let square = w * w;
    let scale = (square + square - load(one)?).store();
    let q = [q[0], q[1], q[2], 0.0];
    let dot = fp.add(
        fp.mul(q[2], v[2]),
        fp.add(fp.mul(q[1], v[1]), fp.mul(q[0], v[0])),
    );
    let dot = load(dot)?;
    let dot2 = (dot + dot).store();
    let w2 = (w + w).store();
    Some(std::array::from_fn(|i| {
        let (j, k) = if i == 3 {
            (3, 3)
        } else {
            ((i + 1) % 3, (i + 2) % 3)
        };
        let cross = fp.sub(fp.mul(q[j], v[k]), fp.mul(q[k], v[j]));
        fp.add(
            fp.add(fp.mul(scale, v[i]), fp.mul(dot2, q[i])),
            fp.mul(w2, cross),
        )
    }))
}

pub fn export(
    kind: Export,
    mut input: PoseInput,
    basis: Basis,
    solved: Solved,
    scale: f32,
    constants: PoseConstants,
    control: u16,
) -> Option<Writeback> {
    if kind != Export::Keyframed {
        (input.linear, input.angular) = det::checked(|fp| {
            let angular = motion::rotate(fp, &basis, solved.angular_sum);
            Some((
                solved.linear_sum.map(|v| fp.mul(scale, v)),
                angular.map(|v| fp.mul(scale, v)),
            ))
        })?;
        if kind == Export::Spherical {
            input.linear = masked(input.linear, constants.angular_mask);
            input.angular = masked(input.angular, constants.angular_mask);
        }
    }
    let mut pose = motion::integrate_pose(input, constants, control)?;
    if kind != Export::Dynamic {
        pose.translation = det::checked(|fp| {
            let offset = quaternion_offset(
                fp,
                control,
                pose.orientation,
                input.local_center,
                constants.identity,
            )?;
            // The motion overwrites center.w with 1 after subtracting the offset.
            let mut center = pose.center;
            center[3] = fp.add(input.center[3], fp.mul(input.dt, input.linear[3]));
            Some(std::array::from_fn(|i| fp.sub(center[i], offset[i])))
        })?;
    }
    let (linear, angular) = if kind == Export::Dynamic {
        let angular = det::checked(|fp| Some(motion::rotate(fp, &pose.basis, solved.angular)))?;
        (
            masked(solved.linear, constants.angular_mask),
            masked(angular, constants.angular_mask),
        )
    } else {
        (input.linear, input.angular)
    };
    Some(Writeback {
        pose,
        linear,
        angular,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_moves_to_the_overflow_block_at_its_limit() {
        let mut c = Cursor { rows: 0x100, schemas: 0x800, limit: 0x100, overflow: 0x4000 };
        c.before_constraint();
        assert_eq!(c, Cursor { rows: 0x4000, schemas: 0x800, limit: u32::MAX, overflow: 0x4000 });
        let mut c = Cursor { rows: 0xff, schemas: 0x800, limit: 0x100, overflow: 0x4000 };
        c.before_constraint();
        assert_eq!(c.rows, 0xff);
        assert_eq!(Class::of(CONTACT_BUILD), None);
    }

    #[test]
    fn offset_preserves_the_fourth_lane_before_subtraction() {
        let result = det::checked(|fp| {
            quaternion_offset(fp, 0x027f, [0.5, 0.5, 0.5, 0.5], [1.0, 2.0, 3.0, 4.0], 1.0)
        })
        .unwrap();
        assert_eq!(result, [3.0, 1.0, 2.0, -2.0]);
    }
}
