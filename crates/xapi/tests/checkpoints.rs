//! Retail checkpoint acceptance on synthetic state, through generated code.
//! Requires H2_XBE and clang; never copies game bytes into the repository.
mod checkpoint_support;

use ir::{Control, Target};
use runtime::{CpuAbi, Event, ExecCtx, Guest, H2GenModule, Runner, Runtime, guest::Permissions};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const TITLE: u32 = 0x4d53_0064;
const STATE: u32 = 0x8100_0000;
const SIZE: u32 = 0x3fe000;
const HEADER: u32 = 0x1288;
const SCRATCH: u32 = 0x2000_0000;
const TASK: u32 = SCRATCH + 0x1000;
const HEAP: u32 = SCRATCH + 0x10000;
const STAGING: u32 = 0x8200_0000;
const HOOK: u32 = 0x0012_4620;
const COMMIT: u32 = 0x0012_3e20;
const REVERT: u32 = 0x0012_3f60;
const RESTORE: u32 = 0x0012_4380;
const CREATE: u32 = 0x002d_2750;
const CLOSE: u32 = 0x002d_1f3a;
const READ: u32 = 0x002d_2241;
const WRITE: u32 = 0x002d_232e;
const HEADER_VALIDATE: u32 = 0x0012_4520;
const HEADER_COMPATIBLE: u32 = 0x0012_4590;
const READ_ORIGINAL: u32 = 0x6000_0000;

// Only host services and the synthetic scenario lifecycle are substituted.
// Snapshot policy, copies, validity/count checks, header identity checks,
// XAPI file wrappers, save metadata and signing stay generated.
const SEAMS: &[u32] = &[
    0x1208b0, 0x1209c0, 0x12d400, 0x12d520, 0x12280, 0x122c0, 0x593e0, 0x199520, 0x199540,
    0x18e8b0, 0x18f1c0, 0x11c1b0, 0x138180, 0x1384a0,
];

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = PathBuf::from(std::env::var_os("TMPDIR").expect("TMPDIR"))
            .join(format!("h2-checkpoints-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn functions(bytes: &[u8]) -> Vec<ir::IrFunction> {
    let image = xbe::Xbe::parse(bytes).unwrap();
    let analysis = analyze::analyze(&image).unwrap();
    let lifter = lift::Lifter::new(&analysis, &image).unwrap();
    let mut todo = vec![
        COMMIT, REVERT, RESTORE, HOOK, 0x215140, 0x215180, 0x1a0e70, 0x1a0fb0, CREATE, CLOSE, READ,
        WRITE, 0x2d2488, 0x2d2404, 0x2d15da, 0x2d156a, 0x330df2, 0x331125, 0x33122f, 0x331276,
        0x2d6765, 0x2d6779, 0x2d67c7,
    ];
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    while let Some(entry) = todo.pop() {
        if SEAMS.contains(&entry) || !seen.insert(entry) {
            continue;
        }
        let f = lifter
            .lift(entry)
            .unwrap_or_else(|| panic!("no owner {entry:#x}"));
        for i in f.insns() {
            match &i.control {
                Control::Call {
                    target: Target::Direct(pc),
                    ..
                }
                | Control::Jump {
                    target: Target::Direct(pc),
                    tail: true,
                    ..
                } => todo.push(*pc),
                _ => {}
            }
        }
        out.push(f);
        assert!(
            out.len() < 600,
            "fixture escaped storage boundary at {entry:#x}"
        );
    }
    eprintln!("checkpoint module: {} owners", out.len());
    out
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
struct Lifecycle {
    scenario: Vec<String>,
    bsp: Vec<u32>,
    before: usize,
    after: usize,
}

#[derive(Clone, Default)]
struct RestoreTrace {
    reads: Vec<ReadTrace>,
    identity: Vec<Vec<u8>>,
    compatibility: Vec<Vec<u8>>,
}

#[derive(Clone)]
struct ReadTrace {
    requested: u32,
    completed: u32,
    success: bool,
    prefix: Vec<u8>,
}

struct World {
    runtime: Arc<Runtime>,
    kernel: Arc<kernel::Kernel>,
    vfs: Arc<Mutex<vfs::Vfs>>,
    cpu: CpuAbi,
    ctx: ExecCtx,
    lifecycle: Arc<Mutex<Lifecycle>>,
    staging: Arc<AtomicBool>,
    trace: Arc<Mutex<RestoreTrace>>,
}

fn cstr(m: &Guest, addr: u32) -> String {
    let bytes: Vec<_> = (0..1024)
        .map(|i| m.u8(addr + i).unwrap())
        .take_while(|&b| b != 0)
        .collect();
    String::from_utf8(bytes).unwrap()
}

// Runs the real generated worker callback synchronously, including requeues.
// The game's worker-thread allocator/queue is the boundary; its I/O job is not.
fn work(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    task: u32,
) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    let rt = ctx.runtime().clone();
    let mut m = rt.memory();
    let callback = m.u32(task + 0x34)?;
    let completion = m.u32(task + 0x38)?;
    let arg = task + 8;
    m.set_u32(cpu.esp.wrapping_sub(4), arg)?;
    cpu.esp -= 4;
    ctx.call_guest(cpu, &mut m, callback, move |cpu, ctx| {
        if cpu.eax == 0 {
            return work(cpu, ctx, task);
        }
        let mut m = ctx.memory();
        m.set_u8(completion, 1)?;
        m.ret(cpu, 0)
    })
}

impl World {
    fn new(bytes: &[u8], lib: &libloading::Library, root: &Path) -> Self {
        let mut loaded = runtime::load_xbe(bytes).unwrap();
        let kernel = kernel::Kernel::for_image(&mut loaded).unwrap();
        let runtime = loaded.runtime;
        let config = vfs::Config::new(root, TITLE);
        let mut fs = vfs::Vfs::with_storage(config.clone()).unwrap();
        fs.create_standard_links(&config).unwrap();
        fs.namespace
            .create_symbolic_link("\\??\\N:", "\\Device\\Harddisk0\\Partition4")
            .unwrap();
        let vfs = Arc::new(Mutex::new(fs));
        kernel.set_shared_vfs(vfs.clone()).unwrap();
        // SAFETY: compiled for this runtime ABI; lib outlives every World.
        unsafe {
            let module: libloading::Symbol<*const H2GenModule> = lib.get(b"h2gen_module").unwrap();
            let version = lib
                .get::<unsafe extern "C" fn() -> u32>(b"h2gen_module_abi_version")
                .unwrap();
            runtime
                .edit_dispatch(|d| d.register_generated_module(&**module, *version))
                .unwrap();
        }
        {
            let mut m = runtime.memory();
            m.map_zeroed(STAGING, 0x40b000, Permissions::RW).unwrap();
            m.map_zeroed(SCRATCH, 0x100000, Permissions::RW).unwrap();
            m.map_zeroed(READ_ORIGINAL, 0x1000, Permissions::RX)
                .unwrap();
            m.set_u32(0x4e6080, STATE).unwrap();
            m.set_u32(0x4e6094, STATE).unwrap();
            m.set_u32(0x4e608c, 0x12345678).unwrap();
            m.set_u32(0x5020dc, STATE).unwrap();
            m.set_u32(0x5020e0, SIZE).unwrap();
            m.set_u32(0x46e48c, 0x10000).unwrap();
            m.set_u32(0x46e488, 0x10000).unwrap();
            m.set_u16(0x4e3b64, u16::MAX).unwrap();
            m.set_u32(0x510c54, SCRATCH + 0x2000).unwrap();
            for i in 0..5 {
                m.set_u32(0x46e320 + i * 4, HOOK).unwrap();
            }
            // XAPI's generated Get/SetLastError follows FS -> TLS -> slots.
            m.set_u32(0x480100, 0).unwrap();
            let fs = kernel.kpcr();
            m.set_u32(fs + 4, SCRATCH + 0x3000).unwrap();
            m.set_u32(SCRATCH + 0x3000, SCRATCH + 0x4000).unwrap();
            // Synthetic signing input, explicitly stable across the two runtimes.
            let key = m.u32(0x411660).unwrap();
            m.write_bytes(key, &[0x5a; 16], 1).unwrap();
        }
        let lifecycle = Arc::new(Mutex::new(Lifecycle::default()));
        let staging = Arc::new(AtomicBool::new(true));
        let trace = Arc::new(Mutex::new(RestoreTrace::default()));
        let read_original = runtime.resolve_original(READ).unwrap();
        let identity_original = runtime.resolve_original(HEADER_VALIDATE).unwrap();
        let compatible_original = runtime.resolve_original(HEADER_COMPATIBLE).unwrap();
        runtime.edit_dispatch(|d| {
            // Observe the generated reader's actual result without substituting
            // file I/O. The trampoline avoids re-entering this entry wrapper.
            d.register_override(
                READ_ORIGINAL,
                "fixture original ReadFile",
                move |cpu, ctx, _| Ok(read_original.invoke(cpu, ctx, READ)),
            );
            let reads = trace.clone();
            d.register_override(READ, "fixture observe ReadFile", move |cpu, ctx, _| {
                let rt = ctx.runtime().clone();
                let mut m = rt.memory();
                let mut args = [0; 5];
                for (i, arg) in args.iter_mut().enumerate() {
                    *arg = m.u32(cpu.esp + 4 + i as u32 * 4)?;
                }
                for arg in args.iter().rev() {
                    cpu.esp -= 4;
                    m.set_u32(cpu.esp, *arg)?;
                }
                let reads = reads.clone();
                ctx.call_guest(cpu, &mut m, READ_ORIGINAL, move |cpu, ctx| {
                    let m = ctx.memory();
                    let completed = m.u32(args[3])?;
                    assert!(completed <= args[2]);
                    let mut prefix = vec![0; completed.min(HEADER) as usize];
                    m.read_bytes(args[1], &mut prefix, 1)?;
                    reads.lock().unwrap().reads.push(ReadTrace {
                        requested: args[2],
                        completed,
                        success: cpu.eax != 0,
                        prefix,
                    });
                    m.ret(cpu, 20)
                })
            });
            let observed = trace.clone();
            d.register_override(
                HEADER_VALIDATE,
                "fixture observe header validation",
                move |cpu, ctx, pc| {
                    let mut header = vec![0; HEADER as usize];
                    ctx.memory().read_bytes(cpu.eax, &mut header, 1)?;
                    observed.lock().unwrap().identity.push(header);
                    Ok(identity_original.invoke(cpu, ctx, pc))
                },
            );
            let observed = trace.clone();
            d.register_override(
                HEADER_COMPATIBLE,
                "fixture observe header compatibility",
                move |cpu, ctx, pc| {
                    let mut header = vec![0; HEADER as usize];
                    ctx.memory().read_bytes(cpu.edi, &mut header, 1)?;
                    observed.lock().unwrap().compatibility.push(header);
                    Ok(compatible_original.invoke(cpu, ctx, pc))
                },
            );
            d.register_override(0x1208b0, "fixture task allocation", |cpu, ctx, _| {
                cpu.eax = TASK;
                ctx.memory().ret(cpu, 0)
            });
            d.register_override(0x1209c0, "fixture synchronous queue", |cpu, ctx, _| {
                work(cpu, ctx, cpu.eax)
            });
            let staging = staging.clone();
            d.register_override(
                0x12d400,
                "fixture staging allocation",
                move |cpu, ctx, _| {
                    cpu.eax = if staging.load(Ordering::Relaxed) {
                        STAGING
                    } else {
                        0
                    };
                    ctx.memory().ret(cpu, 16)
                },
            );
            d.register_override(0x12d520, "fixture staging release", |cpu, ctx, _| {
                ctx.memory().ret(cpu, 0)
            });
            let next = Mutex::new(HEAP);
            d.register_override(0x12280, "fixture tagged allocator", move |cpu, ctx, _| {
                let m = ctx.memory();
                let size = m.u32(cpu.esp + 4)?;
                let mut next = next.lock().unwrap();
                cpu.eax = *next;
                *next += (size + 15) & !15;
                assert!(*next < SCRATCH + 0x100000);
                m.ret(cpu, 8)
            });
            d.register_override(0x122c0, "fixture tagged free", |cpu, ctx, _| {
                ctx.memory().ret(cpu, 8)
            });
            for (pc, cleanup) in [(0x593e0, 0), (0x18f1c0, 4)] {
                d.register_override(pc, "fixture lifecycle", move |cpu, ctx, _| {
                    ctx.memory().ret(cpu, cleanup)
                });
            }
            for (pc, before) in [(0x199520, true), (0x199540, false)] {
                let life = lifecycle.clone();
                d.register_override(pc, "fixture state protection", move |cpu, ctx, _| {
                    let m = ctx.memory();
                    let mut life = life.lock().unwrap();
                    if before {
                        life.before += 1;
                    } else {
                        life.after += 1;
                    }
                    drop(life);
                    drop(m);
                    let rt = ctx.runtime().clone();
                    let mut m = rt.memory();
                    for arg in [if before { 4 } else { 2 }, SIZE, STATE] {
                        cpu.esp -= 4;
                        m.set_u32(cpu.esp, arg)?;
                    }
                    ctx.call_guest(cpu, &mut m, 0x2d15da, |cpu, ctx| ctx.memory().ret(cpu, 0))
                });
            }
            let life = lifecycle.clone();
            d.register_override(0x18e8b0, "fixture scenario load", move |cpu, ctx, _| {
                let mut m = ctx.memory();
                let options = m.u32(cpu.esp + 4)?;
                let scenario = cstr(&m, options + 0x1c);
                life.lock().unwrap().scenario.push(scenario.clone());
                m.write_bytes(STATE + 8, &[scenario.as_bytes(), &[0]].concat(), 1)?;
                let mut data = vec![0; 0x1118];
                m.read_bytes(options, &mut data, 1)?;
                m.write_bytes(STATE + 0x130, &data, 1)?;
                cpu.eax = 1;
                m.ret(cpu, 4)
            });
            let life = lifecycle.clone();
            d.register_override(0x11c1b0, "fixture BSP select", move |cpu, ctx, _| {
                let mut m = ctx.memory();
                let bsp = m.u32(cpu.esp + 4)?;
                assert_eq!(m.u32(cpu.esp + 8)?, 1);
                life.lock().unwrap().bsp.push(bsp);
                m.set_u16(STATE + 0x1248, bsp as u16)?;
                cpu.eax = 1;
                m.ret(cpu, 8)
            });
            d.register_override(
                0x138180,
                "fixture synthetic options validation",
                |cpu, ctx, _| {
                    let m = ctx.memory();
                    let options = m.u32(cpu.esp + 4)?;
                    cpu.eax = u32::from(
                        m.u16(options + 6)? == 30
                            && cstr(&m, options + 0x1c).starts_with("levels\\synthetic_"),
                    );
                    m.ret(cpu, 4)
                },
            );
            d.register_override(
                0x1384a0,
                "fixture synthetic options comparison",
                |cpu, ctx, _| {
                    let m = ctx.memory();
                    let a = m.u32(cpu.esp + 4)?;
                    let b = m.u32(cpu.esp + 8)?;
                    cpu.eax = u32::from(
                        m.u32(a)? == m.u32(b)?
                            && m.u16(a + 6)? == m.u16(b + 6)?
                            && cstr(&m, a + 0x1c) == cstr(&m, b + 0x1c),
                    );
                    m.ret(cpu, 8)
                },
            );
        });
        let ctx = ExecCtx::new(runtime.clone(), 1);
        let cpu = CpuAbi {
            fs_base: kernel.kpcr(),
            ..loaded.cpu
        };
        let mut world = Self {
            runtime,
            kernel,
            vfs,
            cpu,
            ctx,
            lifecycle,
            staging,
            trace,
        };
        assert_eq!(world.call(0x2d156a, &[SIZE, 0x1000000, 0, 4]), STATE);
        world
    }

    fn call(&mut self, pc: u32, args: &[u32]) -> u32 {
        let esp = runtime::loader::STACK_TOP - 0x100;
        {
            let mut m = self.runtime.memory();
            m.set_u32(esp, runtime::exec::HARNESS_EXIT).unwrap();
            for (i, &arg) in args.iter().enumerate() {
                m.set_u32(esp + 4 + i as u32 * 4, arg).unwrap();
            }
        }
        self.cpu.esp = esp;
        let t = Runner {
            max_transfers: 200_000,
        }
        .run(&mut self.cpu, &mut self.ctx, pc);
        assert!(
            matches!(self.ctx.event(t.event_id), Some(Event::HarnessExit)),
            "{pc:#x}: {:?}; kernel {:?}",
            self.ctx.report(t.event_id),
            self.kernel.stop_reasons()
        );
        self.cpu.eax
    }

    fn text(&self, addr: u32, text: &str) {
        self.runtime
            .memory()
            .write_bytes(addr, &[text.as_bytes(), &[0]].concat(), 1)
            .unwrap();
    }

    fn open(&mut self, path: &str, disposition: u32) -> u32 {
        self.text(SCRATCH, path);
        let h = self.call(CREATE, &[SCRATCH, 0xc0000000, 3, 0, disposition, 0x80, 0]);
        assert_ne!(h, u32::MAX, "open {path}");
        h
    }

    fn snapshots(&mut self) {
        for slot in 0..2 {
            let h = self.open(&format!("N:\\savegame{slot}.bin"), 4);
            self.runtime
                .memory()
                .set_u32(0x5020f8 + slot * 4, h)
                .unwrap();
            assert_eq!(self.call(0x2d2488, &[h, 0x40b000, 0, 0]), 0x40b000);
            // SetEndOfFile, discovered separately from SetFilePointer.
            assert_eq!(self.call(0x2d2404, &[h]), 1);
            assert_eq!(
                self.vfs
                    .lock()
                    .unwrap()
                    .query_path(&format!("N:\\savegame{slot}.bin"))
                    .unwrap()
                    .end_of_file,
                0x40b000
            );
        }
    }

    fn bytes(&self, addr: u32, len: u32) -> Vec<u8> {
        let mut out = vec![0; len as usize];
        self.runtime.memory().read_bytes(addr, &mut out, 1).unwrap();
        out
    }

    fn signature(&mut self, split: bool) -> Vec<u8> {
        let handle = self.call(0x2d6765, &[0]);
        assert_ne!(handle, u32::MAX);
        if split {
            assert_eq!(self.call(0x2d6779, &[handle, STATE, HEADER]), 0);
            assert_eq!(
                self.call(0x2d6779, &[handle, STATE + HEADER, SIZE - HEADER]),
                0
            );
        } else {
            assert_eq!(self.call(0x2d6779, &[handle, STATE, SIZE]), 0);
        }
        assert_eq!(self.call(0x2d67c7, &[handle, SCRATCH + 0x9000]), 0);
        self.bytes(SCRATCH + 0x9000, 20)
    }

    fn file(&mut self, path: &str) -> Vec<u8> {
        let len = self
            .vfs
            .lock()
            .unwrap()
            .query_path(path)
            .unwrap()
            .end_of_file as u32;
        let h = self.open(path, 3);
        assert_eq!(self.call(READ, &[h, STAGING, len, SCRATCH + 0x9200, 0]), 1);
        assert_eq!(self.runtime.memory().u32(SCRATCH + 0x9200).unwrap(), len);
        assert_eq!(self.call(CLOSE, &[h]), 1);
        self.bytes(STAGING, len)
    }

    fn put(&mut self, path: &str, buffer: u32, len: u32) {
        let h = self.open(path, 2);
        assert_eq!(self.call(WRITE, &[h, buffer, len, SCRATCH + 0x9200, 0]), 1);
        assert_eq!(self.runtime.memory().u32(SCRATCH + 0x9200).unwrap(), len);
        assert_eq!(self.call(CLOSE, &[h]), 1);
    }

    fn create_save(&mut self) -> String {
        self.text(SCRATCH + 0x8000, "U:\\");
        let name = "Synthetic checkpoint \u{03a9}";
        let wide: Vec<u8> = name
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        self.runtime
            .memory()
            .write_bytes(SCRATCH + 0x8100, &wide, 1)
            .unwrap();
        assert_eq!(
            self.call(
                0x330df2,
                &[
                    SCRATCH + 0x8000,
                    SCRATCH + 0x8100,
                    4,
                    1,
                    SCRATCH + 0x8300,
                    260
                ]
            ),
            0
        );
        let path = cstr(&self.runtime.memory(), SCRATCH + 0x8300);
        let hash = name.encode_utf16().fold(0u128, |h, u| {
            (h * 65536 + u128::from(u)) % ((1u128 << 48) - 59)
        });
        assert_eq!(path, format!("U:\\{hash:012X}\\"));
        let meta: Vec<u8> = format!("\u{feff}Name={name}\r\nNoCopy=1\r\n")
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(self.file(&format!("{path}SaveMeta.xbx")), meta);
        path
    }

    fn enumerate_save(&mut self, expected_path: &str) {
        self.text(SCRATCH + 0x8000, "U:\\");
        let data = SCRATCH + 0xa000;
        self.runtime
            .memory()
            .write_bytes(data, &[0xcc; 836], 1)
            .unwrap();
        let search = self.call(0x331125, &[SCRATCH + 0x8000, data]);
        assert_ne!(search, u32::MAX);
        assert_eq!(cstr(&self.runtime.memory(), data + 320), expected_path);
        let wide = self.bytes(data + 580, 256);
        let name: Vec<_> = wide
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
            .take_while(|&u| u != 0)
            .collect();
        assert_eq!(
            String::from_utf16(&name).unwrap(),
            "Synthetic checkpoint \u{03a9}"
        );
        assert!(name.len() < 128);
        assert_eq!(self.call(0x33122f, &[search, data]), 0);
        assert_eq!(self.call(0x2d1d3e, &[]), 18);
        assert_eq!(self.call(0x331276, &[search]), 1);
    }

    fn prepare_restore(&mut self) {
        self.call(0x2d15da, &[STATE, SIZE, 4]);
        let initial = state(self, 0xee, 0);
        self.runtime
            .memory()
            .write_bytes(STATE, &initial, 1)
            .unwrap();
        self.runtime.memory().set_u32(0x4e6948, 0).unwrap();
    }

    fn probe_restore(&mut self) -> (u32, RestoreTrace) {
        *self.trace.lock().unwrap() = RestoreTrace::default();
        self.text(SCRATCH, "N:\\header-case");
        let result = self.call(RESTORE, &[SCRATCH]) & 0xff;
        let trace = self.trace.lock().unwrap().clone();
        (result, trace)
    }

    fn header_rejections(&mut self, older: &[u8]) {
        let path = "N:\\header-case.bin";
        // Every candidate is closed before restore. A valid control uses the
        // identical file, open mode, writer cleanup and restore setup.
        self.runtime
            .memory()
            .write_bytes(STAGING, older, 1)
            .unwrap();
        self.put(path, STAGING, SIZE);
        self.prepare_restore();
        let (result, trace) = self.probe_restore();
        assert_eq!(result, 1, "valid header control");
        assert_header_read(&trace, &older[..HEADER as usize]);
        assert_eq!(trace.reads.len(), 2);
        assert_eq!(trace.reads[1].completed, SIZE);
        assert_eq!((trace.identity.len(), trace.compatibility.len()), (2, 1));
        assert_bytes(&self.bytes(STATE, SIZE), older);

        for (name, offset, validator, cleanup) in [
            ("allocation identity", 0, HEADER_VALIDATE, 0),
            ("backing address", 4, HEADER_VALIDATE, 0),
            ("build", 0x108, HEADER_VALIDATE, 0),
            ("compatibility", 0x128, HEADER_COMPATIBLE, 4),
        ] {
            let mut bad = older.to_vec();
            bad[offset] ^= 0xff;
            self.runtime.memory().write_bytes(STAGING, &bad, 1).unwrap();
            self.put(path, STAGING, SIZE);
            self.prepare_restore();
            let baseline = self.bytes(STATE, SIZE);
            let life = self.lifecycle.lock().unwrap().clone();
            let (result, trace) = self.probe_restore();
            assert_eq!(result, 0, "{name} must be rejected");
            assert_header_read(&trace, &bad[..HEADER as usize]);
            assert_eq!(trace.reads.len(), 1, "{name}: no bulk state read");
            assert_bytes(&trace.identity[0], &bad[..HEADER as usize]);
            if validator == HEADER_VALIDATE {
                assert_eq!((trace.identity.len(), trace.compatibility.len()), (1, 0));
                assert_eq!(
                    *self.lifecycle.lock().unwrap(),
                    life,
                    "{name}: lifecycle unchanged"
                );
                assert_bytes(&self.bytes(STATE, SIZE), &baseline);
            } else {
                assert_eq!((trace.identity.len(), trace.compatibility.len()), (2, 1));
                // Restore marks the temporary options initialized before the
                // compatibility call; the original disk read remains exact.
                let mut compared = bad[..HEADER as usize].to_vec();
                compared[0x134] = 1;
                assert_bytes(&trace.compatibility[0], &compared);
                assert_bytes(&trace.identity[1], &compared);
                let current = self.lifecycle.lock().unwrap();
                assert_eq!(current.scenario.len(), life.scenario.len() + 1);
                assert_eq!(current.bsp.len(), life.bsp.len() + 1);
                assert_eq!((current.before, current.after), (life.before, life.after));
                assert_bytes(
                    &self.bytes(STATE + HEADER, SIZE - HEADER),
                    &baseline[HEADER as usize..],
                );
            }

            // Counterfactual on the same closed candidate: disabling exactly
            // its validator must make this rejection test fail. Preserve and
            // restore the observer around the real generated validator.
            let previous = self.runtime.edit_dispatch(|d| {
                d.register_override(
                    validator,
                    "fixture mutation: bypass validator",
                    move |cpu, ctx, _| {
                        cpu.eax = 1;
                        ctx.memory().ret(cpu, cleanup)
                    },
                )
                .unwrap()
            });
            self.prepare_restore();
            let (accepted, trace) = self.probe_restore();
            self.runtime
                .edit_dispatch(|d| d.restore_override(validator, previous));
            assert_eq!(accepted, 1, "{name}: bypass must break rejection");
            assert_eq!(trace.reads.len(), 2, "{name}: bypass reaches bulk read");
            assert_eq!(trace.reads[1].completed, SIZE);
            assert_bytes(&self.bytes(STATE, SIZE), &bad);
            eprintln!("header {name}: rejected normally; accepted with {validator:#x} disabled");
        }

        self.runtime
            .memory()
            .write_bytes(STAGING, older, 1)
            .unwrap();
        self.put(path, STAGING, 8);
        self.prepare_restore();
        let baseline = self.bytes(STATE, SIZE);
        let life = self.lifecycle.lock().unwrap().clone();
        let (result, trace) = self.probe_restore();
        assert_eq!(result, 0, "short header");
        assert_eq!(trace.reads.len(), 1);
        let read = &trace.reads[0];
        assert!(read.success, "short header: successful open and ReadFile");
        assert_eq!((read.requested, read.completed), (HEADER, 8));
        assert_bytes(&read.prefix, &older[..8]);
        assert!(trace.identity.is_empty() && trace.compatibility.is_empty());
        assert_eq!(*self.lifecycle.lock().unwrap(), life);
        assert_bytes(&self.bytes(STATE, SIZE), &baseline);
        eprintln!("short header: successful ReadFile, 8 bytes, no validators or lifecycle work");
    }

    fn failure_cases(&mut self, older: &[u8], root: &Path) {
        {
            let mut fs = self.vfs.lock().unwrap();
            fs.mount(
                "\\Device\\Full",
                vfs::HostBackend::new(root.join("full-data"), root.join("full-meta"))
                    .unwrap()
                    .with_volume("small", 99, 0x10000),
            )
            .unwrap();
            fs.namespace
                .create_symbolic_link("\\??\\F:", "\\Device\\Full")
                .unwrap();
        }
        let failed = self.open("F:\\failed.bin", 4);
        let original = self.runtime.memory().u32(0x5020f8).unwrap();
        self.runtime.memory().set_u32(0x5020f8, failed).unwrap();
        self.runtime.memory().set_u16(0x4e3b64, 1).unwrap();
        self.staging.store(true, Ordering::Relaxed);
        self.call(COMMIT, &[]); // wraps back to slot zero
        self.call(0x215180, &[0, 0]);
        assert_eq!(self.runtime.memory().u16(0x4e3b64).unwrap(), 0);
        assert_eq!(self.runtime.memory().u32(0x5020ec).unwrap(), 0x10000);
        assert_eq!(self.runtime.memory().u8(0x5020f2).unwrap(), 0);
        assert_eq!(self.runtime.memory().u8(0x5020f3).unwrap(), 1);
        assert_bytes(&self.file("N:\\savegame1.bin")[..SIZE as usize], older);
        // A valid slot marker does not turn a short read into a successful revert.
        self.runtime.memory().set_u8(0x5020f2, 1).unwrap();
        assert_eq!(self.call(0x2152e0, &[0]) & 0xff, 0);
        assert_eq!(self.call(0x2152e0, &[1]) & 0xff, 1);
        assert_bytes(&self.bytes(STATE, SIZE), older);
        self.runtime.memory().set_u32(0x5020f8, original).unwrap();
        assert_eq!(self.call(CLOSE, &[failed]), 1);
        self.header_rejections(older);
    }
}

fn state(w: &World, seed: u8, bsp: u16) -> Vec<u8> {
    let mut bytes: Vec<_> = (0..SIZE)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect();
    bytes[..HEADER as usize].fill(0);
    bytes[..4].copy_from_slice(&0x12345678u32.to_le_bytes());
    bytes[4..8].copy_from_slice(&STATE.to_le_bytes());
    let scenario = format!("levels\\synthetic_{bsp}\0");
    bytes[8..8 + scenario.len()].copy_from_slice(scenario.as_bytes());
    // Accepted build name comes from the image at runtime, never committed.
    let m = w.runtime.memory();
    let build = cstr(&m, m.u32(0x43e86c).unwrap());
    bytes[0x108..0x108 + build.len()].copy_from_slice(build.as_bytes());
    bytes[0x14c..0x14c + scenario.len()].copy_from_slice(scenario.as_bytes());
    bytes[0x136..0x138].copy_from_slice(&30u16.to_le_bytes());
    bytes[0x1248..0x124a].copy_from_slice(&bsp.to_le_bytes());
    bytes
}

fn assert_bytes(actual: &[u8], expected: &[u8]) {
    assert_eq!(actual.len(), expected.len());
    let diff = actual.iter().zip(expected).position(|(a, b)| a != b);
    assert_eq!(
        diff,
        None,
        "first different state byte; values {:?}",
        diff.map(|i| (actual[i], expected[i]))
    );
}

fn assert_header_read(trace: &RestoreTrace, expected: &[u8]) {
    let read = &trace.reads[0];
    assert!(read.success, "header: successful open and ReadFile");
    assert_eq!((read.requested, read.completed), (HEADER, HEADER));
    assert_bytes(&read.prefix, expected);
}

#[test]
#[ignore = "requires H2_XBE and clang; generated checkpoint acceptance"]
fn checkpoint_commit_revert_and_restart() {
    let scratch = Scratch::new();
    let bytes = std::fs::read(std::env::var_os("H2_XBE").expect("set H2_XBE")).unwrap();
    let lib = checkpoint_support::build(
        functions(&bytes),
        &Path::new(env!("CARGO_TARGET_TMPDIR")).join("checkpoints-module"),
    );
    let root = scratch.0.join("storage");
    let mut w = World::new(&bytes, &lib, &root);
    w.snapshots();
    let mut expected = Vec::new();
    for (seed, bsp) in [(0x31, 1), (0x72, 2)] {
        let state = state(&w, seed, bsp);
        w.runtime.memory().write_bytes(STATE, &state, 1).unwrap();
        w.call(0x2d15da, &[STATE, SIZE, 2]);
        w.staging.store(expected.is_empty(), Ordering::Relaxed);
        w.call(COMMIT, &[]);
        if expected.is_empty() {
            assert_eq!(w.runtime.memory().u32(0x5020e4).unwrap(), STAGING);
            assert_eq!(w.runtime.memory().u8(0x5020f2).unwrap(), 0);
        }
        w.call(0x215180, &[0, 0]);
        let slot = expected.len() as u32;
        let m = w.runtime.memory();
        assert_eq!(m.u16(0x4e3b64).unwrap(), slot as u16);
        assert_eq!(m.u32(0x5020ec).unwrap(), SIZE);
        assert_eq!(m.u8(0x5020f2 + slot).unwrap(), 1);
        drop(m);
        assert_bytes(&w.bytes(STATE, SIZE), &state);
        assert_bytes(
            &w.bytes(0x4e3b68 + slot * HEADER, HEADER),
            &state[..HEADER as usize],
        );
        expected.push(state);
        assert!(w.runtime.memory().set_u8(STATE + SIZE - 1, 0).is_err());
        w.call(0x2d15da, &[STATE, SIZE, 4]);
    }
    for (slot, saved) in expected.iter().enumerate() {
        w.runtime
            .memory()
            .write_bytes(STATE, &expected[1 - slot], 1)
            .unwrap();
        w.runtime.memory().set_u16(0x4e3b64, slot as u16).unwrap();
        // In-session revert requires a live matching scenario/options.
        w.runtime
            .memory()
            .set_u32(0x4e6948, SCRATCH + 0x5000)
            .unwrap();
        w.runtime.memory().set_u8(SCRATCH + 0x6120, 1).unwrap();
        w.runtime
            .memory()
            .write_bytes(SCRATCH + 0x5008, &saved[0x130..0x1248], 1)
            .unwrap();
        // Restore compatibility compares current metadata with the saved header.
        w.runtime
            .memory()
            .write_bytes(STATE, &saved[..HEADER as usize], 1)
            .unwrap();
        w.cpu.ecx = 0;
        w.call(REVERT, &[]);
        assert_bytes(&w.bytes(STATE, SIZE), saved);
        assert!(w.runtime.memory().set_u8(STATE, 0).is_err());
        w.call(0x2d15da, &[STATE, SIZE, 4]);
    }
    let signature = w.signature(false);
    assert_eq!(w.signature(true), signature);
    w.runtime
        .memory()
        .set_u8(STATE + SIZE - 1, expected[1][SIZE as usize - 1] ^ 1)
        .unwrap();
    assert_ne!(w.signature(true), signature);
    w.runtime
        .memory()
        .write_bytes(STATE, &expected[1], 1)
        .unwrap();
    let save_path = w.create_save();
    w.enumerate_save(&save_path);
    let meta_path = format!("{save_path}SaveMeta.xbx");
    let meta = w.file(&meta_path);
    let creation = w
        .vfs
        .lock()
        .unwrap()
        .query_path(&meta_path)
        .unwrap()
        .basic
        .creation_time;
    // This persistent fixture uses U: independently of the disposable N: slots.
    w.put(&format!("{save_path}checkpoint.bin"), STATE, SIZE);
    w.runtime
        .memory()
        .write_bytes(SCRATCH + 0x9000, &signature, 1)
        .unwrap();
    w.put(&format!("{save_path}checkpoint.sig"), SCRATCH + 0x9000, 20);
    w.failure_cases(&expected[1], &scratch.0);
    drop(w);
    // A new Runtime, kernel, handles and host backend; only disk survives.
    let mut w = World::new(&bytes, &lib, &root);
    for (slot, saved) in expected.iter().enumerate() {
        assert_bytes(
            &w.file(&format!("N:\\savegame{slot}.bin"))[..SIZE as usize],
            saved,
        );
        let initial = state(&w, 0xee, 0);
        w.runtime.memory().write_bytes(STATE, &initial, 1).unwrap();
        // N: validity/header caches are process-local; full restore takes a path.
        w.text(SCRATCH, &format!("N:\\savegame{slot}"));
        assert_eq!(w.call(RESTORE, &[SCRATCH]) & 0xff, 1);
        assert_bytes(&w.bytes(STATE, SIZE), saved);
        assert!(w.runtime.memory().set_u8(STATE, 0).is_err());
        w.call(0x2d15da, &[STATE, SIZE, 4]);
    }
    w.enumerate_save(&save_path);
    assert_eq!(w.file(&meta_path), meta);
    assert_eq!(
        w.vfs
            .lock()
            .unwrap()
            .query_path(&meta_path)
            .unwrap()
            .basic
            .creation_time,
        creation
    );
    let initial = state(&w, 0xee, 0);
    w.runtime.memory().write_bytes(STATE, &initial, 1).unwrap();
    w.text(SCRATCH, &format!("{save_path}checkpoint"));
    assert_eq!(w.call(RESTORE, &[SCRATCH]) & 0xff, 1);
    assert_bytes(&w.bytes(STATE, SIZE), &expected[1]);
    assert_eq!(w.file(&format!("{save_path}checkpoint.sig")), signature);
    assert_eq!(w.signature(true), signature);
    let life = w.lifecycle.lock().unwrap();
    assert_eq!(
        life.scenario,
        [
            "levels\\synthetic_1",
            "levels\\synthetic_2",
            "levels\\synthetic_2"
        ]
    );
    assert_eq!(life.bsp, [1, 2, 2]);
    assert_eq!((life.before, life.after), (3, 3));
}
