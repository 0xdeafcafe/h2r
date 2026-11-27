//! Motion state for one unconstrained body. Positions include the swept
//! transform's fourth word. All four lanes participate in the packed math.
use sim_core::det::{self, Chain, Checked};

pub type Vector = [f32; 4];
type Basis = [Vector; 3];

/// Values loaded through the image at the motion and normalisation entries.
#[derive(Clone, Copy)]
pub struct PoseConstants {
    pub angular_mask: [u32; 4],
    pub normalise_bias: f32,
    pub normalise_scale: f32,
    pub identity: f32,
}

#[derive(Clone, Copy)]
pub struct PoseInput {
    pub dt: f32,
    pub sweep_scale: f32,
    pub local_center: Vector,
    pub orientation: Vector,
    pub linear: Vector,
    pub angular: Vector,
    pub center: Vector,
}

pub struct Pose {
    pub previous_center: Vector,
    pub center: Vector,
    pub orientation: Vector,
    pub basis: [Vector; 3],
    pub translation: Vector,
    pub saved_linear: Vector,
    pub saved_angular: Vector,
}

fn add(fp: &Checked, a: Vector, b: Vector) -> Vector {
    std::array::from_fn(|i| fp.add(a[i], b[i]))
}

fn scaled(fp: &Checked, scalar: f32, v: Vector) -> Vector {
    v.map(|component| fp.mul(scalar, component))
}

fn dot3(fp: &Checked, a: Vector, b: Vector) -> f32 {
    fp.add(
        fp.mul(a[2], b[2]),
        fp.add(fp.mul(a[1], b[1]), fp.mul(a[0], b[0])),
    )
}

pub fn rotate(fp: &Checked, columns: &Basis, v: Vector) -> Vector {
    std::array::from_fn(|i| {
        let xy = fp.add(fp.mul(columns[0][i], v[0]), fp.mul(columns[1][i], v[1]));
        fp.add(xy, fp.mul(columns[2][i], v[2]))
    })
}

/// Quaternion velocity product at the dynamic motion integration boundary.
fn quaternion_rate(fp: &Checked, control: u16, omega: Vector, q: Vector) -> Option<Vector> {
    let mut rate = [0.0; 4];
    for i in 0..3 {
        let (j, k) = ((i + 1) % 3, (i + 2) % 3);
        let cross = fp.sub(fp.mul(omega[j], q[k]), fp.mul(omega[k], q[j]));
        rate[i] = fp.add(
            fp.add(cross, fp.mul(omega[3], q[i])),
            fp.mul(q[3], omega[i]),
        );
    }
    let x = |v| Chain::load(fp, control, v);
    rate[3] = (x(omega[3])? * x(q[3])? - x(dot3(fp, omega, q))?).store();
    Some(rate)
}

fn normalise(fp: &Checked, q: Vector, constants: PoseConstants) -> Vector {
    let squares = q.map(|v| fp.mul(v, v));
    let n = fp.add(
        fp.add(squares[2], squares[0]),
        fp.add(squares[3], squares[1]),
    );
    let inverse = f32::from_bits(det::rsqrt(n.to_bits()));
    let residual = fp.sub(
        constants.normalise_bias,
        fp.mul(fp.mul(n, inverse), inverse),
    );
    scaled(
        fp,
        fp.mul(fp.mul(constants.normalise_scale, inverse), residual),
        q,
    )
}

/// The x87 matrix builder stores most cross terms before composing the
/// columns. The y*w and z*w terms stay in working precision.
fn basis(fp: &Checked, control: u16, q: Vector, identity: f32) -> Option<Basis> {
    let load = |v| Chain::load(fp, control, v);
    let [x, y, z, w] = q;
    let (x, y, z, w) = (load(x)?, load(y)?, load(z)?, load(w)?);
    let (dx, dy, dz) = (x + x, y + y, load((z + z).store())?);
    let xx = load((dx * x).store())?;
    let xy = load((dy * x).store())?;
    let xz = load((dz * x).store())?;
    let yy = load((dy * y).store())?;
    let yz = load((dz * y).store())?;
    let zz = load((dz * z).store())?;
    let xw = load((dx * w).store())?;
    let (yw, zw) = (dy * w, dz * w);
    let one = load(identity)?;
    Some([
        [
            (one - (zz + yy)).store(),
            (xy + zw).store(),
            (xz - yw).store(),
            0.0,
        ],
        [
            (xy - zw).store(),
            (one - (zz + xx)).store(),
            (xw + yz).store(),
            0.0,
        ],
        [
            (yw + xz).store(),
            (yz - xw).store(),
            (one - (yy + xx)).store(),
            0.0,
        ],
    ])
}

/// Unconstrained motion vtable slot +4. A failed arithmetic check leaves
/// the entire motion for the guest. No working state survives the call.
pub fn integrate_pose(input: PoseInput, constants: PoseConstants, control: u16) -> Option<Pose> {
    det::checked(|fp| {
        let load = |v| Chain::load(fp, control, v);
        let mut previous_center = input.center;
        let mut center = add(fp, input.center, scaled(fp, input.dt, input.linear));
        let magnitude = fp.sqrt(dot3(fp, input.angular, input.angular));
        previous_center[3] =
            (load(input.sweep_scale)? * load(input.dt)? * load(magnitude)?).store();

        let half = scaled(fp, 0.5, input.angular);
        let omega =
            std::array::from_fn(|i| f32::from_bits(half[i].to_bits() & constants.angular_mask[i]));
        let rate = quaternion_rate(fp, control, omega, input.orientation)?;
        let orientation = normalise(
            fp,
            add(fp, input.orientation, scaled(fp, input.dt, rate)),
            constants,
        );
        let basis = basis(fp, control, orientation, constants.identity)?;
        let offset = rotate(fp, &basis, input.local_center);
        let translation = std::array::from_fn(|i| fp.sub(center[i], offset[i]));
        center[3] = 1.0;
        Some(Pose {
            previous_center,
            center,
            orientation,
            basis,
            translation,
            saved_linear: input.linear,
            saved_angular: input.angular,
        })
    })
}

#[derive(Clone, Copy)]
pub struct ForceInput {
    pub dt: f32,
    pub gravity: Vector,
    pub linear: Vector,
    pub angular: Vector,
    pub force: Vector,
    pub torque: Vector,
    pub inverse_mass: f32,
    pub inverse_inertia: Vector,
    pub basis: [Vector; 3],
    pub linear_damping: f32,
    pub angular_damping: f32,
}

pub struct Velocities {
    pub linear: Vector,
    pub angular: Vector,
    pub clear_torque: bool,
}

/// Unconstrained motion vtable slot +8. The torque's fourth lane marks
/// pending work. The force bucket is always cleared, even for a zero step.
pub fn apply_forces(input: ForceInput, one: f32, zero: f32, control: u16) -> Option<Velocities> {
    det::checked(|fp| {
        let load = |v| Chain::load(fp, control, v);
        let dt = load(input.dt)?;
        let damping = |amount| {
            let factor = load(one)? - dt * load(amount)?;
            let floor = load(zero)?;
            Some(if factor.compare(floor)?.is_lt() {
                floor.store()
            } else {
                factor.store()
            })
        };
        let weight = (dt * load(input.inverse_mass)?).store();
        let linear = add(fp, input.linear, scaled(fp, weight, input.force));
        let linear = add(fp, linear, scaled(fp, input.dt, input.gravity));
        let linear = scaled(fp, damping(input.linear_damping)?, linear);

        let mut angular = input.angular;
        let clear_torque = !load(input.torque[3])?.compare(load(zero)?)?.is_eq();
        if clear_torque {
            let torque = scaled(fp, input.dt, input.torque);
            // Column dot products of the world impulse give the local impulse.
            let mut local = [0.0; 4];
            for (i, column) in input.basis.iter().enumerate() {
                let xy = fp.add(fp.mul(column[0], torque[0]), fp.mul(column[1], torque[1]));
                local[i] = fp.mul(
                    input.inverse_inertia[i],
                    fp.add(xy, fp.mul(column[2], torque[2])),
                );
            }
            angular = add(fp, angular, rotate(fp, &input.basis, local));
        }
        let angular = scaled(fp, damping(input.angular_damping)?, angular);
        Some(Velocities {
            linear,
            angular,
            clear_torque,
        })
    })
}

/// The solver-side virtuals of a motion (docs/engine/havok-step.md,
/// "Motions"): `vt[+0x0c]` builds its accumulator in the solver buffer,
/// `vt[+0x10]` integrates from it after the solve
/// ([`crate::motion_solve`]). Classes are told apart by those two slots'
/// targets, so a copied vtable is still recognised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolverClass {
    /// Type 2: scalar inertia, world-frame accumulator; the export
    /// restores the solved velocities.
    Sphere,
    /// Type 3: as type 2, the export keeps the masked iteration sums.
    StabilizedSphere,
    /// Type 4: inertia per body axis, body-frame accumulator; the export
    /// restores the solved velocities.
    Box,
    /// Type 5: as type 4, the export keeps the masked iteration sums.
    StabilizedBox,
    /// Type 6: velocities copied in, the pose integrated, no mass.
    Keyframed,
    /// Type 7, the level's: `0x0031a770` zeroes the accumulator,
    /// `0x0031a6a0` writes nothing.
    Fixed,
}

pub const SPHERE_ACCUMULATE: u32 = 0x0031_9df0;
pub const SPHERE_EXPORT: u32 = 0x0031_a0d0;
pub const STABILIZED_SPHERE_EXPORT: u32 = 0x0031_a500;
pub const BOX_ACCUMULATE: u32 = 0x0031_b300;
pub const BOX_EXPORT: u32 = 0x0031_b4c0;
pub const STABILIZED_BOX_EXPORT: u32 = 0x0031_a8c0;
pub const KEYFRAMED_ACCUMULATE: u32 = 0x0031_9430;
pub const KEYFRAMED_EXPORT: u32 = 0x0031_9490;
pub const FIXED_ACCUMULATE: u32 = 0x0031_a770;
pub const FIXED_EXPORT: u32 = 0x0031_a6a0;

impl SolverClass {
    pub const ALL: [Self; 6] =
        [Self::Sphere, Self::StabilizedSphere, Self::Box, Self::StabilizedBox, Self::Keyframed, Self::Fixed];

    pub fn of(accumulate: u32, export: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.slots() == (accumulate, export))
    }

    /// The `vt[+0x0c]` and `vt[+0x10]` targets.
    pub fn slots(self) -> (u32, u32) {
        match self {
            Self::Sphere => (SPHERE_ACCUMULATE, SPHERE_EXPORT),
            Self::StabilizedSphere => (SPHERE_ACCUMULATE, STABILIZED_SPHERE_EXPORT),
            Self::Box => (BOX_ACCUMULATE, BOX_EXPORT),
            Self::StabilizedBox => (BOX_ACCUMULATE, STABILIZED_BOX_EXPORT),
            Self::Keyframed => (KEYFRAMED_ACCUMULATE, KEYFRAMED_EXPORT),
            Self::Fixed => (FIXED_ACCUMULATE, FIXED_EXPORT),
        }
    }
}

/// One accumulator's bytes, `0x80` each. Byte 0 is the kind
/// ([`ACCUMULATOR_KIND`]); byte `+0x0c` is the identity-basis flag the
/// contact builder tests (`0x0031f6be`); `+0x10..+0x80` are seven vectors:
/// linear velocity, angular velocity, inverse inertia with inverse mass in
/// `w`, then the centre and the transposed body basis while the Jacobians
/// are built (the iteration sums during the solve).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accumulator {
    /// `None`: the builder leaves byte `+0x0c` as the buffer held it.
    pub flag: Option<u8>,
    pub vectors: [u128; 7],
}

pub const ACCUMULATOR_BYTES: u32 = 0x80;
/// Byte 0 of every accumulator, the island's leading zeroed one included.
pub const ACCUMULATOR_KIND: u8 = 2;
/// Byte 0 after the last accumulator (`0x0031c31c`).
pub const ACCUMULATOR_END: u8 = 3;

/// `0x0031a770` (type 7) and the island's leading accumulator
/// (`0x0031c2b0`): kind 2, every vector zero, bytes 1 to 15 untouched.
pub fn fixed_accumulator() -> Accumulator {
    Accumulator { flag: None, vectors: [0; 7] }
}

/// `0x00319430` (type 6): the motion's velocities (`+0x40`, `+0x50`) and
/// centre (`+0x70`) copied, no inverse mass or inertia, the identity as
/// the basis, and the flag set.
pub fn keyframed_accumulator(linear: u128, angular: u128, center: u128) -> Accumulator {
    let one = u128::from(1.0f32.to_bits());
    Accumulator {
        flag: Some(1),
        vectors: [linear, angular, 0, center, one, one << 32, one << 64],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damping_clamps_and_consumes_torque_in_the_body_frame() {
        let input = ForceInput {
            dt: 0.5,
            gravity: [0.0, -2.0, 0.0, 0.0],
            linear: [1.0; 4],
            angular: [0.0; 4],
            force: [3.0; 4],
            torque: [2.0, 6.0, 4.0, 1.0],
            inverse_mass: 0.25,
            inverse_inertia: [2.0, 4.0, 1.5, 0.0],
            basis: [
                [0.0, 1.0, 0.0, 0.0],
                [-1.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
            linear_damping: 4.0,
            angular_damping: 0.5,
        };
        let result = apply_forces(input, 1.0, 0.0, 0x27f).unwrap();
        assert!(result.clear_torque);
        assert_eq!(result.linear, [0.0; 4]);
        assert_eq!(result.angular, [3.0, 4.5, 2.25, 0.0]);
    }

    #[test]
    fn each_pair_of_slots_names_one_class() {
        for c in SolverClass::ALL {
            let (a, e) = c.slots();
            assert_eq!(SolverClass::of(a, e), Some(c));
        }
        assert_eq!(SolverClass::of(FIXED_ACCUMULATE, KEYFRAMED_EXPORT), None);
        assert_eq!(SolverClass::of(BOX_ACCUMULATE, SPHERE_EXPORT), None);
    }

    #[test]
    fn a_keyframed_accumulator_carries_the_identity_basis_in_its_last_rows() {
        let a = keyframed_accumulator(1, 2, 3);
        assert_eq!(a.flag, Some(1));
        assert_eq!(a.vectors[..4], [1, 2, 0, 3]);
        // +0x50, +0x64 and +0x78 hold 1.0; every other word is zero.
        let words: Vec<u32> = a.vectors[4..].iter().flat_map(|v| (0..4).map(move |i| (*v >> (32 * i)) as u32)).collect();
        let one = 1.0f32.to_bits();
        assert_eq!(words, [one, 0, 0, 0, 0, one, 0, 0, 0, 0, one, 0]);
    }
}
