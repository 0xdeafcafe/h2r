mod common;
use common::*;
use kernel::image;
use runtime::{
    Dispatch, Guest, Runtime,
    guest::{Access, Permissions},
};

#[test]
fn retail_style_last_section_probe_finds_no_discardable_init() {
    let h = Harness::new();
    let memory = h.runtime.memory();
    assert_eq!(memory.u16(image::BASE).unwrap(), 0x5a4d);
    // Same address arithmetic/field reads as the retail routine at 0x2d1e10,
    // expressed as a synthetic probe, without retaining any executable bytes.
    let nt = image::BASE + memory.u32(image::BASE + 0x3c).unwrap();
    assert_eq!(memory.u32(nt).unwrap(), 0x4550);
    assert_eq!(memory.u16(nt + 4).unwrap(), 0x14c);
    let count = memory.u16(nt + 6).unwrap() as u32;
    let optional_size = memory.u16(nt + 20).unwrap() as u32;
    let last = nt + 24 + optional_size + (count - 1) * 40;
    assert_eq!(last, image::BASE + image::SECTION_OFFSET);
    assert_ne!(memory.read::<4>(last).unwrap(), *b"INIT");
    assert_eq!(memory.read::<8>(last).unwrap(), *b".rdata\0\0");
    let section = image::BASE + memory.u32(last + 12).unwrap();
    memory
        .check(
            section,
            memory.u32(last + 8).unwrap() as usize,
            Access::Read,
            1,
        )
        .unwrap();
    assert_eq!(memory.u32(nt + 24 + 56).unwrap(), image::SIZE);
    assert_eq!(memory.u32(nt + 24 + 96).unwrap(), 0); // no export RVA
    assert!(
        memory
            .check(image::BASE, image::SIZE as usize, Access::Write, 1)
            .is_err()
    );
    assert!(memory.check(section, 1, Access::Execute, 1).is_err());
}

#[test]
fn image_collision_is_reported_without_overwriting_or_installing_handlers() {
    let mut memory = Guest::new();
    memory.map_zeroed(image::BASE, 4, Permissions::RW).unwrap();
    memory.set_u32(image::BASE, 0x12345678).unwrap();
    let rt = Runtime::new(memory, Dispatch::default());
    assert!(kernel::Kernel::install(&rt, kernel::Config::default()).is_err());
    assert_eq!(rt.memory().u32(image::BASE).unwrap(), 0x12345678);
    assert_eq!(rt.memory().mappings().len(), 1);
}

#[test]
#[ignore = "requires the user-supplied retail XBE"]
fn retail_loader_data_exports_are_relocated_out_of_the_kernel_header_range() {
    // H2_XBE names the user's retail image (absolute path); the default is
    // the repository checkout's disc/ directory.
    let path = std::env::var("H2_XBE").unwrap_or_else(|_| "../../disc/default.xbe".into());
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut loaded = runtime::load_xbe(&bytes).unwrap();
    let kernel = kernel::Kernel::for_image(&mut loaded).unwrap();
    assert_eq!(
        loaded.runtime.memory().u32(image::BASE + 0x3c).unwrap(),
        image::NT_OFFSET
    );
    for (&ordinal, &address) in &loaded.data_exports {
        assert!(
            !(image::BASE..image::BASE + image::SIZE).contains(&address),
            "DATA {ordinal}"
        );
    }
    assert_ne!(kernel.kpcr(), image::BASE);
    // ObReferenceObjectByHandle type matching uses these DATA exports
    // (SetThreadPriority passes [0x4115b0] = PsThreadObjectType).
    assert!(loaded.data_exports.contains_key(&259));
    let m = loaded.runtime.memory();
    // Boot DATA: three cache partitions; the image launched from the DVD.
    assert_eq!(m.u32(loaded.data_exports[&40]).unwrap(), 3);
    let name = loaded.data_exports[&326];
    let mut text = vec![0; m.u16(name).unwrap() as usize];
    m.read_bytes(m.u32(name + 4).unwrap(), &mut text, 1)
        .unwrap();
    assert_eq!(text, b"\\Device\\CdRom0\\default.xbe");
    // Section accounting: .text (preload) holds one reference; the
    // on-demand $$XTIMAGE/$$XSIMAGE/.XTLID sections none.
    let table = m.u32(0x10120).unwrap();
    for i in 0..m.u32(0x1011c).unwrap() {
        let header = table + i * 56;
        let mut name = [0u8; 9];
        m.read_bytes(m.u32(header + 0x14).unwrap(), &mut name, 1)
            .unwrap();
        let preload = m.u32(header).unwrap() & 2 != 0;
        assert_eq!(
            m.u32(header + 0x18).unwrap(),
            preload as u32,
            "{}",
            String::from_utf8_lossy(&name)
        );
    }
}

#[test]
fn conflicting_data_is_copied_and_only_its_thunk_is_relocated() {
    use runtime::{CpuAbi, LoadedImage, guest::Permissions};
    let mut memory = Guest::new();
    memory.map_zeroed(0x10000, 4096, Permissions::RW).unwrap();
    memory.map_zeroed(0x20000, 4096, Permissions::RW).unwrap();
    memory.map_zeroed(image::BASE, 8, Permissions::RW).unwrap();
    memory.set_u64(image::BASE, 0x12345678_9abcdef0).unwrap();
    memory.set_u32(0x10128, CODE ^ 0xa8fc57ab).unwrap();
    memory.set_u32(0x10158, 0x20000 ^ 0x5b6d40b6).unwrap();
    memory.set_u32(0x20000, image::BASE).unwrap();
    memory
        .set_u32(0x20004, runtime::kernel::function_token(255))
        .unwrap();
    memory.set_u32(0x20008, image::BASE).unwrap();
    memory.protect(0x20000, Permissions::READ).unwrap();
    let mut loaded = LoadedImage {
        runtime: Runtime::new(memory, Dispatch::default()),
        cpu: CpuAbi {
            eip: CODE,
            ..CpuAbi::xbox_thread()
        },
        tls: None,
        tls_data_size: None,
        stack_bottom: STACK_BOTTOM,
        stack_top: STACK_TOP,
        data_exports: [(41, image::BASE)].into(),
        permission_notes: Vec::new(),
    };
    kernel::Kernel::for_image(&mut loaded).unwrap();
    let address = loaded.data_exports[&41];
    assert_ne!(address, image::BASE);
    let m = loaded.runtime.memory();
    assert_eq!(m.u64(address).unwrap(), 0x12345678_9abcdef0);
    assert_eq!(m.u32(0x20000).unwrap(), address);
    assert_eq!(m.u32(0x20008).unwrap(), address);
    assert_eq!(
        m.u32(0x20004).unwrap(),
        runtime::kernel::function_token(255)
    );
    assert!(m.check(0x20000, 4, Access::Write, 1).is_err());
    assert!(
        m.check(image::BASE, image::SIZE as usize, Access::Write, 1)
            .is_err()
    );
    assert_eq!(m.u16(image::BASE).unwrap(), 0x5a4d);
}

/// `load_xbe_with(.., &LoadOptions::halo2())` reserves the title arena; the
/// kernel's own placements avoid it, and the game's XPhysicalAlloc
/// (0x12b43e: MmAllocateContiguousMemoryEx(0x30e4000, 0x61000, 0x3144fff, 0,
/// PAGE_READWRITE)) gets exactly the arena at 0x80061000.
#[test]
#[ignore = "requires the user-supplied retail XBE"]
fn retail_title_arena_is_kept_free_and_handed_to_xphysicalalloc() {
    use runtime::loader::{HALO2_PHYSICAL_ARENA, LoadOptions};
    let path = std::env::var("H2_XBE").unwrap_or_else(|_| "../../disc/default.xbe".into());
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut loaded = runtime::load_xbe_with(&bytes, &LoadOptions::halo2()).unwrap();
    let kernel = kernel::Kernel::for_image(&mut loaded).unwrap();
    let window = HALO2_PHYSICAL_ARENA;
    {
        let m = loaded.runtime.memory();
        for x in m.mappings() {
            if (0x8006_1000..0x8314_5000).contains(&x.virtual_start) {
                continue;
            }
            assert!(
                x.physical_start as u64 + x.len as u64 <= window.start as u64
                    || x.physical_start >= window.end,
                "{:#x} -> {:#x}",
                x.virtual_start,
                x.physical_start
            );
        }
    }
    let call = |ordinal: u32, args: &[u32]| {
        let export = *kernel::EXPORTS
            .iter()
            .find(|e| e.ordinal == ordinal)
            .unwrap();
        let mut cpu = runtime::CpuAbi {
            esp: loaded.stack_top - 0x100,
            ..loaded.cpu
        };
        frame(
            &loaded.runtime,
            &mut cpu,
            export,
            args,
            runtime::exec::HARNESS_EXIT,
        );
        let mut ctx = runtime::ExecCtx::new(std::sync::Arc::clone(&loaded.runtime), 1);
        let t = runtime::Runner { max_transfers: 10 }.run(
            &mut cpu,
            &mut ctx,
            kernel.entry(ordinal).unwrap(),
        );
        assert!(
            matches!(ctx.event(t.event_id), Some(runtime::Event::HarnessExit)),
            "{ordinal}: {:?}",
            ctx.event(t.event_id)
        );
        cpu.eax
    };
    let arena = call(166, &[0x30e_4000, 0x6_1000, 0x314_4fff, 0, 4]);
    assert_eq!(arena, 0x8006_1000);
    assert_eq!(call(173, &[arena + 0x123]), 0x6_1123);
    // A carver's XPhysicalProtect(.., 0x404) on part of the arena.
    call(182, &[0x8300_0000, 0x10_0000, 0x404]);
    assert_eq!(call(179, &[0x8300_0000]), 0x404);
    assert_eq!(call(179, &[0x8006_1000]), 4);
    loaded.runtime.memory().set_u32(0x8314_4ffc, 1).unwrap();
}
