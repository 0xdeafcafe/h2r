//! Motion data on the two sides of an island solve. The accumulator's
//! last three vectors hold the transposed body basis while building the
//! Jacobians. During the solve that same space holds iteration sums.
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
    fn offset_preserves_the_fourth_lane_before_subtraction() {
        let result = det::checked(|fp| {
            quaternion_offset(fp, 0x027f, [0.5, 0.5, 0.5, 0.5], [1.0, 2.0, 3.0, 4.0], 1.0)
        })
        .unwrap();
        assert_eq!(result, [3.0, 1.0, 2.0, -2.0]);
    }
}
