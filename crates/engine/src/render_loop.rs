//! Stage 7a of the rasterizer front-end (docs/engine/feature-engine.md, "7.
//! Rasterizer front-end"; the read is in docs/graphics/render-loop.md): the game's
//! render orchestrators as Port-grade Rust that calls the recompiled bodies.
//!
//! Built so far: `render_views` (`0x0002b5d0`), the view loop. It is a
//! state machine rather than a function because every callee is guest code
//! that may block: [`Machine::start`] and [`Machine::resume`] run the port
//! up to its next guest call and return [`Step::Call`] with the CPU set up
//! exactly as the original sets it up at that `call` (every general
//! register, `xmm0`, the stack arguments pushed). The driver makes the call
//! however its world does (the runtime: `call_guest`; the differential
//! test: the interpreter or a stub) and hands the CPU back to `resume`. The
//! callee pops its own stack arguments, as in the original.
//!
//! Port grade means: the same guest memory and the same registers as the
//! original, call for call (the differential test in
//! `tests/render_loop_differential.rs` checks that against the lifted
//! original). Only the return addresses differ: the driver pushes its own.
//! x87 and flag state are not reproduced; none of the callees reads them
//! (the original leaves the x87 stack empty at every call).
//!
//! Every address here is a research point from the read; no game data.

use crate::memory::{GuestMemory, MemFault};

pub mod layer;
pub mod world_walk;
pub mod occlusion;
pub mod scene;
pub mod states;

/// The registers a port sets and reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Regs {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub ebx: u32,
    pub esp: u32,
    pub ebp: u32,
    pub esi: u32,
    pub edi: u32,
    pub xmm0: u128,
    /// Set by `render_scene_passes` before one call; carried through by the
    /// others.
    pub xmm1: u128,
}

/// What the port wants next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Call guest `target` with the registers as they are. `site` is the
    /// original's return address (for logs; the driver pushes its own).
    /// The callee pops `cleanup` bytes of arguments.
    Call {
        target: u32,
        site: u32,
        cleanup: u32,
    },
    /// The port returned: ESP is past the return address and the `ret n`
    /// arguments, EIP is `to`.
    Return { to: u32 },
}

/// The callees of `render_views`, by the original's names where known.
pub mod views {
    /// Called on entry when `0x0047989c` is non-zero, and on exit again.
    pub const DEBUG_BEGIN: u32 = 0x0014_6de0;
    pub const DEBUG_END: u32 = 0x0014_6b80;
    /// Begin-frame setup; EAX points at a 0x18-byte frame record.
    pub const FRAME_BEGIN: u32 = 0x0001_2a50;
    /// A view record's predicate (AL).
    pub const VIEW_TEST: u32 = 0x0002_5820;
    /// Takes the view's `+4` in EAX before the game view.
    pub const VIEW_PREPARE: u32 = 0x0002_0720;
    /// `render_game_view(view, 0)`, stdcall.
    pub const GAME_VIEW: u32 = 0x0002_b790;
    /// The alternate view (`[view] != 0`), stdcall with the view.
    pub const ALT_VIEW: u32 = 0x0002_c790;
    /// End of drawing (two calls).
    pub const END_A: u32 = 0x0001_3b90;
    pub const END_B: u32 = 0x0001_3cd0;
    /// Every callee, for drivers that stub them.
    pub const CALLEES: [u32; 9] = [
        DEBUG_BEGIN,
        FRAME_BEGIN,
        VIEW_TEST,
        VIEW_PREPARE,
        GAME_VIEW,
        ALT_VIEW,
        END_A,
        END_B,
        DEBUG_END,
    ];
    /// Bytes each callee pops.
    pub fn cleanup(target: u32) -> u32 {
        match target {
            GAME_VIEW => 8,
            ALT_VIEW => 4,
            _ => 0,
        }
    }
    /// Size of one view record.
    pub const VIEW_STRIDE: u32 = 0x118;
}

// Globals `render_views` reads and writes (named by use; see the doc).
const DEBUG_ON: u32 = 0x0047_989c;
const FRAME_COUNT: u32 = 0x004b_a034;
const FRAME_TIME: u32 = 0x004b_a040;
const ARG0: u32 = 0x004b_a04c;
const ARG1: u32 = 0x004b_a048;
const ZERO_A: u32 = 0x0048_7b18;
const ZERO_B: u32 = 0x004a_eb1c;
const ZERO_C: u32 = 0x004a_eb28;
const CLEARED: u32 = 0x004c_56c0;
const CURRENT: u32 = 0x004b_a030;
const SCALE_SRC: u32 = 0x0045_dd78;
const SCALE: u32 = 0x004a_eb4c;
const VIEW_INDEX: u32 = 0x0054_d598;
const SKIP_FLAG: u32 = 0x0050_93fc;
const VIEW_LAST: u32 = 0x0052_34c4;
const CAMERA_SRC: u32 = 0x004b_9dac;
const CAMERA_DST: u32 = 0x0048_5b14;

/// Where the port is between calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum At {
    DebugBegin,
    FrameBegin,
    /// A view's predicate returned.
    ViewTest,
    ViewPrepare,
    GameView,
    AltView,
    EndA,
    EndB,
    DebugEnd,
}

/// `render_views` in progress. See the module docs.
#[derive(Clone, Copy, Debug)]
pub struct Machine {
    at: At,
    /// ESP at entry (pointing at the return address).
    entry_esp: u32,
    /// The frame base after `and esp, -8`.
    frame: u32,
    ret_to: u32,
}

const RET_CLEANUP: u32 = 0xc;

impl Machine {
    fn flag_addr(&self) -> u32 {
        self.frame.wrapping_sub(0x19)
    }

    /// Enters `render_views` with the CPU at its first instruction:
    /// EAX the view count, ECX the frame context, stack arguments
    /// `(arg0, arg1, views)`.
    pub fn start(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<(Machine, Step), MemFault> {
        let entry_esp = r.esp;
        let ret_to = m.u32(entry_esp)?;
        // push ebp; mov ebp, esp; and esp, -8; sub esp, 0x1c
        let ebp = entry_esp.wrapping_sub(4);
        m.set_u32(ebp, r.ebp)?;
        let frame = ebp & !7;
        r.ebp = ebp;
        let debug = m.u32(DEBUG_ON)?;
        // push ebx; push esi; push edi
        m.set_u32(frame.wrapping_sub(0x20), r.ebx)?;
        m.set_u32(frame.wrapping_sub(0x24), r.esi)?;
        m.set_u32(frame.wrapping_sub(0x28), r.edi)?;
        r.esp = frame.wrapping_sub(0x28);
        r.edi = 0;
        r.edx = debug;
        r.ebx = r.eax;
        let on = debug != 0;
        r.eax = (r.eax & !0xff) | on as u32;
        r.esi = r.ecx;
        let mut s = Machine {
            at: At::DebugBegin,
            entry_esp,
            frame,
            ret_to,
        };
        m.set_u8(s.flag_addr(), on as u8)?;
        if on {
            return Ok((s, s.call(r, m, views::DEBUG_BEGIN, 0x0002_b5fa)?));
        }
        let step = s.frame_begin(r, m)?;
        Ok((s, step))
    }

    /// Continues after the call [`Step::Call`] asked for returned.
    pub fn resume(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        match self.at {
            At::DebugBegin => self.frame_begin(r, m),
            At::FrameBegin => {
                r.edi = u32::MAX;
                m.set_u32(VIEW_INDEX, r.edi)?;
                if (r.ebx as i32) <= 0 {
                    return self.end(r, m);
                }
                r.esi = m.u32(self.entry_esp.wrapping_add(12))?;
                self.view(r, m)
            }
            At::ViewTest => {
                if r.eax & 0xff != 0 {
                    let skip = m.u8(SKIP_FLAG)?;
                    r.eax = (r.eax & !0xff) | skip as u32;
                    if skip != 0 {
                        return self.next_view(r, m);
                    }
                }
                self.prepare(r, m)
            }
            At::ViewPrepare => {
                r.esp = r.esp.wrapping_sub(8);
                m.set_u32(r.esp, r.esi)?;
                m.set_u32(r.esp.wrapping_add(4), 0)?;
                self.call(r, m, views::GAME_VIEW, 0x0002_b6b9)
            }
            At::GameView => {
                r.eax = m.u32(CAMERA_SRC)?;
                r.ecx = m.u32(CAMERA_SRC + 4)?;
                r.edx = m.u32(CAMERA_SRC + 8)?;
                m.set_u32(VIEW_LAST, r.edi)?;
                m.set_u32(CAMERA_DST, r.eax)?;
                m.set_u32(CAMERA_DST + 4, r.ecx)?;
                m.set_u32(CAMERA_DST + 8, r.edx)?;
                self.next_view(r, m)
            }
            At::AltView => self.next_view(r, m),
            At::EndA => self.call(r, m, views::END_B, 0x0002_b6fc),
            At::EndB => {
                let flag = m.u8(self.flag_addr())?;
                r.eax = (r.eax & !0xff) | flag as u32;
                if flag != 0 {
                    return self.call(r, m, views::DEBUG_END, 0x0002_b709);
                }
                self.leave(r, m)
            }
            At::DebugEnd => self.leave(r, m),
        }
    }

    fn call(
        &mut self,
        _r: &mut Regs,
        _m: &mut dyn GuestMemory,
        target: u32,
        site: u32,
    ) -> Result<Step, MemFault> {
        self.at = match target {
            views::DEBUG_BEGIN => At::DebugBegin,
            views::FRAME_BEGIN => At::FrameBegin,
            views::VIEW_TEST => At::ViewTest,
            views::VIEW_PREPARE => At::ViewPrepare,
            views::GAME_VIEW => At::GameView,
            views::ALT_VIEW => At::AltView,
            views::END_A => At::EndA,
            views::END_B => At::EndB,
            _ => At::DebugEnd,
        };
        Ok(Step::Call {
            target,
            site,
            cleanup: views::cleanup(target),
        })
    }

    /// `0x0002b5fa` to the frame-begin call.
    fn frame_begin(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        let count = m.u32(FRAME_COUNT)?.wrapping_add(1);
        let mut time = u64::from_le_bytes(m.bytes(FRAME_TIME, 8)?.try_into().expect("8 bytes"));
        // fld qword / fstp qword: a signalling NaN comes out quiet.
        let exp = (time >> 52) & 0x7ff;
        let mant = time & ((1 << 52) - 1);
        if exp == 0x7ff && mant != 0 {
            time |= 1 << 51;
        }
        let arg0 = m.u32(self.entry_esp.wrapping_add(4))?;
        let arg1 = m.u32(self.entry_esp.wrapping_add(8))?;
        let scale = m.u32(SCALE_SRC)?;
        r.xmm0 = scale as u128;
        m.set_u32(FRAME_COUNT, count)?;
        m.set_u32(ARG0, arg0)?;
        m.set_u32(ARG1, arg1)?;
        m.set_u32(ZERO_A, 0)?;
        m.set_u32(ZERO_B, 0)?;
        m.set_u32(ZERO_C, 0)?;
        m.write(CLEARED, &[0; 0x40])?;
        // The frame record at [esp + 0x10]: context, 0, time, 0, 0.
        let rec = r.esp.wrapping_add(0x10);
        m.set_u32(rec, r.esi)?;
        m.set_u32(rec + 4, 0)?;
        m.write(rec + 8, &time.to_le_bytes())?;
        m.set_u32(rec + 0x10, 0)?;
        m.set_u32(rec + 0x14, 0)?;
        m.set_u32(CURRENT, r.esi)?;
        m.set_u32(SCALE, scale)?;
        r.eax = rec;
        r.ecx = 0;
        r.edx = 0;
        r.edi = CLEARED + 0x40;
        self.call(r, m, views::FRAME_BEGIN, 0x0002_b67f)
    }

    /// The loop body at `0x0002b690` for the view at ESI.
    fn view(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        if m.u32(r.esi)? != 0 {
            r.esp = r.esp.wrapping_sub(4);
            m.set_u32(r.esp, r.esi)?;
            return self.call(r, m, views::ALT_VIEW, 0x0002_b6e9);
        }
        r.eax = r.esi;
        self.call(r, m, views::VIEW_TEST, 0x0002_b69c)
    }

    fn prepare(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        r.eax = m.u32(r.esi.wrapping_add(4))?;
        self.call(r, m, views::VIEW_PREPARE, 0x0002_b6b1)
    }

    /// `0x0002b6e9`: next record, `dec ebx; jne`.
    fn next_view(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        r.esi = r.esi.wrapping_add(views::VIEW_STRIDE);
        r.ebx = r.ebx.wrapping_sub(1);
        if r.ebx != 0 {
            return self.view(r, m);
        }
        self.end(r, m)
    }

    fn end(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        self.call(r, m, views::END_A, 0x0002_b6f7)
    }

    /// The epilogue from `0x0002b709`.
    fn leave(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        // pop edi; pop esi; (the store); pop ebx; mov esp, ebp; pop ebp:
        // from the stack slots, as the original does.
        r.edi = m.u32(self.frame.wrapping_sub(0x28))?;
        r.esi = m.u32(self.frame.wrapping_sub(0x24))?;
        m.set_u32(CURRENT, 0)?;
        r.ebx = m.u32(self.frame.wrapping_sub(0x20))?;
        r.ebp = m.u32(self.entry_esp.wrapping_sub(4))?;
        r.esp = self.entry_esp.wrapping_add(4 + RET_CLEANUP);
        Ok(Step::Return { to: self.ret_to })
    }
}
