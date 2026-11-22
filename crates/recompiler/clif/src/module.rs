//! The module's two objects besides the shards, built with Cranelift like
//! them, so the fast tier compiles no C (docs/lifter/cranelift-tier.md
//! section 2):
//!
//! - [`registry`]: `h2gen_module_abi_version` and `h2gen_module` with its
//!   entry and resume tables, as emit's C registry lays them out, with no
//!   module features.
//! - [`bridge`]: the import table the handlers call through
//!   ([`IMPORTS`]) and the installers the hosts call: boot's
//!   `h2boot_install_v3`, the test host's `h2bridge_install_v3` and
//!   `h2bridge_layout_v3`, `h2x87bridge_install`, and the fast tier's own
//!   `h2clif_install_v1`.

use std::collections::{BTreeSet, HashMap};

use cranelift_codegen::Context;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{self as cir, AbiParam, BlockArg, InstBuilder, MemFlagsData, Signature, types};
use cranelift_codegen::isa::OwnedTargetIsa;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};
use emit::{EntryKind, Owner};

use crate::abi::*;
use crate::{Error, error, stem, symbol};

/// What the registry and the bridge export, `(name, is data)`: the
/// symbols the hosts look up (a Windows DLL lists them; the other
/// platforms export every global symbol).
pub const EXPORTS: [(&str, bool); 7] = [
    ("h2gen_module", true),
    ("h2gen_module_abi_version", false),
    ("h2boot_install_v3", false),
    ("h2bridge_install_v3", false),
    ("h2bridge_layout_v3", false),
    ("h2x87bridge_install", false),
    ("h2clif_install_v1", false),
];

/// `H2GenEntry`, `H2GenResume` and `H2GenModule` (h2abi.h,
/// runtime::generated): sizes and the pointer fields' offsets.
const ENTRY_SIZE: usize = 24;
const ENTRY_HANDLER: usize = 8;
const ENTRY_PROBE: usize = 16;
const RESUME_SIZE: usize = 12;
const MODULE_SIZE: usize = 32;
const MODULE_ENTRIES: usize = 16;
const MODULE_RESUMES: usize = 24;

/// `H2SHIM_UNMODELLED`: what a fast-tier helper the host never installed
/// answers.
const SHIM_UNMODELLED: u32 = 12;

fn object(isa: &OwnedTargetIsa, name: &str) -> Result<ObjectModule, Error> {
    let builder = ObjectBuilder::new(isa.clone(), name, default_libcall_names()).map_err(|e| error(None, e))?;
    Ok(ObjectModule::new(builder))
}

fn emit(m: ObjectModule) -> Result<Vec<u8>, Error> {
    m.finish().emit().map_err(|e| error(None, e))
}

fn signature(m: &ObjectModule, params: &[cir::Type], ret: Option<cir::Type>) -> Signature {
    let mut s = m.make_signature();
    s.params.extend(params.iter().map(|t| AbiParam::new(*t)));
    s.returns.extend(ret.map(AbiParam::new));
    s
}

fn declare(m: &mut ObjectModule, name: &str, linkage: Linkage, sig: &Signature) -> Result<FuncId, Error> {
    m.declare_function(name, linkage, sig).map_err(|e| error(None, format!("{name}: {e}")))
}

fn declare_data(m: &mut ObjectModule, name: &str, linkage: Linkage, writable: bool) -> Result<DataId, Error> {
    m.declare_data(name, linkage, writable, false).map_err(|e| error(None, format!("{name}: {e}")))
}

fn define_data(m: &mut ObjectModule, id: DataId, d: &DataDescription) -> Result<(), Error> {
    m.define_data(id, d).map_err(|e| error(None, e))
}

/// Define `id` (signature `sig`) with the body `body` writes, entering at
/// a fresh block it is switched to.
fn define(
    m: &mut ObjectModule,
    id: FuncId,
    sig: &Signature,
    body: impl FnOnce(&mut ObjectModule, &mut FunctionBuilder<'_>, &[cir::Value]),
) -> Result<(), Error> {
    let fc = m.isa().frontend_config();
    let mut ctx = Context::new();
    ctx.func.signature = sig.clone();
    let mut fbc = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbc);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let params = b.block_params(entry).to_vec();
        body(m, &mut b, &params);
        b.seal_all_blocks();
        b.finalize(fc);
    }
    m.define_function(id, &mut ctx).map_err(|e| error(None, format!("{e:?}")))
}

fn return_const(b: &mut FunctionBuilder<'_>, v: u32) {
    let v = b.ins().iconst(types::I32, i64::from(v));
    b.ins().return_(&[v]);
}

/// The registration object: `uint32_t h2gen_module_abi_version(void)` and
/// `const H2GenModule h2gen_module`, whose entry rows (sorted by address,
/// then a zero row) point at each owner's handler and its shard's probe,
/// and whose resume rows (sorted, then a zero row) name the owner. The
/// same tables emit's C registry declares, with [`features`]: the fast tier
/// counts no accesses and makes no direct calls.
pub fn registry(isa: &OwnedTargetIsa, owners: &[Owner]) -> Result<Vec<u8>, Error> {
    let mut m = object(isa, "h2clif_registry")?;
    let probe_sig = signature(&m, &[], Some(types::I32));
    let version = declare(&mut m, "h2gen_module_abi_version", Linkage::Export, &probe_sig)?;
    define(&mut m, version, &probe_sig, |_, b, _| return_const(b, ABI_VERSION))?;

    let handler_sig = crate::lower::handler_signature(isa.default_call_conv());
    let mut handlers: HashMap<u32, FuncId> = HashMap::new();
    for o in owners {
        let e = o.func.entry;
        handlers.insert(e, declare(&mut m, &symbol(e), Linkage::Import, &handler_sig)?);
    }
    let stems: BTreeSet<String> = owners.iter().map(|o| stem(o.func.entry)).collect();
    let mut probes: HashMap<String, FuncId> = HashMap::new();
    for st in stems {
        let id = declare(&mut m, &format!("h2gen_abi_version_{st}"), Linkage::Import, &probe_sig)?;
        probes.insert(st, id);
    }

    let mut entries: Vec<(u32, EntryKind, u32)> =
        owners.iter().flat_map(|o| o.entries.iter().map(move |(a, k)| (*a, *k, o.func.entry))).collect();
    entries.sort();
    let mut d = DataDescription::new();
    let mut refs: HashMap<FuncId, cir::FuncRef> = HashMap::new();
    let mut fref = |m: &ObjectModule, d: &mut DataDescription, id: FuncId| {
        *refs.entry(id).or_insert_with(|| m.declare_func_in_data(id, d))
    };
    let mut bytes = vec![0u8; (entries.len() + 1) * ENTRY_SIZE];
    for (i, (addr, kind, owner)) in entries.iter().enumerate() {
        let o = i * ENTRY_SIZE;
        bytes[o..o + 4].copy_from_slice(&addr.to_le_bytes());
        bytes[o + 4..o + 8].copy_from_slice(&(*kind as u32).to_le_bytes());
        let h = fref(&m, &mut d, handlers[owner]);
        d.write_function_addr((o + ENTRY_HANDLER) as u32, h);
        let p = fref(&m, &mut d, probes[&stem(*owner)]);
        d.write_function_addr((o + ENTRY_PROBE) as u32, p);
    }
    d.define(bytes.into_boxed_slice());
    d.set_align(8);
    let entries_id = declare_data(&mut m, "h2clif_entries", Linkage::Local, false)?;
    define_data(&mut m, entries_id, &d)?;

    let mut resumes: Vec<(u32, u32)> =
        owners.iter().flat_map(|o| o.resumes.iter().map(move |pc| (*pc, o.func.entry))).collect();
    resumes.sort();
    let mut bytes = vec![0u8; (resumes.len() + 1) * RESUME_SIZE];
    for (i, (pc, owner)) in resumes.iter().enumerate() {
        let o = i * RESUME_SIZE;
        // pc, owner, start: a resume starts its owner at the continuation.
        bytes[o..o + 4].copy_from_slice(&pc.to_le_bytes());
        bytes[o + 4..o + 8].copy_from_slice(&owner.to_le_bytes());
        bytes[o + 8..o + 12].copy_from_slice(&pc.to_le_bytes());
    }
    let mut d = DataDescription::new();
    d.define(bytes.into_boxed_slice());
    d.set_align(4);
    let resumes_id = declare_data(&mut m, "h2clif_resumes", Linkage::Local, false)?;
    define_data(&mut m, resumes_id, &d)?;

    let mut bytes = vec![0u8; MODULE_SIZE];
    bytes[0..4].copy_from_slice(&ABI_VERSION.to_le_bytes());
    bytes[4..8].copy_from_slice(&(entries.len() as u32).to_le_bytes());
    bytes[8..12].copy_from_slice(&(resumes.len() as u32).to_le_bytes());
    bytes[12..16].copy_from_slice(&features(owners).to_le_bytes());
    let mut d = DataDescription::new();
    let e = m.declare_data_in_data(entries_id, &mut d);
    d.write_data_addr(MODULE_ENTRIES as u32, e, 0);
    let r = m.declare_data_in_data(resumes_id, &mut d);
    d.write_data_addr(MODULE_RESUMES as u32, r, 0);
    d.define(bytes.into_boxed_slice());
    d.set_align(8);
    let module = declare_data(&mut m, "h2gen_module", Linkage::Export, false)?;
    define_data(&mut m, module, &d)?;
    emit(m)
}

/// The module's feature word: no clock and no call features, and
/// `H2GEN_FEATURE_IMPRECISE_FAULTS` (128) when the passes ran with imprecise
/// faults, as emit's registry declares it
/// (docs/runtime/block-clock-imprecise-faults.md).
pub fn features(owners: &[Owner]) -> u32 {
    if owners.iter().any(|o| o.opt.imprecise_faults) { IMPRECISE_FAULTS } else { 0 }
}

/// `H2GEN_FEATURE_IMPRECISE_FAULTS` in `h2gen.h`.
const IMPRECISE_FAULTS: u32 = 128;

/// The bridge object: the import table [`IMPORTS`] ([`SLOTS`] pointers,
/// hidden in the module) and the hosts' installers, which fill it.
///
/// - `uint32_t h2boot_install_v3(const H2BootBridge *, size_t)` (boot,
///   `recomp::install_bridge`) and `uint32_t h2bridge_install_v3(const
///   H2Bridge *, size_t)` (emit's test host): 0 unless the table is there
///   and its size is the bridge's; else the eight [`CORE`] callbacks are
///   copied and the answer is 1.
/// - `size_t h2bridge_layout_v3(uint32_t slot)`: the test host's layout
///   check, `slot * 8` up to the size at slot 16, then 0.
/// - `void h2x87bridge_install(void *const *, uint32_t n)` and
///   `void h2clif_install_v1(void *const *, uint32_t n)`: the first `n`
///   x87 helpers ([`X87_COUNT`] at most) and fast-tier helpers
///   ([`CLIF_COUNT`]: `ir::interp::sse_helper`). A slot left null, or
///   never installed, answers `H2X87_UNMODELLED` (x87, as the C bridge's
///   forwarders do) or `H2SHIM_UNMODELLED`, so a handler stops rather than
///   calling through null.
pub fn bridge(isa: &OwnedTargetIsa) -> Result<Vec<u8>, Error> {
    let mut m = object(isa, "h2clif_bridge")?;
    let (q, i) = (types::I64, types::I32);

    let stub_sig = signature(&m, &[], Some(i));
    let x87_stub = declare(&mut m, "h2clif_x87_unmodelled", Linkage::Local, &stub_sig)?;
    define(&mut m, x87_stub, &stub_sig, |_, b, _| return_const(b, X87_UNMODELLED))?;
    let clif_stub = declare(&mut m, "h2clif_helper_missing", Linkage::Local, &stub_sig)?;
    define(&mut m, clif_stub, &stub_sig, |_, b, _| return_const(b, SHIM_UNMODELLED))?;

    let table = declare_data(&mut m, IMPORTS, Linkage::Hidden, true)?;
    let mut d = DataDescription::new();
    d.define(vec![0u8; SLOTS * 8].into_boxed_slice());
    d.set_align(8);
    let x = m.declare_func_in_data(x87_stub, &mut d);
    for k in 0..X87_COUNT {
        d.write_function_addr(((X87_SLOT + k) * 8) as u32, x);
    }
    let c = m.declare_func_in_data(clif_stub, &mut d);
    for k in 0..CLIF_COUNT {
        d.write_function_addr(((SSE_SLOT + k) * 8) as u32, c);
    }
    define_data(&mut m, table, &d)?;

    let install_sig = signature(&m, &[q, q], Some(i));
    for (name, size) in [("h2boot_install_v3", BOOT_BRIDGE_SIZE), ("h2bridge_install_v3", TEST_BRIDGE_SIZE)] {
        let id = declare(&mut m, name, Linkage::Export, &install_sig)?;
        define(&mut m, id, &install_sig, |m, b, p| {
            let (src, n) = (p[0], p[1]);
            let null = b.ins().icmp_imm_u(IntCC::Equal, src, 0i64);
            let wrong = b.ins().icmp_imm_u(IntCC::NotEqual, n, size as i64);
            let refuse = b.ins().bor(null, wrong);
            let (no, yes) = (b.create_block(), b.create_block());
            b.ins().brif(refuse, no, &[], yes, &[]);
            b.switch_to_block(no);
            return_const(b, 0);
            b.switch_to_block(yes);
            let gv = m.declare_data_in_func(table, b.func);
            let t = b.ins().symbol_value(q, gv);
            for k in 0..CORE.len() {
                let off = (k * 8) as i32;
                let v = b.ins().load(q, MemFlagsData::new().with_notrap(), src, off);
                b.ins().store(MemFlagsData::new().with_notrap(), v, t, off);
            }
            return_const(b, 1);
        })?;
    }

    let layout_sig = signature(&m, &[i], Some(q));
    let layout = declare(&mut m, "h2bridge_layout_v3", Linkage::Export, &layout_sig)?;
    define(&mut m, layout, &layout_sig, |_, b, p| {
        // Every field is a pointer: field k at k * 8, and the size, 16 * 8,
        // in slot 16.
        let s = b.ins().uextend(q, p[0]);
        let off = b.ins().ishl_imm_u(s, 3i64);
        let known = b.ins().icmp_imm_u(IntCC::UnsignedLessThanOrEqual, s, (TEST_BRIDGE_SIZE / 8) as i64);
        let zero = b.ins().iconst(q, 0);
        let r = b.ins().select(known, off, zero);
        b.ins().return_(&[r]);
    })?;

    let helpers_sig = signature(&m, &[q, i], None);
    for (name, first, count, stub) in
        [("h2x87bridge_install", X87_SLOT, X87_COUNT, x87_stub), ("h2clif_install_v1", SSE_SLOT, CLIF_COUNT, clif_stub)]
    {
        let id = declare(&mut m, name, Linkage::Export, &helpers_sig)?;
        define(&mut m, id, &helpers_sig, |m, b, p| {
            let (fns, n) = (p[0], p[1]);
            let gv = m.declare_data_in_func(table, b.func);
            let t = b.ins().symbol_value(q, gv);
            let sr = m.declare_func_in_func(stub, b.func);
            let stub = b.ins().func_addr(q, sr);
            let (head, body, done) = (b.create_block(), b.create_block(), b.create_block());
            b.append_block_param(head, i);
            b.append_block_param(body, i);
            let zero = b.ins().iconst(i, 0);
            b.ins().jump(head, &[BlockArg::Value(zero)]);

            // for (k = 0; k < count && k < n; k++)
            b.switch_to_block(head);
            let k = b.block_params(head)[0];
            let in_table = b.ins().icmp_imm_u(IntCC::UnsignedLessThan, k, count as i64);
            let given = b.ins().icmp(IntCC::UnsignedLessThan, k, n);
            let go = b.ins().band(in_table, given);
            b.ins().brif(go, body, &[BlockArg::Value(k)], done, &[]);

            //     table[first + k] = fns[k] ? fns[k] : stub;
            b.switch_to_block(body);
            let k = b.block_params(body)[0];
            let k64 = b.ins().uextend(q, k);
            let off = b.ins().ishl_imm_u(k64, 3i64);
            let at = b.ins().iadd(fns, off);
            let f = b.ins().load(q, MemFlagsData::new().with_notrap(), at, 0);
            let null = b.ins().icmp_imm_u(IntCC::Equal, f, 0i64);
            let f = b.ins().select(null, stub, f);
            let dst = b.ins().iadd(t, off);
            b.ins().store(MemFlagsData::new().with_notrap(), f, dst, (first * 8) as i32);
            let next = b.ins().iadd_imm_u(k, 1i64);
            b.ins().jump(head, &[BlockArg::Value(next)]);

            b.switch_to_block(done);
            b.ins().return_(&[]);
        })?;
    }
    emit(m)
}
