mod common;
use common::*;
use kernel::status;
use runtime::{CpuAbi, Dispatch, ExecCtx, Guest, LoadedImage, Runtime, guest::Permissions};

const TABLE: u32 = DATA + 0x400;
const PRELOAD: u32 = TABLE;
const ON_DEMAND: u32 = TABLE + 56;
const COUNTS: u32 = DATA + 0x800;
const SECTION_VA: u32 = 0x0030_0800;
const SECTION_SIZE: u32 = 0x1000;

/// A loaded image whose header (at DATA = 0x10000) lists one PRELOAD section
/// and one on-demand section spanning two read-only pages.
fn loaded() -> Harness {
    let mut memory = Guest::new();
    memory.map_zeroed(DATA, 0x20000, Permissions::RW).unwrap();
    memory.map_zeroed(CODE, 0x10000, Permissions::RWX).unwrap();
    memory
        .map_zeroed(STACK_BOTTOM, STACK_TOP - STACK_BOTTOM, Permissions::RW)
        .unwrap();
    for page in [0x30_0000, 0x30_1000] {
        memory.map_zeroed(page, 0x1000, Permissions::RW).unwrap();
    }
    memory.set_u32(DATA + 0x11c, 2).unwrap();
    memory.set_u32(DATA + 0x120, TABLE).unwrap();
    for (header, flags, va, head, tail) in [
        (PRELOAD, 2, CODE, COUNTS, COUNTS),
        (ON_DEMAND, 0, SECTION_VA, COUNTS + 2, COUNTS + 4),
    ] {
        memory.set_u32(header, flags).unwrap();
        memory.set_u32(header + 4, va).unwrap();
        memory.set_u32(header + 8, SECTION_SIZE).unwrap();
        memory.set_u32(header + 0x1c, head).unwrap();
        memory.set_u32(header + 0x20, tail).unwrap();
    }
    let pattern: Vec<u8> = (0..SECTION_SIZE).map(|i| i as u8 ^ 0x5a).collect();
    memory.write_bytes(SECTION_VA, &pattern, 1).unwrap();
    for page in [0x30_0000, 0x30_1000] {
        memory.protect(page, Permissions::READ).unwrap();
    }
    memory
        .set_u32(STACK_TOP - 4, runtime::exec::HARNESS_EXIT)
        .unwrap();
    let mut image = LoadedImage {
        runtime: Runtime::new(memory, Dispatch::default()),
        cpu: CpuAbi {
            eip: CODE,
            esp: STACK_TOP - 4,
            ..CpuAbi::xbox_thread()
        },
        tls: None,
        tls_data_size: None,
        stack_bottom: STACK_BOTTOM,
        stack_top: STACK_TOP,
        data_exports: Default::default(),
        permission_notes: Vec::new(),
    };
    let kernel = kernel::Kernel::for_image(&mut image).unwrap();
    Harness {
        ctx: ExecCtx::new(image.runtime.clone(), 1),
        runtime: image.runtime,
        kernel,
        cpu: image.cpu,
    }
}
fn counts(h: &Harness) -> [u16; 3] {
    let m = h.runtime.memory();
    [0, 2, 4].map(|i| m.u16(COUNTS + i).unwrap())
}

#[test]
fn attachment_counts_preloaded_sections_like_the_retail_loader() {
    let h = loaded();
    assert_eq!(h.get(PRELOAD + 0x18), 1);
    assert_eq!(h.get(ON_DEMAND + 0x18), 0);
    // Head and tail coincide for the preload section: counted once.
    assert_eq!(counts(&h), [1, 0, 0]);
}

#[test]
fn loading_an_on_demand_section_restores_pristine_bytes_and_counts_pages() {
    let mut h = loaded();
    // The guest scribbles over the (discarded) section through a writable view.
    h.runtime
        .memory()
        .protect(0x30_1000, Permissions::RW)
        .unwrap();
    h.bytes(0x30_1000, &[0xee; 16]);
    h.runtime
        .memory()
        .protect(0x30_1000, Permissions::READ)
        .unwrap();
    assert_eq!(h.success(327, &[ON_DEMAND]), status::SUCCESS);
    assert_eq!(h.get(ON_DEMAND + 0x18), 1);
    assert_eq!(counts(&h), [1, 1, 1]);
    let mut bytes = vec![0; SECTION_SIZE as usize];
    h.runtime
        .memory()
        .read_bytes(SECTION_VA, &mut bytes, 1)
        .unwrap();
    assert!(bytes.iter().enumerate().all(|(i, b)| *b == i as u8 ^ 0x5a));
    // Page permissions are unchanged.
    assert!(h.runtime.memory().set_u8(SECTION_VA, 0).is_err());
    // A second load only counts.
    assert_eq!(h.success(327, &[ON_DEMAND]), status::SUCCESS);
    assert_eq!(h.get(ON_DEMAND + 0x18), 2);
    assert_eq!(counts(&h), [1, 1, 1]);
    assert_eq!(h.success(328, &[ON_DEMAND]), status::SUCCESS);
    assert_eq!(counts(&h), [1, 1, 1]);
    assert_eq!(h.success(328, &[ON_DEMAND]), status::SUCCESS);
    assert_eq!(h.get(ON_DEMAND + 0x18), 0);
    assert_eq!(counts(&h), [1, 0, 0]);
    assert_eq!(h.success(328, &[ON_DEMAND]), status::INVALID_PARAMETER);
    // Preload sections can be released and re-referenced, but a discarded
    // preload section has no retained image bytes.
    assert_eq!(h.success(328, &[PRELOAD]), status::SUCCESS);
    h.stopped(327, &[PRELOAD]);
    h.stopped(327, &[DATA + 0x900]);
}
