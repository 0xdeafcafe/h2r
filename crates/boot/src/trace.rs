//! Kernel-call trace: every imported kernel function, with its arguments,
//! return address and result, in call order. This is the sequence to
//! compare against the golden (Unicorn) reference run.
//!
//! How: each kernel token's override (the kernel HLE's handler, a boot
//! stopgap, or the runtime's stop) is moved to a shadow address on a page
//! of our own, and a tracer takes its place at the token. The tracer
//! records the call, swaps the guest return word for a continuation token
//! (`ExecCtx::call_guest`, after popping the original word so the stack
//! layout is unchanged) and calls the shadow. When the real handler
//! returns to the token, the continuation records EAX and returns to the
//! original caller. Blocking calls work because the return is still an
//! ordinary RET to that token, whenever it happens.

use std::io::Write;
use std::sync::{Arc, Mutex};

use runtime::{TransferAbi, guest::Permissions};

/// Shadow page for the displaced kernel handlers (below the kernel HLE's
/// internal range, which starts at 0xa1000000).
pub const SHADOW_BASE: u32 = 0xa0ff_0000;

#[derive(Debug, Clone)]
pub struct Call {
    pub seq: u64,
    pub thread: u32,
    pub ordinal: u32,
    pub name: &'static str,
    pub args: Vec<u32>,
    pub ret_pc: u32,
    pub result: Option<u32>,
    /// Decoded names for file and link calls.
    pub detail: String,
}

#[derive(Default)]
pub struct Trace {
    pub calls: Vec<Call>,
    out: Option<std::fs::File>,
}

impl Trace {
    fn write(&mut self, c: &Call) {
        if let Some(f) = &mut self.out {
            let args: Vec<String> = c.args.iter().map(|a| format!("{a:#x}")).collect();
            let result = c.result.map_or("-".into(), |r| format!("{r:#x}"));
            let _ = writeln!(
                f,
                "{}\t{}\t{}\t{}\t{:#x}\t{}\t{}\t{}",
                c.seq,
                c.thread,
                c.ordinal,
                c.name,
                c.ret_pc,
                args.join(","),
                result,
                c.detail
            );
        }
    }
}

pub type Shared = Arc<Mutex<Trace>>;

/// Install the tracer on every imported kernel function. `path` receives a
/// TSV line per completed call (seq, thread, ordinal, name, return pc,
/// args, eax) and per call that never returned (eax `-`), at the end.
pub fn install(rt: &runtime::Runtime, path: Option<&std::path::Path>) -> anyhow::Result<Shared> {
    rt.memory().map_zeroed(SHADOW_BASE, 0x1000, Permissions::RX)?;
    let trace: Shared = Arc::new(Mutex::new(Trace {
        calls: Vec::new(),
        out: path.map(std::fs::File::create).transpose()?,
    }));
    rt.edit_dispatch(|d| {
        for imp in runtime::kernel::FUNCTION_IMPORTS {
            let token = runtime::kernel::function_token(imp.ordinal);
            let shadow = SHADOW_BASE + imp.ordinal * 4;
            let t = Arc::clone(&trace);
            let (ordinal, name, words) = (imp.ordinal, imp.name, imp.stack_words);
            let Some(prev) = d.register_override(token, format!("trace:{name}"), move |cpu, ctx, _| {
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let ret_pc = mem.u32(cpu.esp)?;
                let mut args = vec![cpu.ecx, cpu.edx];
                for i in 0..words as u32 {
                    args.push(mem.u32(cpu.esp.wrapping_add(4 + 4 * i))?);
                }
                let stack = &args[2..];
                let detail = match ordinal {
                    190 | 202 => ansi_at(&mem, stack.get(2).copied(), 4),
                    210 => ansi_at(&mem, stack.first().copied(), 4),
                    67 => format!("{} -> {}", ansi(&mem, stack[0]), ansi(&mem, stack[1])),
                    69 => ansi(&mem, stack[0]),
                    8 => cstr(&mem, stack[0]),
                    _ => String::new(),
                };
                let idx = {
                    let mut tr = t.lock().unwrap();
                    let seq = tr.calls.len() as u64;
                    tr.calls.push(Call { seq, thread: ctx.thread_id, ordinal, name, args, ret_pc, result: None, detail });
                    seq as usize
                };
                // Pop the caller's return word; call_guest pushes the token
                // into the same slot.
                cpu.esp = cpu.esp.wrapping_add(4);
                let t2 = Arc::clone(&t);
                ctx.call_guest(cpu, &mut mem, shadow, move |cpu, _ctx| {
                    let mut tr = t2.lock().unwrap();
                    tr.calls[idx].result = Some(cpu.eax);
                    let c = tr.calls[idx].clone();
                    tr.write(&c);
                    Ok(TransferAbi::ret(ret_pc))
                })
            }) else {
                continue;
            };
            d.restore_override(shadow, prev);
        }
    });
    Ok(trace)
}

/// Write the calls that never returned (the stop, thread exits).
pub fn flush(t: &Shared) {
    let mut tr = t.lock().unwrap();
    let open: Vec<Call> = tr.calls.iter().filter(|c| c.result.is_none()).cloned().collect();
    for c in &open {
        tr.write(c);
    }
}

fn cstr(mem: &runtime::Guest, p: u32) -> String {
    let mut out = Vec::new();
    for i in 0..200 {
        match mem.u8(p.wrapping_add(i)) {
            Ok(0) | Err(_) => break,
            Ok(b) => out.push(b),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A counted ANSI string (len u16, max u16, buffer).
fn ansi(mem: &runtime::Guest, p: u32) -> String {
    let (Ok(len), Ok(buf)) = (mem.u16(p), mem.u32(p.wrapping_add(4))) else { return "?".into() };
    let bytes: Vec<u8> = (0..len as u32).filter_map(|i| mem.u8(buf.wrapping_add(i)).ok()).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The ANSI string pointed to at `off` inside a structure (OBJECT_ATTRIBUTES.ObjectName).
fn ansi_at(mem: &runtime::Guest, p: Option<u32>, off: u32) -> String {
    let Some(p) = p else { return String::new() };
    let root = mem.u32(p).unwrap_or(0);
    match mem.u32(p.wrapping_add(off)) {
        Ok(name) if name != 0 => {
            let n = ansi(mem, name);
            if root != 0 { format!("[root {root:#x}] {n}") } else { n }
        }
        _ => String::new(),
    }
}
