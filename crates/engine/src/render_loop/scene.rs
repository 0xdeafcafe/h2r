//! `render_game_view` (`0x0002b790`) and `render_scene_passes`
//! (`0x0002bcd0`) as Port-grade state machines, like [`super::Machine`]
//! (`render_views`): each stops at its next guest call with every general
//! register, `xmm0` and `xmm1` as the original has them there, and the stack
//! arguments pushed (docs/graphics/render-loop.md).
//!
//! Both are written as block machines: the state is the original's address
//! the port continues at (after a call, the call's return address), and
//! [`GameView::resume`] / [`ScenePasses::resume`] run the blocks from there
//! to the next call. Each block is the original's instructions in order,
//! including the partial-register writes (`mov al, ..`), the stack locals and
//! the argument slots the original reuses as locals. Flags are not kept
//! across blocks; every branch reads the value its compare reads.
//!
//! With `render_scene_passes` in Rust the pass identity of every draw is
//! known here: [`ScenePasses::layer`] says which layer the call about to be
//! made belongs to. Every address is a research point from the read; no
//! game data.

use super::{Regs, Step};
use crate::memory::{GuestMemory, MemFault};

/// The callees of `render_game_view`.
pub mod game_view {
    /// stdcall `(out)`, AL whether the view's cached pair was replaced.
    pub const PREPARE: u32 = 0x0002_b720;
    pub const OBSERVER: u32 = 0x0014_b360;
    /// stdcall, four arguments.
    pub const CAMERA: u32 = 0x0012_e5e0;
    pub const PROJECTION: u32 = 0x0002_f970;
    pub const CLUSTER: u32 = 0x0015_4310;
    /// `render_view_scene`: stdcall, 17 arguments.
    pub const VIEW_SCENE: u32 = 0x0002_ba10;
    pub const CALLEES: [u32; 6] = [PREPARE, OBSERVER, CAMERA, PROJECTION, CLUSTER, VIEW_SCENE];
    /// Bytes each callee pops.
    pub fn cleanup(target: u32) -> u32 {
        match target {
            PREPARE => 4,
            CAMERA => 0x10,
            VIEW_SCENE => 0x44,
            _ => 0,
        }
    }
    /// `ret 8`: `(view, unused)`.
    pub const RET_CLEANUP: u32 = 8;
}

/// The callees of `render_scene_passes`.
pub mod scene_passes {
    pub const FRAME_RECORD: u32 = 0x0001_3450;
    /// The two early-outs (AL).
    pub const SKIP_A: u32 = 0x0002_89e0;
    pub const SKIP_B: u32 = 0x0015_9610;
    /// Visibility for the view (EAX the fifth argument).
    pub const VISIBILITY: u32 = 0x0004_0e90;
    /// The layer begin (AL whether it draws), draw and end, EDI the layer.
    pub const LAYER_BEGIN: u32 = 0x0004_4370;
    pub const LAYER_DRAW: u32 = 0x0004_4550;
    pub const LAYER_END: u32 = 0x0004_45d0;
    /// Target and state calls around the base layers.
    pub const TARGET: u32 = 0x0001_4bc0;
    pub const CLEAR: u32 = 0x0001_4b60;
    pub const TARGET_MODE: u32 = 0x0001_5680;
    pub const STATE: u32 = 0x0001_5370;
    /// Called with EAX 0 to 6.
    pub const LAYER_PASS: u32 = 0x0002_c370;
    pub const PASS_335E0: u32 = 0x0003_35e0;
    /// The dynamic-light pass (calls `render_light_passes` per light).
    pub const LIGHTS: u32 = 0x0003_1c80;
    pub const PASS_3EDE0: u32 = 0x0003_ede0;
    pub const PASS_25D50: u32 = 0x0002_5d50;
    pub const PASS_3BA90: u32 = 0x0003_ba90;
    pub const PASS_3C3C0: u32 = 0x0003_c3c0;
    pub const PASS_25DC0: u32 = 0x0002_5dc0;
    pub const PASS_2C340: u32 = 0x0002_c340;
    pub const PASS_C3E90: u32 = 0x000c_3e90;
    pub const PASS_36B50: u32 = 0x0003_6b50;
    pub const PASS_1595D0: u32 = 0x0015_95d0;
    pub const PASS_42B20: u32 = 0x0004_2b20;
    /// Sorted transparents (qsort, then one indirect call per entry).
    pub const TRANSPARENTS: u32 = 0x0001_e370;
    pub const OBJ_155760: u32 = 0x0015_5760;
    pub const OBJ_14DDC0: u32 = 0x0014_ddc0;
    pub const OBJ_14DE70: u32 = 0x0014_de70;
    pub const OBJ_CBD50: u32 = 0x000c_bd50;
    pub const OBJ_2AFA0: u32 = 0x0002_afa0;
    pub const PASS_3FDF0: u32 = 0x0003_fdf0;
    pub const PASS_404B0: u32 = 0x0004_04b0;
    pub const PASS_40870: u32 = 0x0004_0870;
    pub const PASS_2DD30: u32 = 0x0002_dd30;
    pub const PASS_20770: u32 = 0x0002_0770;
    pub const PASS_13870: u32 = 0x0001_3870;
    /// Called on every exit.
    pub const EXIT: u32 = 0x0001_3630;
    pub const CALLEES: [u32; 37] = [
        FRAME_RECORD,
        SKIP_A,
        SKIP_B,
        VISIBILITY,
        LAYER_BEGIN,
        LAYER_DRAW,
        LAYER_END,
        TARGET,
        CLEAR,
        TARGET_MODE,
        STATE,
        LAYER_PASS,
        PASS_335E0,
        LIGHTS,
        PASS_3EDE0,
        PASS_25D50,
        PASS_3BA90,
        PASS_3C3C0,
        PASS_25DC0,
        PASS_2C340,
        PASS_C3E90,
        PASS_36B50,
        PASS_1595D0,
        PASS_42B20,
        TRANSPARENTS,
        OBJ_155760,
        OBJ_14DDC0,
        OBJ_14DE70,
        OBJ_CBD50,
        OBJ_2AFA0,
        PASS_3FDF0,
        PASS_404B0,
        PASS_40870,
        PASS_2DD30,
        PASS_20770,
        PASS_13870,
        EXIT,
    ];
    /// Bytes each callee pops.
    pub fn cleanup(target: u32) -> u32 {
        match target {
            TARGET | PASS_3C3C0 | PASS_13870 => 8,
            CLEAR => 0xc,
            PASS_335E0 | PASS_3BA90 | PASS_42B20 | PASS_3FDF0 | PASS_404B0 => 4,
            _ => 0,
        }
    }
    /// `ret 0x18`: six arguments.
    pub const RET_CLEANUP: u32 = 0x18;
}

// Globals (named by use).
const GV_PAIR_SRC: u32 = 0x0046_86d4;
const GV_VIEW_INDEX: u32 = 0x004b_a050;
const GV_PAIR_A: u32 = 0x004b_a054;
const GV_PAIR_B: u32 = 0x004b_a064;
const GV_OBSERVER: u32 = 0x0046_86cc;
const GV_FAR_MIN: u32 = 0x0045_dbdc;
const OBJECTS: u32 = 0x004e_8c20;
const OBJECT_DATA: u32 = 0x004e_8c24;

const SP_PASS_COUNT: u32 = 0x004b_a03c;
const SP_LAYER_MASK: u32 = 0x004c_1a50;
const SP_LIST: u32 = 0x004c_1a48;
const SP_TARGET_LAYER: u32 = 0x0048_58b4;
const SP_TARGET_SRC: u32 = 0x0048_58b8;
const SP_TARGET_KIND: u32 = 0x0048_5898;
const SP_SKIP_8: u32 = 0x004b_a022;
const SP_FLAG_5C: u32 = 0x0048_575c;
const SP_FLAGS_E0: u32 = 0x004b_9fe0;
const SP_CLEARED: u32 = 0x0050_9430;
const SP_FLAG_88: u32 = 0x004b_9f88;
const SP_COUNT_AC: u32 = 0x004c_19ac;
const SP_PLAYER: u32 = 0x004b_9ed8;
const SP_DATA_300: u32 = 0x004e_0300;
const SP_TAGS: u32 = 0x004e_3b44;

fn al(r: &Regs) -> u8 {
    r.eax as u8
}

fn set_al(r: &mut Regs, v: u8) {
    r.eax = (r.eax & !0xff) | v as u32;
}

fn push(r: &mut Regs, m: &mut dyn GuestMemory, v: u32) -> Result<(), MemFault> {
    r.esp = r.esp.wrapping_sub(4);
    m.set_u32(r.esp, v)
}

fn pop(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    let v = m.u32(r.esp)?;
    r.esp = r.esp.wrapping_add(4);
    Ok(v)
}

/// `movss xmm, [mem]`: the low lane, the rest zeroed.
fn movss(m: &dyn GuestMemory, addr: u32) -> Result<u128, MemFault> {
    Ok(m.u32(addr)? as u128)
}

fn lane(x: u128) -> f32 {
    f32::from_bits(x as u32)
}

/// `render_game_view` in progress.
#[derive(Clone, Copy, Debug)]
pub struct GameView {
    /// The original's address to continue at.
    pc: u32,
    entry_esp: u32,
    ret_to: u32,
}

impl GameView {
    /// Enters `render_game_view` at its first instruction: stdcall
    /// `(view, unused)`.
    pub fn start(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<(GameView, Step), MemFault> {
        let mut s = GameView {
            pc: 0x0002_b790,
            entry_esp: r.esp,
            ret_to: m.u32(r.esp)?,
        };
        let step = s.run(r, m)?;
        Ok((s, step))
    }

    /// Continues after the call [`Step::Call`] asked for returned.
    pub fn resume(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        self.run(r, m)
    }

    fn call(&mut self, target: u32, ret: u32) -> Result<Step, MemFault> {
        self.pc = ret;
        Ok(Step::Call {
            target,
            site: ret,
            cleanup: game_view::cleanup(target),
        })
    }

    fn run(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        use game_view::*;
        loop {
            match self.pc {
                0x0002_b790 => {
                    r.esp = r.esp.wrapping_sub(0x16c);
                    r.eax = m.u32(GV_PAIR_SRC)?;
                    r.ecx = m.u32(r.eax)?;
                    r.edx = m.u32(r.eax.wrapping_add(4))?;
                    r.xmm0 = 0;
                    push(r, m, r.ebx)?;
                    push(r, m, r.ebp)?;
                    r.ebp = m.u32(r.esp.wrapping_add(0x178))?;
                    m.set_u32(r.esp.wrapping_add(0x40), r.ecx)?;
                    r.ecx = m.u32(r.eax.wrapping_add(8))?;
                    m.set_u32(r.esp.wrapping_add(0x44), r.edx)?;
                    r.edx = m.u32(r.eax.wrapping_add(0xc))?;
                    r.eax = m.u32(r.ebp.wrapping_add(4))?;
                    m.set_u32(GV_VIEW_INDEX, r.eax)?;
                    r.eax = m.u32(r.ebp.wrapping_add(4))?;
                    r.eax <<= 2;
                    push(r, m, r.esi)?;
                    m.set_u32(r.esp.wrapping_add(0x50), r.edx)?;
                    r.edx = m.u32(r.eax.wrapping_add(GV_PAIR_B))?;
                    m.set_u32(r.esp.wrapping_add(0x4c), r.ecx)?;
                    r.ecx = m.u32(r.eax.wrapping_add(GV_PAIR_A))?;
                    push(r, m, r.edi)?;
                    r.eax = r.esp.wrapping_add(0x18);
                    m.set_u32(r.esp.wrapping_add(0x1c), r.edx)?;
                    r.ebx = 0;
                    r.edx = r.ebp.wrapping_add(0xc);
                    push(r, m, r.eax)?;
                    r.edi = r.esp.wrapping_add(0x20);
                    m.set_u32(r.esp.wrapping_add(0x28), u32::MAX)?;
                    m.set_u32(r.esp.wrapping_add(0x48), r.xmm0 as u32)?;
                    m.set_u32(r.esp.wrapping_add(0x44), r.ebx)?;
                    m.set_u32(r.esp.wrapping_add(0x1c), r.ecx)?;
                    return self.call(PREPARE, 0x0002_b80b);
                }
                0x0002_b80b => {
                    let replaced = al(r) != 0;
                    r.esi = m.u32(r.esp.wrapping_add(0x18))?;
                    if replaced {
                        r.ecx = m.u32(r.ebp.wrapping_add(4))?;
                        r.eax = m.u32(r.esp.wrapping_add(0x1c))?;
                        m.set_u32(r.ecx.wrapping_mul(4).wrapping_add(GV_PAIR_A), r.esi)?;
                        r.edx = m.u32(r.ebp.wrapping_add(4))?;
                        m.set_u32(r.edx.wrapping_mul(4).wrapping_add(GV_PAIR_B), r.eax)?;
                        m.set_u8(r.esp.wrapping_add(0x20), r.ebx as u8)?;
                    } else {
                        m.set_u8(r.esp.wrapping_add(0x20), 1)?;
                    }
                    r.ecx = m.u32(GV_OBSERVER)?.wrapping_add(4);
                    r.edx = m.u32(r.ecx)?;
                    r.eax = m.u32(r.ecx.wrapping_add(4))?;
                    r.ecx = m.u32(r.ecx.wrapping_add(8))?;
                    m.set_u32(r.esp.wrapping_add(0x28), r.edx)?;
                    m.set_u32(r.esp.wrapping_add(0x30), r.ecx)?;
                    m.set_u8(r.esp.wrapping_add(0x17), r.ebx as u8)?;
                    r.ebx = r.esp.wrapping_add(0x17);
                    r.edi = r.esp.wrapping_add(0x28);
                    r.edx = r.esp.wrapping_add(0x38);
                    r.ecx = r.esi;
                    m.set_u32(r.esp.wrapping_add(0x2c), r.eax)?;
                    return self.call(OBSERVER, 0x0002_b86a);
                }
                0x0002_b86a => {
                    m.set_u8(r.esp.wrapping_add(0x3c), al(r))?;
                    r.ebx = m.u32(r.esp.wrapping_add(0x3c))?;
                    push(r, m, 1)?;
                    push(r, m, r.ebx)?;
                    r.edx = r.ebp.wrapping_add(0x18);
                    push(r, m, r.edx)?;
                    r.eax = r.ebp.wrapping_add(0xc);
                    push(r, m, r.eax)?;
                    r.eax = r.esp.wrapping_add(0x68);
                    r.ecx = r.esi;
                    return self.call(CAMERA, 0x0002_b888);
                }
                0x0002_b888 => {
                    set_al(r, m.u8(r.esp.wrapping_add(0x17))?);
                    if al(r) != 0 {
                        r.eax = m.u32(r.esp.wrapping_add(0x28))?;
                        r.ecx = m.u32(r.esp.wrapping_add(0x2c))?;
                        r.edx = m.u32(r.esp.wrapping_add(0x30))?;
                        m.set_u32(r.esp.wrapping_add(0x5c), r.eax)?;
                        m.set_u32(r.esp.wrapping_add(0x60), r.ecx)?;
                        m.set_u32(r.esp.wrapping_add(0x64), r.edx)?;
                    }
                    set_al(r, m.u8(r.esp.wrapping_add(0x70))?);
                    if al(r) != 0 {
                        r.xmm1 = movss(m, r.ebp.wrapping_add(0x50))?;
                        r.xmm0 = movss(m, r.esp.wrapping_add(0x6c))?;
                        // comiss; jbe: on only when ordered and above.
                        if lane(r.xmm1) > lane(r.xmm0)
                            && lane(r.xmm0) > m.f32(r.ebp.wrapping_add(0x4c))?
                        {
                            m.set_u32(r.ebp.wrapping_add(0x50), r.xmm0 as u32)?;
                        }
                    }
                    r.edi = r.ebp.wrapping_add(0xc);
                    r.esi = r.esp.wrapping_add(0x28);
                    r.eax = r.edi;
                    return self.call(PROJECTION, 0x0002_b8d9);
                }
                0x0002_b8d9 => {
                    r.eax = m.u32(r.ebp.wrapping_add(8))?;
                    if r.eax != u32::MAX {
                        r.ecx = m.u32(OBJECTS)?;
                        r.ecx =
                            m.u32(r.ecx.wrapping_add(r.eax.wrapping_mul(4)).wrapping_add(0xc))?;
                        r.edx = m.u32(OBJECT_DATA)?;
                        r.edx = m.u32(r.edx.wrapping_add(0x44))?;
                        r.ecx &= 0xffff;
                        r.ecx = r.ecx.wrapping_mul(0x21c);
                        r.ecx = m.u32(r.ecx.wrapping_add(r.edx).wrapping_add(0x24))?;
                        r.esi = r.esp.wrapping_add(0x40);
                        m.set_u32(r.esp.wrapping_add(0x24), r.ecx)?;
                        return self.call(CLUSTER, 0x0002_b911);
                    }
                    self.pc = 0x0002_b911;
                }
                0x0002_b911 => {
                    r.xmm0 = movss(m, r.ebp.wrapping_add(0x34))?;
                    // comiss; jbe: skipped unless ordered and above.
                    let above = lane(r.xmm0).partial_cmp(&m.f32(GV_FAR_MIN)?)
                        == Some(std::cmp::Ordering::Greater);
                    if !above {
                        self.pc = 0x0002_b974;
                        continue;
                    }
                    r.edx = r.esp.wrapping_add(0x40);
                    push(r, m, r.edx)?;
                    r.edx = m.u32(r.esp.wrapping_add(0x3c))?;
                    push(r, m, 0)?;
                    r.eax = r.ebp.wrapping_add(0xf4);
                    push(r, m, r.eax)?;
                    r.eax = m.u32(r.esp.wrapping_add(0x30))?;
                    push(r, m, u32::MAX)?;
                    push(r, m, 0)?;
                    r.ecx = r.esp.wrapping_add(0x6c);
                    push(r, m, r.ecx)?;
                    r.ecx = m.u32(r.ebp.wrapping_add(8))?;
                    push(r, m, r.edx)?;
                    push(r, m, r.ebx)?;
                    push(r, m, r.eax)?;
                    r.eax = m.u32(r.esp.wrapping_add(0x44))?;
                    push(r, m, r.ecx)?;
                    r.ecx = m.u32(r.esp.wrapping_add(0x44))?;
                    push(r, m, 0)?;
                    r.edx = r.esp.wrapping_add(0x54);
                    push(r, m, r.edx)?;
                    r.edx = m.u32(r.esp.wrapping_add(0x1b4))?;
                    push(r, m, r.eax)?;
                    push(r, m, r.ecx)?;
                    r.ecx = m.u32(r.ebp.wrapping_add(4))?;
                    push(r, m, r.edx)?;
                    r.edx = m.u32(r.esp.wrapping_add(0x54))?;
                    r.eax = r.ebp.wrapping_add(0x80);
                    push(r, m, r.eax)?;
                    push(r, m, r.ecx)?;
                    r.eax = r.edi;
                    return self.call(VIEW_SCENE, 0x0002_b974);
                }
                0x0002_b974 => {
                    r.edi = pop(r, m)?;
                    r.esi = pop(r, m)?;
                    r.ebp = pop(r, m)?;
                    r.ebx = pop(r, m)?;
                    r.esp = r.esp.wrapping_add(0x16c);
                    debug_assert_eq!(r.esp, self.entry_esp);
                    r.esp = r.esp.wrapping_add(4 + RET_CLEANUP);
                    return Ok(Step::Return { to: self.ret_to });
                }
                pc => unreachable!("render_game_view: no block at {pc:#x}"),
            }
        }
    }
}

/// `render_scene_passes` in progress.
#[derive(Clone, Copy, Debug)]
pub struct ScenePasses {
    pc: u32,
    entry_esp: u32,
    ret_to: u32,
}

impl ScenePasses {
    /// Enters `render_scene_passes` at its first instruction: six stack
    /// arguments, `xmm0` a float it records.
    pub fn start(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<(ScenePasses, Step), MemFault> {
        let mut s = ScenePasses {
            pc: 0x0002_bcd0,
            entry_esp: r.esp,
            ret_to: m.u32(r.esp)?,
        };
        let step = s.run(r, m)?;
        Ok((s, step))
    }

    /// Continues after the call [`Step::Call`] asked for returned.
    pub fn resume(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        self.run(r, m)
    }

    /// The original's return address of the call in progress (or the
    /// address the port continues at): which pass a draw belongs to.
    pub fn at(&self) -> u32 {
        self.pc
    }

    fn call(&mut self, target: u32, ret: u32) -> Result<Step, MemFault> {
        self.pc = ret;
        Ok(Step::Call {
            target,
            site: ret,
            cleanup: scene_passes::cleanup(target),
        })
    }

    /// After a `LAYER_BEGIN` call returning at `here`: `test al, al; je
    /// skip; call LAYER_DRAW` (which returns at `here + 9`, where the block
    /// calls `LAYER_END`).
    fn begun(&mut self, r: &Regs, here: u32, skip: u32) -> Result<Option<Step>, MemFault> {
        if al(r) == 0 {
            self.pc = skip;
            return Ok(None);
        }
        self.call(scene_passes::LAYER_DRAW, here + 0x09).map(Some)
    }

    fn run(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        loop {
            if let Some(step) = self.block(r, m)? {
                return Ok(step);
            }
        }
    }

    /// Runs one block; `None` when it jumped to another block.
    fn block(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Option<Step>, MemFault> {
        use scene_passes::*;
        let call = |s: &mut Self, t, ret| s.call(t, ret).map(Some);
        let jump = |s: &mut Self, to| {
            s.pc = to;
            Ok(None)
        };
        let begun = |s: &mut Self, r: &Regs, here: u32, skip: u32| s.begun(r, here, skip);
        let pc = self.pc;
        match pc {
            0x0002_bcd0 => {
                r.esp = r.esp.wrapping_sub(0xc);
                r.eax = m.u32(SP_PASS_COUNT)?;
                push(r, m, r.ebx)?;
                r.ebx = m.u32(r.esp.wrapping_add(0x28))?;
                r.ecx = 0;
                m.set_u32(r.esp.wrapping_add(4), r.ecx)?;
                push(r, m, r.ebp)?;
                r.ebp = m.u32(r.esp.wrapping_add(0x18))?;
                m.set_u32(r.esp.wrapping_add(0xc), r.ecx)?;
                push(r, m, r.esi)?;
                r.eax = r.eax.wrapping_add(1);
                m.set_u32(r.esp.wrapping_add(0x14), r.ecx)?;
                push(r, m, r.edi)?;
                r.ecx = r.esp.wrapping_add(0x10);
                m.set_u32(SP_PASS_COUNT, r.eax)?;
                m.set_u32(r.esp.wrapping_add(0x10), r.eax)?;
                m.set_u32(r.esp.wrapping_add(0x14), r.ebx)?;
                m.set_u32(r.esp.wrapping_add(0x18), r.xmm0 as u32)?;
                call(self, FRAME_RECORD, 0x0002_bd0f)
            }
            0x0002_bd0f => call(self, SKIP_A, 0x0002_bd14),
            0x0002_bd14 => {
                if al(r) != 0 {
                    return jump(self, 0x0002_c327);
                }
                call(self, SKIP_B, 0x0002_bd21)
            }
            0x0002_bd21 => {
                if al(r) != 0 {
                    return jump(self, 0x0002_c327);
                }
                r.eax = m.u32(r.esp.wrapping_add(0x30))?;
                call(self, VISIBILITY, 0x0002_bd32)
            }
            0x0002_bd32 => {
                if r.ebp == 5 {
                    return jump(self, 0x0002_c327);
                }
                r.edi = 1;
                r.esi = SP_LIST;
                if r.ebp == 1 || r.ebp == 2 {
                    return jump(self, 0x0002_bdb2);
                }
                call(self, LAYER_BEGIN, 0x0002_bd53)
            }
            // Layers 1, 3, 5 and 2 without targets (views other than 1, 2).
            0x0002_bd53 => begun(self, r, 0x0002_bd53, 0x0002_bd61),
            0x0002_bd5c => call(self, LAYER_END, 0x0002_bd61),
            0x0002_bd61 => {
                r.edi = 3;
                call(self, LAYER_BEGIN, 0x0002_bd6b)
            }
            0x0002_bd6b => begun(self, r, 0x0002_bd6b, 0x0002_bd79),
            0x0002_bd74 => call(self, LAYER_END, 0x0002_bd79),
            0x0002_bd79 => {
                r.edi = 5;
                call(self, LAYER_BEGIN, 0x0002_bd83)
            }
            0x0002_bd83 => begun(self, r, 0x0002_bd83, 0x0002_bd91),
            0x0002_bd8c => call(self, LAYER_END, 0x0002_bd91),
            0x0002_bd91 => {
                r.edi = 2;
                call(self, LAYER_BEGIN, 0x0002_bd9b)
            }
            0x0002_bd9b => {
                if al(r) == 0 {
                    return jump(self, 0x0002_c09b);
                }
                call(self, LAYER_DRAW, 0x0002_bda8)
            }
            0x0002_bda8 => call(self, LAYER_END, 0x0002_bdad),
            0x0002_bdad => jump(self, 0x0002_c09b),
            // Layer 1 with its target (views 1 and 2).
            0x0002_bdb2 => {
                if !self.mask_bit(r, m, 1)? {
                    return jump(self, 0x0002_be27);
                }
                push(r, m, 0)?;
                push(r, m, r.edi)?;
                set_al(r, 1);
                m.set_u32(CURRENT_LAYER, r.edi)?;
                m.set_u32(CURRENT_ARG, 0)?;
                m.set_u32(CURRENT_LIST, r.esi)?;
                m.set_u32(SP_TARGET_LAYER, r.edi)?;
                call(self, TARGET, 0x0002_bde7)
            }
            0x0002_bde7 => {
                set_al(r, m.u8(FOG_A)?);
                if al(r) == 0 {
                    set_al(r, m.u8(FOG_B)?);
                    if al(r) == 0 {
                        push(r, m, 0)?;
                        push(r, m, 0)?;
                        push(r, m, 0)?;
                        r.eax = r.edi;
                        return call(self, CLEAR, 0x0002_be06);
                    }
                }
                jump(self, 0x0002_be06)
            }
            0x0002_be06 => {
                r.eax = m.u32(SP_TARGET_KIND)?;
                if (4..=7).contains(&(r.eax as i32)) {
                    r.eax = 0;
                    return call(self, TARGET_MODE, 0x0002_be25);
                }
                r.eax = r.edi;
                call(self, TARGET_MODE, 0x0002_be1c)
            }
            0x0002_be1c | 0x0002_be25 => jump(self, 0x0002_be31),
            0x0002_be27 => {
                m.set_u32(CURRENT_LAYER, 0)?;
                jump(self, 0x0002_be31)
            }
            0x0002_be31 => {
                r.ebx = 1;
                call(self, STATE, 0x0002_be3b)
            }
            0x0002_be3b => {
                set_al(r, m.u8(r.esp.wrapping_add(0x20))?);
                r.ebx = 0;
                if al(r) == r.ebx as u8 {
                    return jump(self, 0x0002_be4f);
                }
                call(self, LAYER_DRAW, 0x0002_be4a)
            }
            0x0002_be4a => call(self, LAYER_END, 0x0002_be4f),
            // Layer 3 (the opaque scene) with its target.
            0x0002_be4f => {
                if !self.mask_bit(r, m, 3)? {
                    m.set_u32(CURRENT_LAYER, r.ebx)?;
                    return jump(self, 0x0002_bebc);
                }
                self.target(r, m, 3)?;
                call(self, TARGET, 0x0002_be88)
            }
            0x0002_be88 => {
                r.eax = m.u32(SP_TARGET_KIND)?;
                if (4..=7).contains(&(r.eax as i32)) {
                    r.eax = 0;
                    return call(self, TARGET_MODE, 0x0002_beaf);
                }
                r.eax = 1;
                call(self, TARGET_MODE, 0x0002_bea1)
            }
            0x0002_bea1 | 0x0002_beaf => {
                m.set_u8(r.esp.wrapping_add(0x20), 1)?;
                jump(self, 0x0002_bebc)
            }
            0x0002_bebc => {
                r.ebx = 1;
                call(self, STATE, 0x0002_bec6)
            }
            0x0002_bec6 => {
                set_al(r, m.u8(r.esp.wrapping_add(0x20))?);
                r.ebx = 0;
                if al(r) == r.ebx as u8 {
                    return jump(self, 0x0002_beda);
                }
                call(self, LAYER_DRAW, 0x0002_bed5)
            }
            0x0002_bed5 => call(self, LAYER_END, 0x0002_beda),
            // Layers 5, 8 and 20: target, mode 0, state 0, draw.
            0x0002_beda => self.target_layer(r, m, 5, false, 0x0002_bf13, 0x0002_bf27),
            0x0002_bf13 => {
                r.eax = m.u32(SP_TARGET_KIND)?;
                r.eax = 0;
                call(self, TARGET_MODE, 0x0002_bf1f)
            }
            0x0002_bf1f => jump(self, 0x0002_bf27),
            0x0002_bf27 => {
                r.ebx = 0;
                call(self, STATE, 0x0002_bf2e)
            }
            0x0002_bf2e => {
                if m.u8(r.esp.wrapping_add(0x20))? == r.ebx as u8 {
                    return jump(self, 0x0002_bf3e);
                }
                call(self, LAYER_DRAW, 0x0002_bf39)
            }
            0x0002_bf39 => call(self, LAYER_END, 0x0002_bf3e),
            0x0002_bf3e => self.target_layer(r, m, 8, true, 0x0002_bf84, 0x0002_bf98),
            0x0002_bf84 => {
                r.eax = m.u32(SP_TARGET_KIND)?;
                r.eax = 0;
                call(self, TARGET_MODE, 0x0002_bf90)
            }
            0x0002_bf90 => jump(self, 0x0002_bf98),
            0x0002_bf98 => {
                r.ebx = 0;
                call(self, STATE, 0x0002_bf9f)
            }
            0x0002_bf9f => {
                if m.u8(r.esp.wrapping_add(0x20))? == r.ebx as u8 {
                    return jump(self, 0x0002_bfaf);
                }
                call(self, LAYER_DRAW, 0x0002_bfaa)
            }
            0x0002_bfaa => call(self, LAYER_END, 0x0002_bfaf),
            0x0002_bfaf => self.target_layer(r, m, 0x14, false, 0x0002_bfe8, 0x0002_bffc),
            0x0002_bfe8 => {
                r.eax = m.u32(SP_TARGET_KIND)?;
                r.eax = 0;
                call(self, TARGET_MODE, 0x0002_bff4)
            }
            0x0002_bff4 => jump(self, 0x0002_bffc),
            0x0002_bffc => {
                r.ebx = 0;
                call(self, STATE, 0x0002_c003)
            }
            0x0002_c003 => {
                set_al(r, m.u8(r.esp.wrapping_add(0x20))?);
                if al(r) == 0 {
                    return jump(self, 0x0002_c015);
                }
                call(self, LAYER_DRAW, 0x0002_c010)
            }
            0x0002_c010 => call(self, LAYER_END, 0x0002_c015),
            0x0002_c015 => {
                r.eax = 0;
                call(self, LAYER_PASS, 0x0002_c01c)
            }
            0x0002_c01c => {
                r.edx = m.u32(r.esp.wrapping_add(0x34))?;
                push(r, m, r.edx)?;
                call(self, PASS_335E0, 0x0002_c026)
            }
            0x0002_c026 => {
                r.edi = 7;
                call(self, LAYER_BEGIN, 0x0002_c030)
            }
            0x0002_c030 => begun(self, r, 0x0002_c030, 0x0002_c03e),
            0x0002_c039 => call(self, LAYER_END, 0x0002_c03e),
            0x0002_c03e => call(self, LIGHTS, 0x0002_c043),
            0x0002_c043 => {
                r.eax = 1;
                call(self, LAYER_PASS, 0x0002_c04d)
            }
            0x0002_c04d => {
                r.edi = 2;
                call(self, LAYER_BEGIN, 0x0002_c057)
            }
            0x0002_c057 => begun(self, r, 0x0002_c057, 0x0002_c065),
            0x0002_c060 => call(self, LAYER_END, 0x0002_c065),
            0x0002_c065 => {
                r.eax = 2;
                call(self, LAYER_PASS, 0x0002_c06f)
            }
            0x0002_c06f => {
                r.eax = 3;
                call(self, LAYER_PASS, 0x0002_c079)
            }
            0x0002_c079 => {
                r.eax = 4;
                call(self, LAYER_PASS, 0x0002_c083)
            }
            0x0002_c083 => {
                r.eax = 5;
                call(self, LAYER_PASS, 0x0002_c08d)
            }
            0x0002_c08d => {
                r.eax = 6;
                call(self, LAYER_PASS, 0x0002_c097)
            }
            0x0002_c097 => {
                r.ebx = m.u32(r.esp.wrapping_add(0x34))?;
                jump(self, 0x0002_c09b)
            }
            0x0002_c09b => {
                if r.ebp == 2 {
                    return jump(self, 0x0002_c164);
                }
                set_al(r, m.u8(SP_FLAG_5C)?);
                if al(r) == 0 {
                    return jump(self, 0x0002_c0cc);
                }
                if r.ebx == 2 {
                    return jump(self, 0x0002_c0ee);
                }
                r.ebx = 2;
                call(self, STATE, 0x0002_c0bc)
            }
            0x0002_c0bc => call(self, PASS_3EDE0, 0x0002_c0c1),
            0x0002_c0c1 => {
                r.ebx = 0;
                call(self, STATE, 0x0002_c0c8)
            }
            0x0002_c0c8 => {
                r.ebx = m.u32(r.esp.wrapping_add(0x34))?;
                jump(self, 0x0002_c0cc)
            }
            0x0002_c0cc => {
                if r.ebx == 2 {
                    return jump(self, 0x0002_c0ee);
                }
                call(self, PASS_25D50, 0x0002_c0d6)
            }
            0x0002_c0d6 => {
                r.edi = 0x10;
                call(self, LAYER_BEGIN, 0x0002_c0e0)
            }
            0x0002_c0e0 => begun(self, r, 0x0002_c0e0, 0x0002_c0ee),
            0x0002_c0e9 => call(self, LAYER_END, 0x0002_c0ee),
            0x0002_c0ee => {
                let skip = m.u8(SP_FLAGS_E0)? & 0x10 != 0;
                m.set_u8(SP_CLEARED, 0)?;
                if skip {
                    push(r, m, 0)?;
                    push(r, m, 0)?;
                    return call(self, PASS_3C3C0, 0x0002_c137);
                }
                push(r, m, r.esi)?;
                r.ecx = 6;
                call(self, PASS_3BA90, 0x0002_c109)
            }
            0x0002_c109 => {
                r.edi = 6;
                m.set_u8(r.esp.wrapping_add(0x20), al(r))?;
                call(self, LAYER_BEGIN, 0x0002_c117)
            }
            0x0002_c117 => begun(self, r, 0x0002_c117, 0x0002_c125),
            0x0002_c120 => call(self, LAYER_END, 0x0002_c125),
            0x0002_c125 => {
                r.eax = m.u32(r.esp.wrapping_add(0x20))?;
                push(r, m, r.eax)?;
                push(r, m, 1)?;
                call(self, PASS_3C3C0, 0x0002_c137)
            }
            0x0002_c137 => {
                if r.ebx == 2 {
                    return jump(self, 0x0002_c164);
                }
                set_al(r, m.u8(SP_FLAG_88)?);
                if al(r) != 0 {
                    r.edx = (r.edx & !0xff) | 1;
                } else {
                    r.eax = m.u32(SP_COUNT_AC)?;
                    if r.eax as i32 <= 0 {
                        return jump(self, 0x0002_c15b);
                    }
                    r.edx &= !0xff;
                }
                r.ecx = (r.ecx & !0xff) | 1;
                call(self, PASS_25DC0, 0x0002_c15b)
            }
            0x0002_c15b => {
                set_al(r, m.u8(r.esp.wrapping_add(0x28))?);
                call(self, PASS_2C340, 0x0002_c164)
            }
            0x0002_c164 => call(self, PASS_C3E90, 0x0002_c169),
            0x0002_c169 => call(self, PASS_36B50, 0x0002_c16e),
            0x0002_c16e => {
                r.edi = 0xf;
                call(self, LAYER_BEGIN, 0x0002_c178)
            }
            0x0002_c178 => begun(self, r, 0x0002_c178, 0x0002_c186),
            0x0002_c181 => call(self, LAYER_END, 0x0002_c186),
            0x0002_c186 => call(self, PASS_1595D0, 0x0002_c18b),
            0x0002_c18b => {
                push(r, m, 0xffff_fff3)?;
                call(self, PASS_42B20, 0x0002_c192)
            }
            0x0002_c192 => {
                let bl = m.u8(r.esp.wrapping_add(0x24))?;
                r.ebx = (r.ebx & !0xff) | bl as u32;
                if bl == 0 {
                    return jump(self, 0x0002_c19f);
                }
                call(self, TRANSPARENTS, 0x0002_c19f)
            }
            0x0002_c19f => {
                if r.ebp == 2 {
                    return jump(self, 0x0002_c327);
                }
                r.edi = m.u32(r.esp.wrapping_add(0x34))?;
                if r.edi == 2 {
                    return jump(self, 0x0002_c1d0);
                }
                set_al(r, m.u8(SP_FLAG_88)?);
                if al(r) != 0 {
                    return jump(self, 0x0002_c1d0);
                }
                r.eax = m.u32(SP_COUNT_AC)?;
                r.edx = (r.edx & !0xff) | 1;
                let cl = if r.eax as i32 <= 0 { 1 } else { 0 };
                r.ecx = (r.ecx & !0xff) | cl;
                call(self, PASS_25DC0, 0x0002_c1d0)
            }
            0x0002_c1d0 => {
                r.esi = m.u32(SP_PLAYER)?;
                m.set_u8(FOG_B, 1)?;
                if r.esi == u32::MAX {
                    return jump(self, 0x0002_c2ab);
                }
                r.eax = r.esi;
                call(self, OBJ_155760, 0x0002_c1ed)
            }
            0x0002_c1ed => {
                if r.eax != 0 {
                    return jump(self, 0x0002_c2ab);
                }
                r.ecx = r.esi;
                call(self, OBJ_14DDC0, 0x0002_c1fc)
            }
            0x0002_c1fc => {
                if al(r) == 0 {
                    return jump(self, 0x0002_c2ab);
                }
                call(self, OBJ_14DE70, 0x0002_c209)
            }
            0x0002_c209 => {
                r.ecx = m.u32(OBJECT_DATA)?;
                r.edx = m.u32(r.ecx.wrapping_add(0x44))?;
                r.eax &= 0xffff;
                r.eax = r.eax.wrapping_mul(0x21c);
                r.eax = m.u32(r.eax.wrapping_add(r.edx).wrapping_add(0x2c))?;
                if r.eax == u32::MAX {
                    return jump(self, 0x0002_c2ab);
                }
                r.ecx = m.u32(SP_DATA_300)?;
                r.esi = m.u32(r.ecx.wrapping_add(0x44))?;
                r.ecx = r.eax & 0xffff;
                r.edx = r.ecx.wrapping_mul(3);
                r.ecx = m.u32(r.esi.wrapping_add(r.edx.wrapping_mul(4)).wrapping_add(8))?;
                // movsx cx, byte: the low word only.
                let b = m.u8(r.ecx.wrapping_add(0x212))? as i8 as i16 as u16;
                r.ecx = (r.ecx & 0xffff_0000) | b as u32;
                call(self, OBJ_CBD50, 0x0002_c24f)
            }
            0x0002_c24f => {
                if r.eax == u32::MAX {
                    return jump(self, 0x0002_c2ab);
                }
                r.eax &= 0xffff;
                r.edx = r.eax.wrapping_mul(3);
                r.esi = m.u32(r.esi.wrapping_add(r.edx.wrapping_mul(4)).wrapping_add(8))?;
                r.eax = m.u32(r.esi)?;
                if r.eax == u32::MAX {
                    return jump(self, 0x0002_c2ab);
                }
                r.ecx = m.u32(SP_TAGS)?;
                r.xmm0 = 0;
                r.eax &= 0xffff;
                r.eax <<= 4;
                r.ecx = m.u32(r.eax.wrapping_add(r.ecx).wrapping_add(8))?;
                r.xmm1 = movss(m, r.ecx.wrapping_add(0x314))?;
                r.edx = r.ecx.wrapping_add(0x314);
                // ucomiss; lahf; test ah, 0x44; jnp: skip when equal to 0
                // (ordered). `lahf` puts the flags in AH.
                r.eax = lahf(r.eax, lane(r.xmm1));
                if lane(r.xmm1) == 0.0 {
                    return jump(self, 0x0002_c2ab);
                }
                r.xmm1 = movss(m, r.ecx.wrapping_add(0x318))?;
                r.eax = lahf(r.eax, lane(r.xmm1));
                if lane(r.xmm1) == 0.0 {
                    return jump(self, 0x0002_c2ab);
                }
                r.eax = r.edx;
                call(self, OBJ_2AFA0, 0x0002_c2ab)
            }
            0x0002_c2ab => {
                r.esi = m.u32(r.esp.wrapping_add(0x30))?;
                push(r, m, 0)?;
                r.eax = r.esi;
                call(self, PASS_3FDF0, 0x0002_c2b8)
            }
            0x0002_c2b8 => {
                push(r, m, r.esi)?;
                call(self, PASS_404B0, 0x0002_c2be)
            }
            0x0002_c2be => {
                push(r, m, 1)?;
                r.eax = r.esi;
                call(self, PASS_3FDF0, 0x0002_c2c7)
            }
            0x0002_c2c7 => {
                if r.ebx as u8 == 0 {
                    return jump(self, 0x0002_c2d0);
                }
                call(self, PASS_40870, 0x0002_c2d0)
            }
            0x0002_c2d0 => {
                let two = r.edi == 2;
                m.set_u8(FOG_B, 0)?;
                if two {
                    return jump(self, 0x0002_c2e9);
                }
                set_al(r, m.u8(r.esp.wrapping_add(0x28))?);
                if al(r) == 0 {
                    return jump(self, 0x0002_c2e9);
                }
                call(self, PASS_2DD30, 0x0002_c2e9)
            }
            0x0002_c2e9 => {
                r.ebx = m.u32(r.esp.wrapping_add(0x2c))?;
                if r.ebx as u8 == 0 {
                    return jump(self, 0x0002_c310);
                }
                if m.u32(r.esp.wrapping_add(0x34))? == 2 {
                    return jump(self, 0x0002_c327);
                }
                r.edi = 0x11;
                call(self, LAYER_BEGIN, 0x0002_c302)
            }
            0x0002_c302 => begun(self, r, 0x0002_c302, 0x0002_c310),
            0x0002_c30b => call(self, LAYER_END, 0x0002_c310),
            0x0002_c310 => {
                if m.u32(r.esp.wrapping_add(0x34))? == 2 {
                    return jump(self, 0x0002_c327);
                }
                call(self, PASS_20770, 0x0002_c31c)
            }
            0x0002_c31c => {
                r.edx = m.u32(r.esp.wrapping_add(0x28))?;
                push(r, m, r.ebx)?;
                push(r, m, r.edx)?;
                call(self, PASS_13870, 0x0002_c327)
            }
            0x0002_c327 => call(self, EXIT, 0x0002_c32c),
            0x0002_c32c => {
                r.edi = pop(r, m)?;
                r.esi = pop(r, m)?;
                r.ebp = pop(r, m)?;
                r.ebx = pop(r, m)?;
                r.esp = r.esp.wrapping_add(0xc);
                debug_assert_eq!(r.esp, self.entry_esp);
                r.esp = r.esp.wrapping_add(4 + RET_CLEANUP);
                Ok(Some(Step::Return { to: self.ret_to }))
            }
            pc => unreachable!("render_scene_passes: no block at {pc:#x}"),
        }
    }

    /// `mov eax, [mask]; shr eax, n; and al, 1; mov [esp+0x20], al`: whether
    /// layer `n` is on; the byte goes to the reused argument slot.
    fn mask_bit(
        &mut self,
        r: &mut Regs,
        m: &mut dyn GuestMemory,
        n: u32,
    ) -> Result<bool, MemFault> {
        r.eax = m.u32(SP_LAYER_MASK)? >> n;
        set_al(r, al(r) & 1);
        m.set_u8(r.esp.wrapping_add(0x20), al(r))?;
        Ok(al(r) != 0)
    }

    /// The base-layer prologue: `mov eax, [0x4858b8]; push ebx; mov
    /// [0x4858b4], eax; push eax; mov al, 1;` and the current layer, its
    /// argument (EBX) and list (ESI).
    fn target(
        &mut self,
        r: &mut Regs,
        m: &mut dyn GuestMemory,
        layer: u32,
    ) -> Result<(), MemFault> {
        r.eax = m.u32(SP_TARGET_SRC)?;
        push(r, m, r.ebx)?;
        m.set_u32(SP_TARGET_LAYER, r.eax)?;
        push(r, m, r.eax)?;
        set_al(r, 1);
        m.set_u32(CURRENT_LAYER, layer)?;
        m.set_u32(CURRENT_ARG, r.ebx)?;
        m.set_u32(CURRENT_LIST, r.esi)?;
        Ok(())
    }

    /// Layers 5, 8 and 20 (`0x0002beda`, `0x0002bf3e`, `0x0002bfaf`): the
    /// mask bit (and for 8 the byte at `0x004ba022`), then the target, or
    /// the current layer set to EBX and on to the state call at `skip`.
    fn target_layer(
        &mut self,
        r: &mut Regs,
        m: &mut dyn GuestMemory,
        layer: u32,
        gated: bool,
        ret: u32,
        skip: u32,
    ) -> Result<Option<Step>, MemFault> {
        let mut on = self.mask_bit(r, m, layer)?;
        if on && gated {
            set_al(r, m.u8(SP_SKIP_8)?);
            m.set_u8(r.esp.wrapping_add(0x20), al(r))?;
            on = al(r) != r.ebx as u8;
        }
        if !on {
            m.set_u32(CURRENT_LAYER, r.ebx)?;
            self.pc = skip;
            return Ok(None);
        }
        self.target(r, m, layer)?;
        self.call(scene_passes::TARGET, ret).map(Some)
    }
}

use super::layer::{CURRENT_ARG, CURRENT_LAYER, CURRENT_LIST, FOG_A, FOG_B};

/// AH after `ucomiss x, 0; lahf`: SF ZF AF PF CF in bits 7, 6, 4, 2, 0, bit
/// 1 always set; `ucomiss` clears SF and AF.
fn lahf(eax: u32, x: f32) -> u32 {
    let (zf, pf, cf) = if x.is_nan() {
        (1, 1, 1)
    } else if x == 0.0 {
        (1, 0, 0)
    } else if x < 0.0 {
        (0, 0, 1)
    } else {
        (0, 0, 0)
    };
    let ah = (zf << 6) | (pf << 2) | 2 | cf;
    (eax & !0xff00) | (ah << 8)
}
