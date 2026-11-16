//! Campaign cache integration, retail code supplied only at test time.
//! See docs/runtime/cache-slots.md for the boundary and deliberately unproved activation work.
mod common;
#[path = "common/generated.rs"]
mod generated;
use common::*;
use ir::{Control, Target};
use runtime::{CpuAbi, Event, ExecCtx, GuestFault, H2GenModule, Runner, TransferAbi};
use std::{
    collections::BTreeSet,
    fs::File,
    io,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use vfs::{Backend, BasicInfo, DirectoryEntry, Metadata, VolumeInfo};

const SCRATCH: u32 = 0x6000_0000;
const JOB: u32 = SCRATCH + 0x1000;
const RECORDS: u32 = 0x557c90;
const STRIDE: u32 = 0x804;
const STATE: u32 = 0x55bd04;
const HEADER: u32 = 0x55b4f0;
const CURRENT: u32 = 0x55aca8;
const COUNT: u32 = 0x55bcf0;
const COPIED: u32 = 0x55bcf8;
const CHECKSUM: u32 = 0x55c02c;
const BUFFER: u32 = 0x55bd18;
const COPY: u32 = 0x2141f0;
const SELECT: u32 = 0x214b80;
const ROUTE: u32 = 0x213760;
// name, physical file bytes (menu discovered at runtime), map type, expected slot.
const MAPS: [(&str, u32, u32, u32); 6] = [
    ("mainmenu", 0, 2, 2),
    ("shared", 183_370_752, 3, 0),
    ("single_player_shared", 429_017_088, 4, 1),
    ("00a_introduction", 113_031_168, 0, 4),
    ("01a_tutorial", 69_169_664, 0, 5),
    ("01b_spacestation", 248_586_240, 0, 4),
];
// Engine scheduling/arena and a clock boundary, not replacements for the loader.
const ADAPTERS: &[u32] = &[0x1208b0, 0x1209c0, 0x12d400, 0x12d520, 0x3303f3];

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = std::env::var_os("TMPDIR").expect("TMPDIR must name session scratch");
        let path = PathBuf::from(root).join(format!("cache-slots-{}", std::process::id()));
        std::fs::create_dir(&path).expect("unique cache-slots scratch directory");
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Debug)]
enum Io {
    Read(String, u64, usize),
    Write(String, u64, usize, bool),
    Flush(String),
}
#[derive(Default)]
struct Audit {
    io: Vec<Io>,
    // One-shot faults at the backend seam: exercise real NT/XAPI byte counts.
    short_read: bool,
    short_write: bool,
    full: bool,
    corrupt: bool,
}
struct Audited<B> {
    inner: B,
    audit: Arc<Mutex<Audit>>,
}
impl<B: Backend> Backend for Audited<B> {
    fn read_only(&self) -> bool {
        self.inner.read_only()
    }
    fn stat(&mut self, path: &str) -> vfs::Result<Metadata> {
        self.inner.stat(path)
    }
    fn read_directory(&mut self, path: &str) -> vfs::Result<Vec<DirectoryEntry>> {
        self.inner.read_directory(path)
    }
    fn read_at(&mut self, path: &str, offset: u64, out: &mut [u8]) -> vfs::Result<usize> {
        let mut audit = self.audit.lock().unwrap();
        let len = if offset >= 0x800 && std::mem::take(&mut audit.short_read) {
            out.len() / 2
        } else {
            out.len()
        };
        let n = self.inner.read_at(path, offset, &mut out[..len])?;
        if offset >= 0x800 && n > 0 && std::mem::take(&mut audit.corrupt) {
            out[0] ^= 1;
        }
        audit.io.push(Io::Read(path.into(), offset, n));
        Ok(n)
    }
    fn volume_info(&mut self) -> vfs::Result<VolumeInfo> {
        self.inner.volume_info()
    }
    fn create(&mut self, p: &str, d: bool, a: u32) -> vfs::Result<()> {
        self.inner.create(p, d, a)
    }
    fn create_allocated(
        &mut self,
        p: &str,
        d: bool,
        a: u32,
        allocation: u64,
        owner: u64,
    ) -> vfs::Result<()> {
        self.inner.create_allocated(p, d, a, allocation, owner)
    }
    fn overwrite_allocated(&mut self, p: &str, allocation: u64, owner: u64) -> vfs::Result<()> {
        self.inner.overwrite_allocated(p, allocation, owner)
    }
    fn release_allocation(&mut self, p: &str, owner: u64) -> vfs::Result<()> {
        self.inner.release_allocation(p, owner)
    }
    fn write_at(&mut self, path: &str, offset: u64, bytes: &[u8]) -> vfs::Result<usize> {
        let mut audit = self.audit.lock().unwrap();
        if offset >= 0x800 && std::mem::take(&mut audit.full) {
            return Err(vfs::Error::DiskFull);
        }
        let len = if offset >= 0x800 && std::mem::take(&mut audit.short_write) {
            bytes.len() / 2
        } else {
            bytes.len()
        };
        let n = self.inner.write_at(path, offset, &bytes[..len])?;
        audit.io.push(Io::Write(
            path.into(),
            offset,
            n,
            bytes.starts_with(b"daeh"),
        ));
        Ok(n)
    }
    fn set_len(&mut self, p: &str, n: u64) -> vfs::Result<()> {
        self.inner.set_len(p, n)
    }
    fn set_basic_info(&mut self, p: &str, b: BasicInfo) -> vfs::Result<()> {
        self.inner.set_basic_info(p, b)
    }
    fn remove(&mut self, p: &str) -> vfs::Result<()> {
        self.inner.remove(p)
    }
    fn format(&mut self) -> vfs::Result<()> {
        self.inner.format()
    }
    fn flush(&mut self, p: &str) -> vfs::Result<()> {
        self.inner.flush(p)?;
        self.audit.lock().unwrap().io.push(Io::Flush(p.into()));
        Ok(())
    }
    fn cleanup(&mut self, p: &str, w: bool) -> vfs::Result<()> {
        self.inner.cleanup(p, w)
    }
}

fn functions(xbe: &xbe::Xbe<'_>) -> Vec<ir::IrFunction> {
    let analysis = analyze::analyze(xbe).unwrap();
    let lifter = lift::Lifter::new(&analysis, xbe).unwrap();
    // Worker callbacks are address-taken, so seed them explicitly too.
    let mut todo = vec![
        COPY, SELECT, ROUTE, 0x214a60, 0x214030, 0x1a0e70, 0x1a0fb0, 0x1a1100, 0x1a1530, 0x1a15d0,
        0x1a1680,
    ];
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    while let Some(pc) = todo.pop() {
        if ADAPTERS.contains(&pc) || !seen.insert(pc) {
            continue;
        }
        let f = lifter
            .lift(pc)
            .unwrap_or_else(|| panic!("not discovered: {pc:#x}"));
        f.validate()
            .unwrap_or_else(|e| panic!("invalid {pc:#x}: {e}"));
        for i in f.insns() {
            match &i.control {
                Control::Call {
                    target: Target::Direct(t),
                    ..
                }
                | Control::Jump {
                    target: Target::Direct(t),
                    tail: true,
                    ..
                } => todo.push(*t),
                Control::Branch {
                    taken, fallthrough, ..
                } => {
                    for d in [taken, fallthrough] {
                        if d.tail {
                            todo.push(d.pc);
                        }
                    }
                }
                Control::Switch { cases, .. } => {
                    for d in cases {
                        if d.tail {
                            todo.push(d.pc);
                        }
                    }
                }
                _ => {}
            }
        }
        result.push(f);
        assert!(result.len() < 250, "unexpected loader dependency expansion");
    }
    eprintln!("cache-slots: {} retail functions", result.len());
    result
}

type Result = std::result::Result<TransferAbi, GuestFault>;
fn call<F>(cpu: &mut CpuAbi, ctx: &mut ExecCtx, target: u32, args: &[u32], then: F) -> Result
where
    F: FnOnce(&mut CpuAbi, &mut ExecCtx) -> Result + Send + 'static,
{
    let rt = ctx.runtime().clone();
    let mut m = rt.memory();
    cpu.esp -= args.len() as u32 * 4;
    for (i, word) in args.iter().enumerate() {
        m.set_u32(cpu.esp + i as u32 * 4, *word)?;
    }
    ctx.call_guest(cpu, &mut m, target, then)
}
fn pump(cpu: &mut CpuAbi, ctx: &mut ExecCtx, job: u32, remaining: usize) -> Result {
    assert!(remaining > 0, "worker failed to complete");
    let target = ctx.memory().u32(job + 0x34)?;
    call(cpu, ctx, target, &[job + 8], move |cpu, ctx| {
        if cpu.eax == 0 {
            return pump(cpu, ctx, job, remaining - 1);
        }
        let done = ctx.memory().u32(job + 0x38)?;
        ctx.memory().set_u8(done, 1)?;
        cpu.eax = 1;
        ctx.memory().ret(cpu, 0)
    })
}
fn install_tls(h: &mut Harness, tls: xbe::TlsDirectory) {
    // The fixture enters after XAPI startup. Give generated SetLastError its
    // real TLS template and signed stack-base-relative index, without running
    // the unrelated bootstrap. No Machine FX save area is used by this Runner.
    let raw = tls.raw_end - tls.raw_start;
    let payload = raw + tls.zero_fill;
    let reservation = (payload + 15) & !15;
    let reservation = reservation + 4;
    assert!(reservation + 48 < 0x100, "TLS overlaps fixture call frames");
    let slot = STACK_TOP - reservation;
    let mut bytes = vec![0; payload as usize];
    h.runtime
        .memory()
        .read_bytes(tls.raw_start, &mut bytes[..raw as usize], 1)
        .unwrap();
    h.bytes(slot + 4, &bytes);
    h.set(slot, slot + 4);
    h.set(h.kernel.kpcr() + 4, STACK_TOP);
    let index = (-(reservation as i32) / 4) as u32;
    h.set(tls.index_addr, index);
    h.set(0x480100, index); // XAPI's published TLS index, normally set at startup.
    run(h, 0x2d1d66, &[42], |_| {});
    assert_eq!(h.get(slot + 12), 42, "generated XAPI last-error cell");
}

fn install_adapters(h: &Harness) {
    h.runtime.edit_dispatch(|d| {
        d.register_override(0x1208b0, "fixture job allocation", |cpu, ctx, _| {
            ctx.memory().write_bytes(JOB, &[0; 0x40], 1)?;
            cpu.eax = JOB;
            ctx.memory().ret(cpu, 0)
        });
        d.register_override(0x1209c0, "fixture FIFO worker", |cpu, ctx, _| {
            pump(cpu, ctx, cpu.eax, 4096)
        });
        d.register_override(0x3303f3, "fixture FILETIME", |cpu, ctx, _| {
            let out = ctx.memory().u32(cpu.esp + 4)?;
            ctx.memory().set_u64(out, 1_000_000)?;
            ctx.memory().ret(cpu, 4)
        });
        d.register_override(
            0x12d400,
            "fixture title allocator to kernel",
            |cpu, ctx, _| {
                let size = ctx.memory().u32(cpu.esp + 4)?;
                call(
                    cpu,
                    ctx,
                    runtime::kernel::function_token(166),
                    &[size, 0, u32::MAX, 0, 4],
                    |cpu, ctx| {
                        assert_ne!(cpu.eax, 0, "copy staging allocation");
                        ctx.memory().ret(cpu, 16)
                    },
                )
            },
        );
        d.register_override(0x12d520, "fixture title free to kernel", |cpu, ctx, _| {
            call(
                cpu,
                ctx,
                runtime::kernel::function_token(171),
                &[cpu.esi],
                |cpu, ctx| ctx.memory().ret(cpu, 0),
            )
        });
    });
}

fn run(h: &mut Harness, entry: u32, args: &[u32], regs: impl FnOnce(&mut CpuAbi)) -> u32 {
    h.ctx = ExecCtx::new(h.runtime.clone(), 1);
    h.cpu = CpuAbi {
        esp: STACK_TOP - 0x100,
        fs_base: h.kernel.kpcr(),
        ..CpuAbi::xbox_thread()
    };
    h.set(h.cpu.esp, runtime::exec::HARNESS_EXIT);
    for (i, arg) in args.iter().enumerate() {
        h.set(h.cpu.esp + 4 + i as u32 * 4, *arg);
    }
    regs(&mut h.cpu);
    let t = Runner {
        max_transfers: 200_000,
    }
    .run(&mut h.cpu, &mut h.ctx, entry);
    assert!(
        matches!(h.ctx.event(t.event_id), Some(Event::HarnessExit)),
        "{entry:#x}: {t:?}\n{:?}\n{:?}",
        h.ctx.report(t.event_id),
        h.kernel.stop_reasons()
    );
    assert_eq!(h.cpu.esp, STACK_TOP - 0x100 + 4 + args.len() as u32 * 4);
    assert_eq!(h.ctx.clear_continuations(), 0, "abandoned callback");
    h.cpu.eax
}
fn open(h: &mut Harness, name: &str, create: bool, writable: bool) -> u32 {
    h.bytes(SCRATCH + 0x200, name.as_bytes());
    h.set(
        SCRATCH + 0x80,
        name.len() as u32 | ((name.len() as u32) << 16),
    );
    h.set(SCRATCH + 0x84, SCRATCH + 0x200);
    h.set(SCRATCH + 0x100, 0);
    h.set(SCRATCH + 0x104, SCRATCH + 0x80);
    h.set(SCRATCH + 0x108, 0x40);
    let access = if writable {
        vfs::Access::ALL.0
    } else {
        0x8010_0080
    };
    let result = if create {
        h.success(
            190,
            &[
                SCRATCH,
                access,
                SCRATCH + 0x100,
                SCRATCH + 0x10,
                0,
                0,
                7,
                3,
                0x60,
            ],
        )
    } else {
        h.success(
            202,
            &[SCRATCH, access, SCRATCH + 0x100, SCRATCH + 0x10, 7, 0x60],
        )
    };
    assert_eq!(result, 0, "open {name}");
    h.get(SCRATCH)
}
fn read(h: &mut Harness, handle: u32, offset: u64, len: u32) -> Vec<u8> {
    h.runtime.memory().set_u64(SCRATCH + 0x400, offset).unwrap();
    assert_eq!(
        h.success(
            219,
            &[
                handle,
                0,
                0,
                0,
                SCRATCH + 0x10,
                SCRATCH + 0x2000,
                len,
                SCRATCH + 0x400
            ]
        ),
        0
    );
    assert_eq!(h.get(SCRATCH + 0x14), len);
    let mut bytes = vec![0; len as usize];
    h.runtime
        .memory()
        .read_bytes(SCRATCH + 0x2000, &mut bytes, 1)
        .unwrap();
    bytes
}
fn slot_record(slot: u32) -> u32 {
    RECORDS + slot * STRIDE
}
fn disk_checksum(path: &PathBuf, bytes: u32) -> u32 {
    // Independent scalar oracle reads the persisted payload, with fixed host
    // scratch. The retail MMX routine also applies a parity seed per qword.
    let mut file = std::fs::File::open(path).unwrap();
    file.seek(SeekFrom::Start(0x800)).unwrap();
    let mut left = bytes - 0x800;
    let mut buffer = vec![0; 64 * 1024];
    let mut checksum = 0;
    while left > 0 {
        let n = (left as usize).min(buffer.len());
        file.read_exact(&mut buffer[..n]).unwrap();
        for q in buffer[..n].as_chunks::<8>().0 {
            checksum ^= u32::from_le_bytes(q[..4].try_into().unwrap())
                ^ u32::from_le_bytes(q[4..].try_into().unwrap())
                ^ 0x8844_2211;
        }
        left -= n as u32;
    }
    checksum
}

const COMPARE_CHUNK: usize = 64 * 1024;

/// Compare the source's physical payload extent, not the preallocated cache
/// capacity. Report offsets only: assertion failures must not dump game bytes.
fn compare_payload(
    cache: &mut (impl Read + Seek),
    source_eof: u64,
    mut read_source: impl FnMut(u64, &mut [u8]) -> io::Result<usize>,
) -> io::Result<()> {
    if source_eof < 0x800 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source has no complete header",
        ));
    }
    cache.seek(SeekFrom::Start(0x800))?;
    let mut expected = vec![0; COMPARE_CHUNK];
    let mut actual = vec![0; COMPARE_CHUNK];
    let mut offset = 0x800;
    while offset < source_eof {
        let n = (source_eof - offset).min(COMPARE_CHUNK as u64) as usize;
        let mut filled = 0;
        while filled < n {
            let read = read_source(offset + filled as u64, &mut expected[filled..n])?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "source payload truncated",
                ));
            }
            filled += read;
        }
        cache.read_exact(&mut actual[..n])?;
        if let Some(index) = expected[..n]
            .iter()
            .zip(&actual[..n])
            .position(|(a, b)| a != b)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "payload differs at file offset {:#x}",
                    offset + index as u64
                ),
            ));
        }
        offset += n as u64;
    }
    Ok(())
}

struct PayloadOracle {
    disc: vfs::DiscBackend<File>,
    cache_root: PathBuf,
}
impl PayloadOracle {
    fn path(&self, slot: u32) -> PathBuf {
        self.cache_root
            .join(if slot < 2 { "Z" } else { "N" })
            .join(format!("cache{slot:03}.map"))
    }
    fn assert_payload(&mut self, slot: u32, name: &str) {
        let source = format!("maps\\{name}.map");
        let source_eof = self.disc.stat(&source).unwrap().end_of_file;
        let mut cache = File::open(self.path(slot)).unwrap();
        // This independent ISO handle bypasses Audited, guest completion counts,
        // fault injection, and the guest's already-closed source file object.
        compare_payload(&mut cache, source_eof, |offset, out| {
            self.disc
                .read_at(&source, offset, out)
                .map_err(io::Error::other)
        })
        .unwrap_or_else(|e| panic!("{name}, slot {slot}: {e}"));
    }
}

#[test]
fn payload_comparison_streams_short_reads_and_ignores_cache_capacity() {
    use io::Cursor;
    let source: Vec<u8> = (0..0x800 + 2 * COMPARE_CHUNK + 13)
        .map(|i| (i * 19 + 7) as u8)
        .collect();
    let mut cache = source.clone();
    cache[..0x800].fill(0); // Header edits are validated separately.
    cache.extend_from_slice(&[0xcc; 256]); // Cache capacity exceeds source EOF.
    let mut reader = Cursor::new(&source);
    let mut requests = 0;
    compare_payload(
        &mut Cursor::new(cache),
        source.len() as u64,
        |offset, out| {
            assert!(offset >= 0x800 && offset + out.len() as u64 <= source.len() as u64);
            assert!(out.len() <= COMPARE_CHUNK);
            requests += 1;
            reader.seek(SeekFrom::Start(offset))?;
            let n = out.len().min(997);
            reader.read(&mut out[..n])
        },
    )
    .unwrap();
    assert!(requests > 3);
}

#[test]
fn payload_comparison_rejects_checksum_preserving_corruption() {
    use io::Cursor;
    let source = vec![0; 0x800 + 2 * COMPARE_CHUNK + 13];
    let mut corrupt = source.clone();
    let first = 0x800 + COMPARE_CHUNK + 32;
    corrupt[first] ^= 1;
    corrupt[first + 4] ^= 1;
    let xor = |bytes: &[u8]| {
        bytes[0x800..]
            .as_chunks::<8>()
            .0
            .iter()
            .fold(0u64, |sum, word| sum ^ u64::from_le_bytes(*word))
    };
    // Two matching bit flips in separate dwords preserve the folded retail
    // XOR, lengths, and router sample, but must fail the byte oracle.
    let fold = |value: u64| value as u32 ^ (value >> 32) as u32;
    assert_eq!(fold(xor(&source)), fold(xor(&corrupt)));
    assert_eq!(&source[0x800..0x840], &corrupt[0x800..0x840]);
    let mut reader = Cursor::new(&source);
    let error = compare_payload(
        &mut Cursor::new(corrupt),
        source.len() as u64,
        |offset, out| {
            reader.seek(SeekFrom::Start(offset))?;
            reader.read(out)
        },
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains(&format!("{first:#x}")));
}

#[test]
fn payload_comparison_checks_tail_and_rejects_truncated_streams() {
    use io::Cursor;
    let source = vec![0; 0x800 + COMPARE_CHUNK + 3];
    for (tail_flip, short_source, short_cache, expected) in [
        (true, false, false, io::ErrorKind::InvalidData),
        (false, true, false, io::ErrorKind::UnexpectedEof),
        (false, false, true, io::ErrorKind::UnexpectedEof),
    ] {
        let mut cache = source.clone();
        if tail_flip {
            *cache.last_mut().unwrap() ^= 1;
        }
        if short_cache {
            cache.pop();
        }
        let mut reader = Cursor::new(&source[..source.len() - usize::from(short_source)]);
        let error = compare_payload(
            &mut Cursor::new(cache),
            source.len() as u64,
            |offset, out| {
                reader.seek(SeekFrom::Start(offset))?;
                reader.read(out)
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), expected);
    }
}

fn warm_slot(h: &mut Harness, name: &str) -> u32 {
    let path = format!("d:\\maps\\{name}.map\0");
    h.bytes(SCRATCH + 0x500, path.as_bytes());
    run(h, 0x214a60, &[], |cpu| cpu.eax = SCRATCH + 0x500)
}

fn warm_reload(
    h: &mut Harness,
    handles: &mut [u32],
    slot: u32,
    name: &str,
    audit: &Arc<Mutex<Audit>>,
) {
    // Drop the file object and resident header, retaining only the on-disk cache.
    let old = handles[slot as usize];
    assert_eq!(h.success(187, &[old]), 0);
    let drive = if slot < 2 { 'Z' } else { 'N' };
    let handle = open(h, &format!("{drive}:\\cache{slot:03}.map"), false, true);
    handles[slot as usize] = handle;
    h.set(slot_record(slot), handle);
    h.bytes(slot_record(slot) + 4, &[0; 0x800]);
    audit.lock().unwrap().io.clear();
    run(
        h,
        0x1a0f10,
        &[handle, 0x800, 0, 0, 1, SCRATCH + 0x704],
        |cpu| {
            cpu.edx = slot_record(slot) + 4;
            cpu.ebx = SCRATCH + 0x700;
        },
    );
    assert_eq!(h.get(SCRATCH + 0x704), 0x800);
    assert_eq!(
        run(h, 0x122870, &[], |cpu| cpu.esi = slot_record(slot) + 4) & 0xff,
        1
    );
    assert_eq!(warm_slot(h, name), slot, "persisted header is reusable");
    h.bytes(0x55bf28, format!("d:\\maps\\{name}.map\0").as_bytes());
    assert!(step(h));
    assert_eq!(h.get(STATE), 0, "warm request needs no copy");
    let events = &audit.lock().unwrap().io;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Io::Write(..) | Io::Read(_, 0x800.., _))),
        "warm reload copied payload"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e,Io::Read(p,_,_) if p.starts_with("maps\\"))),
        "warm reload accessed disc"
    );
}
fn routed_sample(h: &mut Harness, handle: u32, selector: u32) {
    let expected = read(h, handle, 0x800, 64);
    h.bytes(SCRATCH + 0x2000, &[0xcc; 64]);
    h.set(SCRATCH + 0x700, 0);
    h.set(SCRATCH + 0x704, 0);
    run(
        h,
        ROUTE,
        &[64, SCRATCH + 0x2000, SCRATCH + 0x704, SCRATCH + 0x700, 0, 1],
        |cpu| {
            cpu.ecx = (selector << 30) | 0x800;
        },
    );
    assert_eq!(h.get(SCRATCH + 0x704), 64);
    assert_eq!(h.runtime.memory().u8(SCRATCH + 0x700).unwrap(), 1);
    let mut actual = vec![0; 64];
    h.runtime
        .memory()
        .read_bytes(SCRATCH + 0x2000, &mut actual, 1)
        .unwrap();
    assert_eq!(actual, expected);
}

fn resource_lifetime(h: &mut Harness, handle: u32) {
    let before = baseline(h);
    let expected = read(h, handle, 0x800, 64);
    let buffer = h.success(165, &[0x1000]);
    assert_ne!(buffer, 0);
    h.bytes(buffer, &[0xcc; 64]);
    h.set(SCRATCH + 0x700, 0);
    h.set(SCRATCH + 0x704, 0);
    run(
        h,
        ROUTE,
        &[64, buffer, SCRATCH + 0x704, SCRATCH + 0x700, 0, 1],
        |cpu| cpu.ecx = 0x800,
    );
    assert_eq!(h.get(SCRATCH + 0x704), 64);
    assert_eq!(h.runtime.memory().u8(SCRATCH + 0x700).unwrap(), 1);
    let mut actual = vec![0; 64];
    h.runtime
        .memory()
        .read_bytes(buffer, &mut actual, 1)
        .unwrap();
    assert_eq!(actual, expected);
    h.success(171, &[buffer]);
    assert!(
        h.runtime.memory().u32(buffer).is_err(),
        "released resource buffer stays mapped"
    );
    assert_baseline(h, &before, "resource backing");
}
fn failed_copies(
    h: &mut Harness,
    handles: &mut [u32],
    audit: &Arc<Mutex<Audit>>,
    base: &[runtime::guest::Mapping],
    payloads: &mut PayloadOracle,
) {
    let name = "01a_tutorial";
    let survivor = read(h, handles[4], 0, 0x800);
    for fault in ["short read", "short write", "disk full", "checksum"] {
        let source = open(h, &format!("D:\\maps\\{name}.map"), false, false);
        {
            let mut a = audit.lock().unwrap();
            a.io.clear();
            match fault {
                "short read" => a.short_read = true,
                "short write" => a.short_write = true,
                "disk full" => a.full = true,
                "checksum" => a.corrupt = true,
                _ => unreachable!(),
            }
        }
        start(h, source, name);
        assert!(!finish(h, 10_000), "{fault}: copy incorrectly succeeded");
        assert_eq!(h.get(0x55bd08), 3, "{fault}: failure status");
        assert_eq!(
            warm_slot(h, name),
            u32::MAX,
            "{fault}: failed copy was reusable"
        );
        assert_eq!(
            h.get(slot_record(5) + 4),
            0,
            "{fault}: resident header stayed valid"
        );
        assert_eq!(
            &read(h, handles[5], 0, 4),
            &[0; 4],
            "{fault}: disk header stayed valid"
        );
        assert_eq!(
            read(h, handles[4], 0, 0x800),
            survivor,
            "current slot damaged"
        );
        payloads.assert_payload(4, "01b_spacestation");
        assert!(
            !audit
                .lock()
                .unwrap()
                .io
                .iter()
                .any(|e| matches!(e, Io::Write(_, 0, _, true))),
            "{fault}: published bad header"
        );
        assert_eq!(h.get(BUFFER), 0);
        assert_baseline(h, base, &format!("{fault}: allocation"));
        assert_eq!(
            h.success(211, &[source, SCRATCH + 0x10, SCRATCH + 0x800, 24, 5]),
            kernel::status::INVALID_HANDLE
        );
        eprintln!("fault {fault}: rejected, header invalid, current slot intact, staging released");
    }
    // Pause after the invalidation flush and reopen the destination. The
    // incomplete cache must remain invalid; then resume and recover normally.
    let source = open(h, &format!("D:\\maps\\{name}.map"), false, false);
    start(h, source, name);
    for _ in 0..32 {
        if h.get(STATE) == 11 {
            break;
        }
        assert!(step(h));
    }
    assert_eq!(h.get(STATE), 11);
    assert_eq!(h.success(187, &[handles[5]]), 0);
    handles[5] = open(h, "N:\\cache005.map", false, true);
    h.set(slot_record(5), handles[5]);
    assert_eq!(
        &read(h, handles[5], 0, 4),
        &[0; 4],
        "interrupted copy header"
    );
    assert_eq!(warm_slot(h, name), u32::MAX);
    assert!(finish(h, 10_000), "recovery copy");
    assert_eq!(warm_slot(h, name), 5);
    assert_eq!(h.get(BUFFER), 0);
    assert_baseline(h, base, "recovery");
    payloads.assert_payload(5, name);
    payloads.assert_payload(4, "01b_spacestation");
    eprintln!("recovery: tutorial and active Cairo payloads byte-exact against ISO");
}

fn start(h: &mut Harness, source: u32, name: &str) {
    // Enter at state 3: engine has selected a request and opened its source.
    // States 3..19 are untouched retail header/copy/checksum/publication/close.
    h.set(STATE, 3);
    h.set(0x55bf24, 0); // foreground priority, retail staging-size table
    h.set(0x55bd14, source);
    h.set(0x55bd10, u32::MAX);
    h.set(0x55bd08, 0);
    h.bytes(0x55bd21, format!("d:\\maps\\{name}.map\0").as_bytes());
    h.bytes(0x55be24, &[0; 0x104]);
}
fn step(h: &mut Harness) -> bool {
    run(h, COPY, &[], |_| {}) & 0xff != 0
}
fn finish(h: &mut Harness, max: usize) -> bool {
    let mut ok = true;
    for iteration in 0..max {
        if h.get(STATE) == 0 {
            return ok;
        }
        let state = h.get(STATE);
        let result = step(h);
        if !result {
            eprintln!(
                "copy failure state={state}, next={}, count={}, size={}, checksum={:#x}",
                h.get(STATE),
                h.get(COUNT),
                h.get(0x55bcfc),
                h.get(CHECKSUM)
            );
        }
        ok &= result;
        if iteration % 512 == 511 {
            eprintln!(
                "copy progress: {}/{} payload extent",
                h.get(COPIED),
                h.get(0x55bcfc)
            );
        }
    }
    panic!("copy did not finish: state {}", h.get(STATE));
}
fn baseline(h: &Harness) -> Vec<runtime::guest::Mapping> {
    h.runtime.memory().mappings().to_vec()
}
fn assert_baseline(h: &Harness, expected: &[runtime::guest::Mapping], label: &str) {
    let actual = baseline(h);
    let added: Vec<_> = actual.iter().filter(|m| !expected.contains(m)).collect();
    let removed: Vec<_> = expected.iter().filter(|m| !actual.contains(m)).collect();
    assert!(
        added.is_empty() && removed.is_empty(),
        "{label}: added {added:?}; removed {removed:?}"
    );
}

#[test]
#[ignore = "requires H2_XBE, H2_DISC (read-only retail ISO), and clang"]
fn campaign_cold_warm_cache_slots() {
    let scratch = Scratch::new();
    let xbe_path = std::env::var_os("H2_XBE").expect("set H2_XBE to your retail default.xbe");
    let disc_path = std::env::var_os("H2_DISC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../../disc/halo2.iso"));
    let mut payloads = PayloadOracle {
        disc: vfs::DiscBackend::open(&disc_path).unwrap(),
        cache_root: scratch.0.clone(),
    };
    let bytes = std::fs::read(xbe_path).unwrap();
    let xbe = xbe::Xbe::parse(&bytes).unwrap();
    assert_eq!(xbe.entry, 0x2d0aee, "retail address contract");
    let lib = generated::build(functions(&xbe), &scratch.0.join("generated"));
    let mut image = runtime::load_xbe_with(&bytes, &runtime::loader::LoadOptions::halo2()).unwrap();
    let kernel = kernel::Kernel::for_image(&mut image).unwrap();
    image
        .runtime
        .memory()
        .map_zeroed(SCRATCH, 0x10000, runtime::guest::Permissions::RW)
        .unwrap();
    let mut h = Harness {
        ctx: image.context(),
        runtime: image.runtime,
        kernel,
        cpu: image.cpu,
    };
    // SAFETY: the fresh host module uses the current ABI; lib outlives the runtime.
    unsafe {
        let module: libloading::Symbol<*const H2GenModule> = lib.get(b"h2gen_module").unwrap();
        let version: libloading::Symbol<unsafe extern "C" fn() -> u32> =
            lib.get(b"h2gen_module_abi_version").unwrap();
        h.runtime
            .edit_dispatch(|d| d.register_generated_module(&**module, *version))
            .unwrap();
    }
    install_adapters(&h);
    install_tls(&mut h, xbe.tls_directory().unwrap().expect("retail TLS"));
    let audit = Arc::new(Mutex::new(Audit::default()));
    let mut storage = vfs::Vfs::new();
    storage
        .mount(
            vfs::CDROM_DEVICE,
            Audited {
                inner: vfs::DiscBackend::open(&disc_path).unwrap(),
                audit: audit.clone(),
            },
        )
        .unwrap();
    for drive in ["Z", "N"] {
        let device = format!("\\Device\\Cache{drive}");
        let backend = vfs::HostBackend::new(
            scratch.0.join(drive),
            scratch.0.join(format!("meta-{drive}")),
        )
        .unwrap()
        .with_volume(drive, 0, 750 * 1024 * 1024);
        storage
            .mount(
                &device,
                Audited {
                    inner: backend,
                    audit: audit.clone(),
                },
            )
            .unwrap();
        storage
            .namespace
            .create_symbolic_link(&format!("\\??\\{drive}:"), &device)
            .unwrap();
    }
    storage
        .namespace
        .create_symbolic_link("\\??\\D:", vfs::CDROM_DEVICE)
        .unwrap();
    h.kernel.set_vfs(storage).unwrap();
    h.set(CURRENT, u32::MAX);
    // All six handles live from boot to shutdown. Capacity is logical disk space.
    let capacities = [
        0x0b40_0000,
        0x2080_0000,
        0x0500_0000,
        0x0500_0000,
        0x1180_0000,
        0x1180_0000,
    ];
    let mut handles = Vec::new();
    for (slot, capacity) in capacities.into_iter().enumerate() {
        let drive = if slot < 2 { 'Z' } else { 'N' };
        let handle = open(&mut h, &format!("{drive}:\\cache{slot:03}.map"), true, true);
        h.set(slot_record(slot as u32), handle);
        h.bytes(slot_record(slot as u32) + 4, &[0; 0x800]);
        h.runtime
            .memory()
            .set_u64(SCRATCH + 0x400, capacity)
            .unwrap();
        assert_eq!(
            h.success(226, &[handle, SCRATCH + 0x10, SCRATCH + 0x400, 8, 20]),
            0
        );
        handles.push(handle);
    }
    // File objects use reusable pool pages. Prime the one source-object slot
    // before measuring; freed pool capacity intentionally remains mapped.
    let prime = open(&mut h, "D:\\maps\\mainmenu.map", false, false);
    assert_eq!(h.success(187, &[prime]), 0);
    let base = baseline(&h);
    for (kind, capacity) in [
        (0, 0x1180_0000u32),
        (1, 0x0500_0000),
        (2, 0x0500_0000),
        (3, 0x0b40_0000),
        (4, 0x2080_0000),
    ] {
        assert_eq!(
            run(&mut h, SELECT, &[capacity + 1], |cpu| cpu.eax = kind),
            u32::MAX,
            "oversized map type {kind}"
        );
    }
    for (current, chosen) in [(4, 5), (5, 4)] {
        h.set(CURRENT, current);
        assert_eq!(
            run(&mut h, SELECT, &[69_169_664], |cpu| cpu.eax = 0),
            chosen,
            "avoid active slot"
        );
    }
    h.set(CURRENT, u32::MAX);
    for (name, size, map_type, expected_slot) in MAPS {
        let source = open(&mut h, &format!("D:\\maps\\{name}.map"), false, false);
        let header = read(&mut h, source, 0, 0x800);
        let declared = u32::from_le_bytes(header[8..12].try_into().unwrap());
        if size != 0 {
            assert_eq!(declared, size);
        }
        let size = declared;
        assert_eq!(
            u16::from_le_bytes(header[0x140..0x142].try_into().unwrap()) as u32,
            map_type
        );
        assert_eq!(
            run(&mut h, SELECT, &[size], |cpu| cpu.eax = map_type),
            expected_slot
        );
        audit.lock().unwrap().io.clear();
        h.bytes(HEADER, &header);
        assert_eq!(
            run(&mut h, 0x122870, &[], |cpu| cpu.esi = HEADER) & 0xff,
            1,
            "source header validation"
        );
        eprintln!("{name}: copying into slot {expected_slot}");
        start(&mut h, source, name);
        assert!(step(&mut h));
        let mut actual_header = vec![0; 0x800];
        h.runtime
            .memory()
            .read_bytes(HEADER, &mut actual_header, 1)
            .unwrap();
        assert!(actual_header == header, "header worker read");
        assert!(finish(&mut h, 10_000), "{name}: copy failed");
        assert_eq!(h.get(0x55bd08), 1, "completion published");
        assert_eq!(h.get(COPIED), size, "exact completed payload extent");
        assert_eq!(
            h.get(CHECKSUM),
            u32::from_le_bytes(header[0x2d0..0x2d4].try_into().unwrap()),
            "retail checksum"
        );
        assert_eq!(h.get(COUNT), 0x800, "final header completion count");
        assert_eq!(h.get(BUFFER), 0, "retail teardown freed staging");
        assert_baseline(&h, &base, &format!("{name}: guest allocation leaked"));
        let published = read(&mut h, handles[expected_slot as usize], 0, 0x800);
        // Retail updates only the timestamp and source-path fields.
        let mut expected_header = header.clone();
        expected_header[0x20..0x120].fill(0);
        let path = format!("d:\\maps\\{name}.map");
        expected_header[0x20..0x20 + path.len()].copy_from_slice(path.as_bytes());
        expected_header[0x14c..0x154].copy_from_slice(&1_000_000u64.to_le_bytes());
        assert!(
            published == expected_header,
            "published header differs at {:?}",
            published
                .iter()
                .zip(&expected_header)
                .position(|(a, b)| a != b)
        );
        assert_eq!(
            disk_checksum(&payloads.path(expected_slot), size),
            h.get(CHECKSUM),
            "persisted payload checksum"
        );
        payloads.assert_payload(expected_slot, name);
        let mut in_memory = vec![0; 0x800];
        h.runtime
            .memory()
            .read_bytes(slot_record(expected_slot) + 4, &mut in_memory, 1)
            .unwrap();
        assert!(
            published == in_memory,
            "disk and resident header publication"
        );
        assert_eq!(
            warm_slot(&mut h, name),
            expected_slot,
            "warm resident lookup"
        );
        let io = audit.lock().unwrap().io.clone();
        let payload_read: usize = io
            .iter()
            .filter_map(|e| match e {
                Io::Read(p, o, n) if p.starts_with("maps\\") && *o >= 0x800 => Some(n),
                _ => None,
            })
            .sum();
        let payload_write: usize = io
            .iter()
            .filter_map(|e| match e {
                Io::Write(_, o, n, _) if *o >= 0x800 => Some(n),
                _ => None,
            })
            .sum();
        assert_eq!(payload_read, size as usize - 0x800);
        assert_eq!(payload_write, payload_read);
        let path = format!("cache{expected_slot:03}.map");
        let writes: Vec<_> = io
            .iter()
            .filter(|e| matches!(e,Io::Write(p,_,_,_) if p==&path))
            .collect();
        assert!(
            matches!(writes[0], Io::Write(_, 0, 0x800, false)),
            "invalidate first"
        );
        assert!(
            matches!(writes.last().unwrap(), Io::Write(_, 0, 0x800, true)),
            "publish last"
        );
        let publish = io
            .iter()
            .rposition(|e| matches!(e,Io::Write(p,0,0x800,true) if p==&path))
            .unwrap();
        assert!(
            matches!(&io[publish-1],Io::Flush(p) if p==&path),
            "payload flushed before publication"
        );
        warm_reload(&mut h, &mut handles, expected_slot, name, &audit);
        assert_baseline(&h, &base, "warm reload");
        if map_type == 0 {
            h.set(CURRENT, expected_slot);
            resource_lifetime(&mut h, handles[expected_slot as usize]);
        }
        assert_eq!(
            h.success(211, &[source, SCRATCH + 0x10, SCRATCH + 0x800, 24, 5]),
            kernel::status::INVALID_HANDLE
        );
        eprintln!(
            "{name}: slot={expected_slot}, payload={payload_read}, no staging allocation retained"
        );
    }
    // Current, menu, shared, and SP-shared are handles, never guest pointers.
    for (selector, slot) in [(0, 4), (1, 2), (2, 0), (3, 1)] {
        routed_sample(&mut h, handles[slot], selector);
    }
    failed_copies(&mut h, &mut handles, &audit, &base, &mut payloads);
    assert_baseline(&h, &base, "shutdown");
    for handle in handles {
        assert_eq!(h.success(187, &[handle]), 0);
    }
    assert!(h.kernel.stop_reasons().is_empty());
    let shared = h.kernel.vfs().unwrap();
    let mut storage = shared.lock().unwrap();
    for device in ["\\Device\\CacheN", "\\Device\\CacheZ"] {
        storage
            .format_device(device)
            .expect("no cache file objects survive shutdown");
    }
    drop(storage);
    // Explicitly drop the runtime before unloading code and deleting scratch caches.
    drop(h);
    drop(lib);
}
