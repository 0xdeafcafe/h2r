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

pub struct Trace {
    /// The most recent calls in memory, oldest first (at most `limit`).
    pub calls: std::collections::VecDeque<Call>,
    /// Calls traced so far; the next call's `seq`.
    pub total: u64,
    /// Calls pushed out of `calls` before they returned, by `seq` (at most
    /// `limit`: calls that never return, such as thread exits, would
    /// otherwise pile up here for the whole run).
    evicted_open: std::collections::BTreeMap<u64, Call>,
    /// Open calls dropped from `evicted_open`, oldest first, to keep it
    /// bounded; their results, if they ever come, are not recorded.
    pub abandoned: u64,
    limit: usize,
    /// The TSV, buffered: an unbuffered `write` per kernel call was 2.8%
    /// of a gameplay frame. [`flush`] writes the open calls and flushes it
    /// at the end of a run; a crash loses at most the buffer, and the
    /// in-memory `calls` still hold the last ones for the report.
    out: Option<std::io::BufWriter<std::fs::File>>,
}

impl Trace {
    fn push(&mut self, mut c: Call) -> u64 {
        let seq = self.total;
        self.total += 1;
        c.seq = seq;
        self.calls.push_back(c);
        while self.calls.len() > self.limit.max(1) {
            if let Some(old) = self.calls.pop_front()
                && old.result.is_none()
            {
                self.evicted_open.insert(old.seq, old);
                if self.evicted_open.len() > self.limit.max(1) {
                    self.evicted_open.pop_first();
                    self.abandoned += 1;
                }
            }
        }
        seq
    }

    /// Record `seq`'s EAX and write its line.
    fn finish(&mut self, seq: u64, eax: u32) {
        let front = self.calls.front().map_or(u64::MAX, |c| c.seq);
        let c = if seq >= front {
            self.calls.get_mut((seq - front) as usize).map(|c| {
                c.result = Some(eax);
                c.clone()
            })
        } else {
            self.evicted_open.remove(&seq).map(|mut c| {
                c.result = Some(eax);
                c
            })
        };
        if let Some(c) = c {
            self.write(&c);
        }
    }

    fn write(&mut self, c: &Call) {
        if let Some(f) = &mut self.out {
            let _ = write_line(f, c);
        }
    }
}

/// One TSV row: seq, thread, ordinal, name, return pc, args, EAX (`-` if
/// the call never returned), detail.
fn write_line(w: &mut impl Write, c: &Call) -> std::io::Result<()> {
    write!(w, "{}\t{}\t{}\t{}\t{:#x}\t", c.seq, c.thread, c.ordinal, c.name, c.ret_pc)?;
    for (i, a) in c.args.iter().enumerate() {
        if i > 0 {
            w.write_all(b",")?;
        }
        write!(w, "{a:#x}")?;
    }
    match c.result {
        Some(r) => write!(w, "\t{r:#x}")?,
        None => w.write_all(b"\t-")?,
    }
    writeln!(w, "\t{}", c.detail)
}

pub type Shared = Arc<Mutex<Trace>>;

/// Install the tracer on every imported kernel function. `path` receives a
/// TSV line per completed call (seq, thread, ordinal, name, return pc,
/// args, eax) and per call that never returned (eax `-`), at the end.
pub fn install(rt: &runtime::Runtime, path: Option<&std::path::Path>) -> anyhow::Result<Shared> {
    install_bounded(rt, path, usize::MAX)
}

/// [`install`] keeping only the last `limit` calls in memory, plus up to
/// `limit` older ones still open (beyond that the oldest open calls are
/// dropped and counted in [`Trace::abandoned`]); the TSV, if any, still
/// gets every completed call. For long runs.
pub fn install_bounded(rt: &runtime::Runtime, path: Option<&std::path::Path>, limit: usize) -> anyhow::Result<Shared> {
    rt.memory().map_zeroed(SHADOW_BASE, 0x1000, Permissions::RX)?;
    let trace: Shared = Arc::new(Mutex::new(Trace {
        calls: Default::default(),
        total: 0,
        evicted_open: Default::default(),
        abandoned: 0,
        limit,
        out: path.map(std::fs::File::create).transpose()?.map(|f| std::io::BufWriter::with_capacity(1 << 16, f)),
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
                let seq = t.lock().unwrap().push(Call {
                    seq: 0,
                    thread: ctx.thread_id,
                    ordinal,
                    name,
                    args,
                    ret_pc,
                    result: None,
                    detail,
                });
                // Pop the caller's return word; call_guest pushes the token
                // into the same slot.
                cpu.esp = cpu.esp.wrapping_add(4);
                let t2 = Arc::clone(&t);
                ctx.call_guest(cpu, &mut mem, shadow, move |cpu, _ctx| {
                    t2.lock().unwrap().finish(seq, cpu.eax);
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

/// Write the calls that never returned (the stop, thread exits), then
/// flush the TSV to disk.
pub fn flush(t: &Shared) {
    let mut tr = t.lock().unwrap();
    let mut open: Vec<Call> = tr.evicted_open.values().cloned().collect();
    open.extend(tr.calls.iter().filter(|c| c.result.is_none()).cloned());
    for c in &open {
        tr.write(c);
    }
    if let Some(f) = &mut tr.out
        && let Err(e) = f.flush()
    {
        eprintln!("trace: writing the kernel-call TSV: {e}");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn call(ordinal: u32) -> Call {
        Call { seq: 0, thread: 0, ordinal, name: "x", args: vec![], ret_pc: 0, result: None, detail: String::new() }
    }

    fn bounded(limit: usize) -> Trace {
        Trace { calls: Default::default(), total: 0, evicted_open: Default::default(), abandoned: 0, limit, out: None }
    }

    #[test]
    fn a_bounded_trace_keeps_the_newest_calls_and_every_open_one() {
        let mut t = bounded(3);
        let blocked = t.push(call(1)); // never returns while the others run
        for i in 2..=10 {
            let s = t.push(call(i));
            t.finish(s, i);
        }
        assert_eq!(t.total, 10);
        assert_eq!(t.calls.iter().map(|c| c.seq).collect::<Vec<_>>(), [7, 8, 9]);
        assert!(t.calls.iter().all(|c| c.result == Some(c.ordinal)));
        // The evicted open call still completes by its seq.
        assert_eq!(t.evicted_open.keys().copied().collect::<Vec<_>>(), [blocked]);
        t.finish(blocked, 0x55);
        assert!(t.evicted_open.is_empty());
    }

    #[test]
    fn calls_that_never_return_stay_bounded() {
        // Thread exits never reach their continuation.
        let mut t = bounded(4);
        for i in 0..20 {
            t.push(call(i));
        }
        assert_eq!(t.calls.len(), 4);
        assert_eq!(t.evicted_open.len(), 4);
        assert_eq!(t.abandoned, 12);
        // The newest evicted ones are the ones kept, and still complete.
        assert_eq!(t.evicted_open.keys().copied().collect::<Vec<_>>(), [12, 13, 14, 15]);
        t.finish(15, 1);
        // An abandoned call's late return is ignored.
        t.finish(0, 1);
        assert_eq!(t.evicted_open.len(), 3);
    }

    #[test]
    fn the_tsv_is_buffered_and_complete_after_flush() {
        let path = std::env::temp_dir().join(format!("h2-trace-test-{}.tsv", std::process::id()));
        let file = std::fs::File::create(&path).unwrap();
        let mut t = bounded(2);
        t.out = Some(std::io::BufWriter::with_capacity(1 << 16, file));
        let open = t.push(Call { args: vec![0x10, 0x2a], detail: "d:\\x".into(), ..call(7) });
        for i in 0..5 {
            let s = t.push(call(i));
            t.finish(s, i + 0x100);
        }
        // Nothing reached the file yet: one write per call is what this saves.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        let shared: Shared = Arc::new(Mutex::new(t));
        flush(&shared);
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 6, "{text}");
        assert_eq!(lines[0], "1\t0\t0\tx\t0x0\t\t0x100\t");
        assert_eq!(lines[4], "5\t0\t4\tx\t0x0\t\t0x104\t");
        // The call that never returned comes last, with no EAX.
        assert_eq!(lines[5], format!("{open}\t0\t7\tx\t0x0\t0x10,0x2a\t-\td:\\x"));
        // The report's in-memory tail is untouched by the file.
        let t = shared.lock().unwrap();
        assert_eq!(t.calls.iter().map(|c| c.seq).collect::<Vec<_>>(), [4, 5]);
        assert_eq!(t.total, 6);
    }

    #[test]
    fn unbounded_keeps_everything() {
        let mut t = bounded(usize::MAX);
        for i in 0..100 {
            t.push(call(i));
        }
        assert_eq!(t.calls.len(), 100);
        t.finish(42, 7);
        assert_eq!(t.calls[42].result, Some(7));
    }
}
