//! The listener's portal table can lack a route. With a route, a distance
//! callback and a live stack slot supply the two components of its response.
use super::*;

const BSP: u32 = ARENA + 0x1800;
const PORTALS: u32 = ARENA + 0x2000;
const POSITION: u32 = ARENA + 0x2800;
const RESPONSE: u32 = ARENA + 0x3000;
const WEIGHTS: u32 = ARENA + 0x3800;

pub(super) fn cases(entry: u32) -> Vec<Case> {
    if entry == 0x2197f0 {
        return (0..9)
            .map(|seed| Case {
                setup: gain,
                reply,
                seed,
            })
            .collect();
    }
    if entry != 0x222150 {
        return Vec::new();
    }
    (0..6).map(|seed| Case { setup, reply, seed }).collect()
}

fn setup(g: &mut Guest, c: &mut Cpu, seed: u32) {
    c.gpr[1] = 2;
    c.gpr[3] = RESPONSE;
    c.gpr[6] = POSITION;
    g.set_u32(0x004e_0348, BSP).unwrap();
    g.set_u32(BSP + 0x60, PORTALS).unwrap();
    g.set_u32(STACK + 4, WEIGHTS).unwrap();
    g.set_u32(STACK + 8, POSITION + 16).unwrap();
    g.set_u32(STACK + 12, seed).unwrap();
    g.set_u32(WEIGHTS + 4 * seed, (0.125 * (seed as f32)).to_bits())
        .unwrap();
    g.write_bytes(RESPONSE, &[0x77; 8], 1).unwrap();
}

fn reply(g: &mut Guest, c: &mut Cpu, target: u32, seed: u32) -> u32 {
    match target {
        0x18cfd0 => {
            let out = g.u32(c.gpr[4] + 4).unwrap();
            // The callback can write the saved ECX slot, which is popped
            // back into the register on the no-route path as well.
            g.set_u32(out, 0x163).unwrap();
            c.gpr[0] = if seed == 0 { u32::MAX } else { 3 };
            4
        }
        0x14b240 => {
            c.xmm[0] =
                u128::from([0.0f32, 0.0, 0.25, 13.75, -1.0, f32::NAN][seed as usize].to_bits());
            0
        }
        _ => panic!("unexpected portal callout {target:#x}"),
    }
}

fn gain(_: &mut Guest, c: &mut Cpu, seed: u32) {
    let bits = [
        0.0f32.to_bits(),
        (-0.0f32).to_bits(),
        1,
        (-4.0f32).to_bits(),
        0.25f32.to_bits(),
        1.0f32.to_bits(),
        4.0f32.to_bits(),
        f32::INFINITY.to_bits(),
        f32::NAN.to_bits(),
    ];
    c.xmm[0] = (77_u128 << 64) | u128::from(bits[seed as usize]);
}
