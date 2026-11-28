//! Guest-first comparison at the two shared unconstrained motion methods.
//! Capture the motion and its inputs before the call, then compare the
//! predicted record, including all untouched fields, when the guest returns.
//! Compare-only: the prediction does not hold the scratch registers retail
//! leaves, so it is never applied in the guest's place (`parity`).
use crate::sim_native::spans::Spans;
use runtime::{CpuAbi, Guest, guest::Access};
use sim_core::det;
use sim_havok::motion::{self, ForceInput, PoseConstants, PoseInput, Vector};

const BODY_LEN: usize = 0x110;
const ANGULAR_MASK: u32 = 0x0041_5120;
const NORMALISE_SCALE: u32 = 0x0045_dbbc;
const IDENTITY: u32 = 0x0045_dbc0;
const NORMALISE_BIAS: u32 = 0x0045_dbd4;
const ZERO: u32 = 0x0045_dbd8;

struct MotionBytes([u8; BODY_LEN]);

impl MotionBytes {
    fn scalar(&self, offset: usize) -> f32 {
        f32::from_le_bytes(self.0[offset..offset + 4].try_into().unwrap())
    }

    fn vector(&self, offset: usize) -> Vector {
        std::array::from_fn(|i| self.scalar(offset + 4 * i))
    }

    fn set_vector(&mut self, offset: usize, vector: Vector) {
        for (bytes, value) in self.0[offset..offset + 16].chunks_exact_mut(4).zip(vector) {
            bytes.copy_from_slice(&value.to_le_bytes());
        }
    }
}

pub struct Prediction {
    address: u32,
    bytes: MotionBytes,
    end_sp: u32,
    saved: [u32; 4],
}

impl Prediction {
    pub fn matches(&self, cpu: &CpuAbi, guest: &Guest) -> bool {
        cpu.esp == self.end_sp
            && self.saved == [cpu.ebx, cpu.ebp, cpu.esi, cpu.edi]
            && guest
                .read::<BODY_LEN>(self.address)
                .is_ok_and(|current| current == self.bytes.0)
    }
}

/// No physical byte in common. Each range is looked up page by page
/// (`sim_native::spans`): a body or frame can cross page mappings.
fn disjoint(a: &Spans, b: &Spans) -> bool {
    !a.overlaps(b)
}

struct Gather<'a> {
    guest: &'a Guest,
    body: Spans,
    stack_frame: Spans,
}

impl<'a> Gather<'a> {
    fn new(cpu: &CpuAbi, guest: &'a Guest, arg_bytes: u32) -> Option<(Self, Prediction)> {
        if !det::checked_env(cpu.mxcsr)
            || !det::x87_env(cpu.x87.control, cpu.x87.status, cpu.x87.tag, 5)
        {
            return None;
        }
        let body = Spans::checked(guest, cpu.ecx, BODY_LEN, Access::Read, 16)?;
        // Includes every nested helper's aligned locals as well as the return and arguments.
        let stack_start = cpu.esp.checked_sub(0x100)?;
        let end_sp = cpu.esp.checked_add(4 + arg_bytes)?;
        let stack_len = (end_sp - stack_start) as usize;
        let stack_frame = Spans::checked(guest, stack_start, stack_len, Access::Read, 1)?;
        if !disjoint(&body, &stack_frame) {
            return None;
        }
        Some((
            Self {
                guest,
                body,
                stack_frame,
            },
            Prediction {
                bytes: MotionBytes(guest.read(cpu.ecx).ok()?),
                address: cpu.ecx,
                saved: [cpu.ebx, cpu.ebp, cpu.esi, cpu.edi],
                end_sp,
            },
        ))
    }

    fn words<const N: usize>(&self, at: u32, alignment: u32) -> Option<[u32; N]> {
        let span = Spans::checked(self.guest, at, 4 * N, Access::Read, alignment)?;
        // An argument can point into the caller's stack above the frame.
        // Reads aliased with stores or with the reserved local frame decline.
        if !disjoint(&self.body, &span) || !disjoint(&self.stack_frame, &span) {
            return None;
        }
        let mut words = [0; N];
        for (i, word) in words.iter_mut().enumerate() {
            *word = self.guest.u32(at.checked_add(i as u32 * 4)?).ok()?;
        }
        Some(words)
    }

    fn scalar(&self, at: u32) -> Option<f32> {
        Some(f32::from_bits(self.words::<1>(at, 1)?[0]))
    }
}

pub fn capture(cpu: &CpuAbi, guest: &Guest) -> Option<Prediction> {
    let (gather, mut prediction) = Gather::new(cpu, guest, 4)?;
    let b = &prediction.bytes;
    let input = PoseInput {
        dt: gather.scalar(guest.u32(cpu.esp + 4).ok()?)?,
        sweep_scale: b.scalar(0x0c),
        local_center: b.vector(0x20),
        orientation: b.vector(0x30),
        linear: b.vector(0x40),
        angular: b.vector(0x50),
        center: b.vector(0x70),
    };
    let constants = PoseConstants {
        angular_mask: gather.words(ANGULAR_MASK, 16)?,
        normalise_bias: gather.scalar(NORMALISE_BIAS)?,
        normalise_scale: gather.scalar(NORMALISE_SCALE)?,
        identity: gather.scalar(IDENTITY)?,
    };
    let pose = motion::integrate_pose(input, constants, cpu.x87.control)?;
    let b = &mut prediction.bytes;
    b.set_vector(0x30, pose.orientation);
    b.set_vector(0x60, pose.previous_center);
    b.set_vector(0x70, pose.center);
    for (i, column) in pose.basis.into_iter().enumerate() {
        b.set_vector(0x80 + 16 * i, column);
    }
    b.set_vector(0xb0, pose.translation);
    b.set_vector(0xc0, pose.saved_linear);
    b.set_vector(0xd0, pose.saved_angular);
    Some(prediction)
}

pub fn capture_forces(cpu: &CpuAbi, guest: &Guest) -> Option<Prediction> {
    let (gather, mut prediction) = Gather::new(cpu, guest, 8)?;
    let b = &prediction.bytes;
    let input = ForceInput {
        dt: gather.scalar(guest.u32(cpu.esp + 4).ok()?)?,
        gravity: gather
            .words::<4>(guest.u32(cpu.esp + 8).ok()?, 16)?
            .map(f32::from_bits),
        linear: b.vector(0x40),
        angular: b.vector(0x50),
        force: b.vector(0xf0),
        torque: b.vector(0x100),
        inverse_mass: b.scalar(0x2c),
        inverse_inertia: b.vector(0xe0),
        basis: [b.vector(0x80), b.vector(0x90), b.vector(0xa0)],
        linear_damping: b.scalar(0x14),
        angular_damping: b.scalar(0x18),
    };
    let result = motion::apply_forces(
        input,
        gather.scalar(IDENTITY)?,
        gather.scalar(ZERO)?,
        cpu.x87.control,
    )?;
    let b = &mut prediction.bytes;
    b.set_vector(0x40, result.linear);
    b.set_vector(0x50, result.angular);
    b.set_vector(0xf0, [0.0; 4]);
    if result.clear_torque {
        b.set_vector(0x100, [0.0; 4]);
    }
    Some(prediction)
}
