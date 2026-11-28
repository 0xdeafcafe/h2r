//! The layer draw (docs/graphics/layer-draw.md): `0x00044550`, which draws the
//! current layer, and `0x0003bc30`, its loop over the layer's entries. The
//! entry loop is a Port-grade state machine ([`Entries`]): it stops at its
//! next guest call with every register as the original has it there. It is
//! not placed in the game; tests drive the loop through it (the native
//! lights' split test, and `tests/layer_differential.rs` against the lifted
//! original). The Rust layer draw itself was removed with its
//! `impl.render.layer_draw` switch (docs/graphics/render-loop.md).

use super::{Regs, Step};
use guest_layout::memory::{GuestMemory, MemFault};

/// A byte set to 1 at every layer draw.
pub const DRAW_FLAG: u32 = 0x0046_70bc;
/// ESI for the layer setup.
pub const SETUP_CONTEXT: u32 = 0x0048_5b48;
/// The layers `0x00044550` draws (bit per layer).
pub const DRAWN_LAYERS: u32 = 0x00bf_fdee;
/// Planar fog (layer 16) is drawn when `[0x0048574c] == 3` or either fog
/// byte is set.
pub const FOG_A: u32 = 0x0048_5a75;
pub const FOG_B: u32 = 0x0048_5a76;

/// Callees of the entry loop.
pub const LAYER_ENTRIES: u32 = 0x0003_bc30;
pub const LAYER_BUILD: u32 = 0x0003_bab0;

/// One entry the build writes.
pub const ENTRY_SIZE: u32 = 0x18;
/// One handler record in `[list + 0x14]`.
pub const RECORD_SIZE: u32 = 0x18;
/// The entry loop's frame (`_chkstk`'s EAX).
pub const ENTRIES_FRAME: u32 = 0xf004;
/// Bytes a handler pops: seven stdcall arguments.
pub const HANDLER_CLEANUP: u32 = 0x1c;

/// `0x0003bc30` in progress: EDI the list, EBX the layer, one stack
/// argument, `ret 4`.
#[derive(Clone, Copy, Debug)]
pub struct Entries {
    at: EntriesAt,
    entry_esp: u32,
    ret_to: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntriesAt {
    Build,
    Handler,
}

impl Entries {
    fn buffer(&self) -> u32 {
        self.entry_esp.wrapping_sub(0xf000)
    }
    /// The loop counter's slot (`[esp+8]` in the loop).
    fn counter(&self) -> u32 {
        self.entry_esp.wrapping_sub(ENTRIES_FRAME)
    }

    pub fn start(r: &mut Regs, m: &mut dyn GuestMemory) -> Result<(Entries, Step), MemFault> {
        let e = r.esp;
        let ret_to = m.u32(e)?;
        // mov eax, 0xf004; call _chkstk: it saves ECX, probes every page of
        // the new frame from the top, and leaves EAX its return address.
        let chk_ret = 0x0003_bc3a;
        m.set_u32(e.wrapping_sub(4), chk_ret)?;
        m.set_u32(e.wrapping_sub(8), r.ecx)?;
        for page in 1..=ENTRIES_FRAME / 0x1000 {
            m.u32(e.wrapping_sub(page * 0x1000))?;
        }
        let top = e.wrapping_sub(ENTRIES_FRAME);
        m.u32(top)?;
        m.set_u32(top.wrapping_sub(4), chk_ret)?;
        // push ebp; mov ebp, [arg]
        m.set_u32(top.wrapping_sub(4), r.ebp)?;
        r.ebp = m.u32(e.wrapping_add(4))?;
        let s = Entries {
            at: EntriesAt::Build,
            entry_esp: e,
            ret_to,
        };
        r.eax = s.buffer();
        // push buffer, layer, arg, list
        let sp = top.wrapping_sub(4);
        for (i, v) in [r.eax, r.ebx, r.ebp, r.edi].into_iter().enumerate() {
            m.set_u32(sp.wrapping_sub(4 * (i as u32 + 1)), v)?;
        }
        r.esp = sp.wrapping_sub(16);
        Ok((
            s,
            Step::Call {
                target: LAYER_BUILD,
                site: 0x0003_bc4f,
                cleanup: 16,
            },
        ))
    }

    pub fn resume(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        match self.at {
            EntriesAt::Build => {
                if (r.eax as i32) <= 0 {
                    return self.leave(r, m);
                }
                // push esi; esi = the buffer; the count to [esp+8]
                let sp = self.entry_esp.wrapping_sub(ENTRIES_FRAME + 8);
                m.set_u32(sp, r.esi)?;
                r.esp = sp;
                r.esi = self.buffer();
                m.set_u32(self.counter(), r.eax)?;
                self.entry(r, m)
            }
            EntriesAt::Handler => self.next(r, m),
        }
    }

    /// The loop body at `0x0003bc60` for the entry at ESI.
    fn entry(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        loop {
            let kind = m.u16(r.esi)? as u32;
            r.edx = m.u32(r.edi.wrapping_add(0x14))?;
            r.ecx = kind.wrapping_mul(3);
            r.eax = r.edx.wrapping_add(r.ecx.wrapping_mul(8));
            r.ecx = m.u32(r.esi.wrapping_add(0x10))?;
            if m.u32(r.ecx.wrapping_add(0x100))? != u32::MAX {
                break;
            }
            if !self.advance(r, m)? {
                return self.finish(r, m);
            }
        }
        let rec = r.eax;
        r.edx = m.u32(rec.wrapping_add(0xc))?;
        r.ecx = m.u8(rec.wrapping_add(0x14))? as u32;
        let args_ecx = r.ecx;
        let args_edx = r.edx;
        r.edx = m.u8(rec.wrapping_add(0x15))? as u32;
        r.ecx = m.u32(rec.wrapping_add(8))?;
        // push esi, edx(+0xc), ecx(+0x14), ebx, edx(+0x15), ebp, ecx(+8)
        let args = [r.esi, args_edx, args_ecx, r.ebx, r.edx, r.ebp, r.ecx];
        for v in args {
            r.esp = r.esp.wrapping_sub(4);
            m.set_u32(r.esp, v)?;
        }
        let handler = m.u32(rec.wrapping_add(4))?;
        self.at = EntriesAt::Handler;
        Ok(Step::Call {
            target: handler,
            site: 0x0003_bc90,
            cleanup: HANDLER_CLEANUP,
        })
    }

    /// `0x0003bc90`: the counter from its slot, next entry; false at the end.
    fn advance(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<bool, MemFault> {
        r.eax = m.u32(self.counter())?;
        r.esi = r.esi.wrapping_add(ENTRY_SIZE);
        r.eax = r.eax.wrapping_sub(1);
        m.set_u32(self.counter(), r.eax)?;
        Ok(r.eax != 0)
    }

    fn next(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        if self.advance(r, m)? {
            return self.entry(r, m);
        }
        self.finish(r, m)
    }

    /// `pop esi` after the loop.
    fn finish(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        r.esi = m.u32(self.entry_esp.wrapping_sub(ENTRIES_FRAME + 8))?;
        self.leave(r, m)
    }

    /// `pop ebp; add esp, 0xf004; ret 4`.
    fn leave(&mut self, r: &mut Regs, m: &mut dyn GuestMemory) -> Result<Step, MemFault> {
        r.ebp = m.u32(self.entry_esp.wrapping_sub(ENTRIES_FRAME + 4))?;
        r.esp = self.entry_esp.wrapping_add(8);
        Ok(Step::Return { to: self.ret_to })
    }
}
