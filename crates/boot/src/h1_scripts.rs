//! Halo 1 script builtins the recompiled engine serves
//! (docs/halo1/scripts.md, "Serving missing builtins").
//!
//! The converter compiles a call of one of [`h1builtins::SERVED`] to a call
//! of a Halo 2 slot no retail script calls, whose retail evaluator is the
//! shared discarded stub. This feature makes the call real:
//!
//! - **The descriptor patch.** Once per run, at game activation, the slot's
//!   declared return type becomes `passthrough`: the retail return path
//!   (`0x209ae0`) skips the cast for a passthrough callee (`cmp dx,3; je`),
//!   so the raw EAX flows through whatever the call node's parent expects.
//!   Retail behaviour for every other builtin of the slot's shape (a
//!   discarded stub returns 0) is unchanged, and no retail script calls
//!   these slots at all.
//! - **The return hook.** A `before` on [`HS_RETURN`](engine::points::HS_RETURN)
//!   (`0x209ae0`, where every builtin and script return lands, with EAX the
//!   value and ECX the thread datum): when the thread's frame names a
//!   served call, the hook reads the call's arguments off the atom nodes
//!   (the slot's stub evaluates nothing, and the converter only serves
//!   calls whose arguments are atoms), runs the implementation registered
//!   for the builtin, and puts its value in EAX before the retail body
//!   writes it through and pops the frame. Without one, the served
//!   fallback ([`h1builtins::Served::done`]) is returned, so a wait on the
//!   call ends as the converter's stub policy made it.
//!
//! Guest layout the hook walks (all **E** in docs/halo1/scripts.md and
//! docs/engine/hs_vm.md): the thread array at `0x4F9384` (elements at the
//! header's `+0x44`, `0x418` each; the thread's current frame at `+0x10`;
//! the frame's expression datum at `+4`), the node array at `0x4F9394`
//! (elements at `+0x44`, `0x14` each), and the scenario globals' cells at
//! `0x4F9380` (elements at the header's `+0x44`, `0x8` each, the cell at
//! `+4`; `hs_new_map` allocates them in table order).
//!
//! **Families.** A lane that owns a family of Halo 1 builtins registers its
//! implementations here ([`implement`]) and claims a slot in
//! `crates/h1builtins` (the converter serves the calls from the same
//! table). The body gets the arguments as 32-bit HaloScript values and
//! guest memory to read and write; it returns the value in the call's own
//! type. Implementations must not call guest code: the hook runs inside
//! the interpreter's dispatch, and a guest call from there has no
//! continuation yet (docs/engine/hs_vm.md, "Next steps", the adapter
//! contract's suspension).

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};

use engine::GuestMemory;
use engine::points::{GAME_ACTIVATE, HS_RETURN};
use runtime::CpuAbi;

/// The feature's id (`features.h1-scripts`).
pub const FEATURE: &str = "h1-scripts";

/// The guest addresses the hook and the patch work on, with the evidence in
/// the module doc.
pub mod addr {
    /// The builtin descriptor pointer table (`0x004744e0`, 911 entries).
    pub const FUNCTION_TABLE: u32 = 0x0047_44e0;
    /// The shared discarded-stub evaluator every served slot's descriptor
    /// names (`mov ecx,[esp+8]; xor eax,eax; call 0x209ae0; ret 0xc`).
    pub const NULL_STUB: u32 = h1builtins::NULL_STUB_EVALUATE;
    /// The return path every builtin and script return flows through (the
    /// `HS_RETURN` point's guest address).
    pub const RETURN_PATH: u32 = 0x0020_9ae0;
    /// The script thread array.
    pub const THREADS: u32 = 0x004f_9384;
    /// The syntax node array.
    pub const NODES: u32 = 0x004f_9394;
    /// The scenario globals' cells.
    pub const GLOBAL_CELLS: u32 = 0x004f_9380;
    /// The game globals: the current tick at `+8` (the interpreter's own
    /// wake comparison reads it, `0x00209902`).
    pub const GAME_GLOBALS: u32 = 0x0051_0c54;
    /// A datum array's elements sit at its header's `+0x44`.
    pub const ELEMENTS: u32 = 0x44;
    /// Bytes per script thread record.
    pub const THREAD_LEN: u32 = 0x418;
    /// Bytes per syntax node.
    pub const NODE_LEN: u32 = 0x14;
    /// The thread's current frame, a pointer.
    pub const THREAD_FRAME: u32 = 0x10;
    /// The frame's expression, a node datum.
    pub const FRAME_EXPR: u32 = 0x4;
}

/// One implementation: the arguments as 32-bit values (Halo 2 encoding, in
/// the parameter types the table declares) and guest memory, returning the
/// value in the call's own type.
pub type Implementation =
    Arc<dyn Fn(&[u32], &mut dyn GuestMemory) -> u32 + Send + Sync + 'static>;

fn registry() -> &'static RwLock<BTreeMap<&'static str, Implementation>> {
    static R: OnceLock<RwLock<BTreeMap<&'static str, Implementation>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(BTreeMap::new()))
}

/// Registers the implementation of a served builtin by its Halo 1 name.
/// Family lanes call this at boot, before the feature builds; a name no
/// served entry carries is refused.
pub fn implement(name: &str, f: Implementation) -> Result<(), String> {
    if h1builtins::by_name(name).is_none() {
        return Err(format!("{name}: not a served builtin (h1builtins::SERVED)"));
    }
    registry()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(name_static(name), f);
    Ok(())
}

/// The table is static; its names live for the program's life.
fn name_static(name: &str) -> &'static str {
    Box::leak(name.to_owned().into_boxed_str())
}

/// The feature. [`H1Scripts::install`] adds it to the registry.
#[derive(Default)]
pub struct H1Scripts;

impl feature::Feature for H1Scripts {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(FEATURE, "Halo 1 script builtins the engine serves")
            .doc("docs/halo1/scripts.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        b.raw()
            .before(&GAME_ACTIVATE, "patch the served slots", move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                patch_descriptors(&mut *mem);
                Ok(feature::Flow::Continue)
            });
        b.raw()
            .before(&HS_RETURN, "serve Halo 1 builtins", move |c| {
                serve(c.cpu, c.ctx);
                Ok(feature::Flow::Continue)
            });
        Ok(())
    }
}

/// Writes `passthrough` over each served slot's declared return type, so
/// the retail return path passes the hook's raw EAX through uncast. The
/// slot's evaluator stays the shared discarded stub, as retail wrote it,
/// and a slot whose shape differs is left alone and named, so a wrong
/// entry in the table cannot shadow a builtin that is real here.
fn patch_descriptors(mem: &mut dyn GuestMemory) {
    for s in h1builtins::SERVED {
        let descriptor = match mem.u32(addr::FUNCTION_TABLE + u32::from(s.slot) * 4) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("h1-scripts: slot {} unreadable ({e}); {} unserved", s.slot, s.name);
                continue;
            }
        };
        match mem.u32(descriptor + 4) {
            Ok(addr::NULL_STUB) => {}
            Ok(other) => {
                eprintln!(
                    "h1-scripts: slot {}'s evaluator is {other:#x}, not the discarded stub; {} unserved",
                    s.slot, s.name
                );
                continue;
            }
            Err(e) => {
                eprintln!("h1-scripts: slot {}'s descriptor unreadable ({e}); {} unserved", s.slot, s.name);
                continue;
            }
        }
        if let Err(e) = mem.write(
            descriptor,
            &(h1builtins::PASSTHROUGH).to_le_bytes(),
        ) {
            eprintln!("h1-scripts: slot {} unwritable ({e}); {} unserved", s.slot, s.name);
        }
    }
}

/// The return hook: EAX the value the callee produced, ECX the thread. When
/// the thread's frame names a served call, EAX becomes the implementation's
/// value (or the fallback), encoded for the call node's own type.
fn serve(cpu: &mut CpuAbi, ctx: &mut runtime::ExecCtx) {
    let thread = cpu.ecx;
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let Some(node) = call_node(&*mem, thread) else {
        return;
    };
    let Some(served) = h1builtins::by_slot(node.opcode) else {
        return;
    };
    let mut args = Vec::with_capacity(served.params.len());
    if !argument_values(&*mem, node.head, served.params.len(), &mut args) {
        // An argument the hook cannot read: the fallback, so a wait on the
        // call still ends.
        cpu.eax = encode(served.done, node.value_type);
        return;
    }
    cpu.eax = encode(
        match registry()
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(served.name)
        {
            Some(f) => f(&args, &mut *mem),
            None => encode(served.done, node.value_type),
        },
        node.value_type,
    );
}

/// The frame's call: its opcode, its own type, and its head atom's datum.
struct Call {
    opcode: u16,
    value_type: u16,
    head: u32,
}

/// The current frame's expression as a call node, when it is one.
fn call_node(mem: &dyn GuestMemory, thread: u32) -> Option<Call> {
    let threads = mem
        .u32(mem.u32(addr::THREADS).ok()? + addr::ELEMENTS)
        .ok()?;
    let frame = mem
        .u32(threads + (thread & 0xffff) * addr::THREAD_LEN + addr::THREAD_FRAME)
        .ok()?;
    let expr = mem.u32(frame + addr::FRAME_EXPR).ok()?;
    let nodes = mem
        .u32(mem.u32(addr::NODES).ok()? + addr::ELEMENTS)
        .ok()?;
    let node = nodes + (expr & 0xffff) * addr::NODE_LEN;
    let flags = mem.u16(node + 6).ok()?;
    if flags & 1 != 0 || flags & 2 != 0 {
        return None; // An atom, or a call of another script.
    }
    Some(Call {
        opcode: mem.u16(node + 2).ok()?,
        value_type: mem.u16(node + 4).ok()?,
        head: mem.u32(node + 16).ok()?,
    })
}

/// Reads `want` argument values off the head's sibling chain of atoms: a
/// literal's bits, or a scenario global's cell. False when the chain falls
/// short or an argument is not an atom (the converter never emits those for
/// a served call; a map that grew one is refused there).
fn argument_values(mem: &dyn GuestMemory, head: u32, want: usize, out: &mut Vec<u32>) -> bool {
    let nodes = match mem.u32(addr::NODES).and_then(|h| mem.u32(h + addr::ELEMENTS)) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let mut next = match mem.u32(nodes + (head & 0xffff) * addr::NODE_LEN + 8) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let cells = mem
        .u32(addr::GLOBAL_CELLS)
        .and_then(|h| mem.u32(h + addr::ELEMENTS))
        .ok();
    for _ in 0..want {
        if next == u32::MAX {
            return false;
        }
        let node = nodes + (next & 0xffff) * addr::NODE_LEN;
        let flags = match mem.u16(node + 6) {
            Ok(f) => f,
            Err(_) => return false,
        };
        if flags & 1 == 0 {
            return false; // Not an atom: the stub would not have run it.
        }
        let data = match mem.u32(node + 16) {
            Ok(d) => d,
            Err(_) => return false,
        };
        if flags & 4 != 0 && data & 0x8000 == 0 {
            // A scenario global: its cell. The engine globals (bit 15) the
            // levels never name; zero stands in for one.
            let Some(cells) = cells else {
                return false;
            };
            match mem.u32(cells + (data & 0xffff) * 8 + 4) {
                Ok(v) => out.push(v),
                Err(_) => return false,
            }
        } else {
            out.push(data);
        }
        match mem.u32(node + 8) {
            Ok(n) => next = n,
            Err(_) => return false,
        }
    }
    next == u32::MAX
}

/// A value in the call node's own type, as the retail return path would
/// leave it after the cast the passthrough patch removes: booleans in the
/// low byte (the comparisons read one), narrower integers in their low
/// half, everything else raw.
fn encode(value: u32, ty: u16) -> u32 {
    match ty {
        h1builtins::VOID => 0,
        h1builtins::BOOLEAN => u32::from(value as u8 != 0),
        h1builtins::SHORT | h1builtins::STRING => value & 0xffff,
        _ => value,
    }
}

impl H1Scripts {
    /// Adds the feature to `registry`, with this lane's own served builtin:
    /// `game_time`, the ticks since the game started, which a30's
    /// extraction timeouts read.
    pub fn install(registry: &mut feature::Registry) -> anyhow::Result<bool> {
        let added = registry
            .add(&Self)
            .map_err(|e| anyhow::anyhow!("h1-scripts: {e}"))?;
        if added {
            if let Err(e) = implement(
                "game_time",
                Arc::new(|_args, mem| {
                    mem.u32(addr::GAME_GLOBALS)
                        .and_then(|g| mem.u32(g + 8))
                        .unwrap_or(0)
                }),
            ) {
                eprintln!("h1-scripts: {e}");
            }
        }
        Ok(added)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value keeps its type's width, as the retail path leaves it.
    #[test]
    fn values_encode_for_their_type() {
        assert_eq!(encode(0x1234_0056, h1builtins::BOOLEAN), 1);
        assert_eq!(encode(2, h1builtins::BOOLEAN), 1);
        assert_eq!(encode(0, h1builtins::BOOLEAN), 0);
        assert_eq!(encode(0x1234_0056, h1builtins::SHORT), 0x56);
        assert_eq!(encode(0x1234_0056, h1builtins::VOID), 0);
        assert_eq!(encode(0x1234_0056, h1builtins::LONG), 0x1234_0056);
    }

    /// Implementations land in the registry by served name; a name no
    /// served entry carries is refused.
    #[test]
    fn implementations_register_by_served_name() {
        assert!(implement("no_such_builtin", Arc::new(|_, _| 0)).is_err());
        assert!(!registry()
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key("no_such_builtin"));
    }
}
