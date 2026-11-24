//! Predict the value returned by the guest VM's global-read helper, including
//! the bytes an external refresh preserves in the checkpointed global cell.
use super::hs_parity;
use engine::{GuestMemory, datum::layout, hs_pool, points};
use hs::vm::{Cell, HsType};
use runtime::{CpuAbi, Guest};

// The refresh reads the pointer held here for an unbound external string.
const EMPTY_STRING_POINTER: u32 = 0x0047_0010;

pub(super) fn install(b: &mut hooks::Builder) {
    hs_parity::observe(
        b,
        &points::HS_GLOBAL_READ,
        &hs_parity::GLOBALS,
        0,
        |cpu, mem| predict(mem, cpu.esi as u16),
        compare,
    );
    hs_parity::observe(
        b,
        &points::HS_GLOBAL_STORED,
        &hs_parity::GLOBAL_STORE,
        0,
        |cpu, mem| predict_store(mem, cpu.eax as u16),
        compare_store,
    );
}

struct Read {
    index: u16,
    address: u32,
    value: Cell,
}

fn cell_address(mem: &dyn GuestMemory, index: u16) -> Option<u32> {
    let external = index & 0x8000 != 0;
    let number = u32::from(index & 0x7fff);
    let slot = if external {
        if number >= hs_pool::EXTERNALS {
            return None;
        }
        number
    } else {
        hs_pool::EXTERNALS + number
    };
    let pool = mem.u32(hs_pool::GLOBALS).ok()?;
    if pool == 0 || mem.u32(pool.checked_add(layout::MAXIMUM_COUNT)?).ok()? <= slot {
        return None;
    }
    let data = mem.u32(pool.checked_add(layout::DATA)?).ok()?;
    if data == 0 {
        return None;
    }
    data.checked_add(slot * 8 + 4)
}

fn predict(mem: &dyn GuestMemory, index: u16) -> Option<Read> {
    let address = cell_address(mem, index)?;
    let mut value = Cell(mem.u32(address).ok()?);
    if index & 0x8000 != 0 {
        let descriptor = mem
            .u32(hs::EXTERNAL_GLOBAL_TABLE + u32::from(index & 0x7fff) * 4)
            .ok()?;
        if descriptor == 0 {
            return None;
        }
        let ty = HsType(mem.u16(descriptor).ok()?);
        if ty.0 >= HsType::COUNT {
            return None;
        }
        let width = ty.external_storage_width();
        if width != 0 {
            let source = mem.u32(descriptor.checked_add(4)?).ok()?;
            let refreshed = match (source, width) {
                (0, _) if ty == HsType::STRING => mem.u32(EMPTY_STRING_POINTER).ok()?,
                (0, _) => ty.unbound_external(),
                (_, 1) => u32::from(mem.u8(source).ok()?),
                (_, 2) => u32::from(mem.u16(source).ok()?),
                (_, 4) => mem.u32(source).ok()?,
                _ => return None,
            };
            value = value.store(ty, Cell(refreshed));
        }
    }
    Some(Read {
        index,
        address,
        value,
    })
}

fn compare(plan: &Read, cpu: &CpuAbi, mem: &Guest) -> Option<String> {
    let actual = mem.u32(plan.address).ok();
    (actual != Some(plan.value.0) || cpu.eax != plan.value.0).then(|| {
        format!(
            "global {:#06x} at {:#010x}: expected {:#010x}, cell {actual:x?}, eax {:#010x}",
            plan.index, plan.address, plan.value.0, cpu.eax
        )
    })
}

// A scalar store writes only the declared width. Object and object-list
// assignments also mark live objects, which this comparison cannot predict.
struct Store {
    index: u16,
    address: u32,
    bytes: [u8; 4],
    width: usize,
}

fn predict_store(mem: &Guest, index: u16) -> Option<Store> {
    if index & 0x8000 == 0 {
        return None;
    }
    let cell = cell_address(mem, index)?;
    let descriptor = mem
        .u32(hs::EXTERNAL_GLOBAL_TABLE + u32::from(index & 0x7fff) * 4)
        .ok()?;
    if descriptor == 0 {
        return None;
    }
    let ty = HsType(mem.u16(descriptor).ok()?);
    let width = usize::from(ty.external_storage_width());
    if width == 0 || ty.is_object() || ty == HsType::OBJECT_LIST {
        return None;
    }
    let address = mem.u32(descriptor.checked_add(4)?).ok()?;
    if address == 0 {
        return None;
    }
    let bytes = mem.u32(cell).ok()?.to_le_bytes();
    Some(Store {
        index,
        address,
        bytes,
        width,
    })
}

fn compare_store(expected: &Store, _: &CpuAbi, mem: &Guest) -> Option<String> {
    let mut bytes = [0u8; 4];
    let got = &mut bytes[..expected.width];
    let want = &expected.bytes[..expected.width];
    if mem.read_bytes(expected.address, got, 1).is_err() || got != want {
        Some(format!(
            "global {:#06x} binding at {:#010x}: expected {want:02x?}, read {got:02x?}",
            expected.index, expected.address
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ir::interp::Cpu;
    use runtime::guest::Permissions;

    const NUMBER: u32 = 2;
    const POOL: u32 = 0x0060_0000;
    const CELLS: u32 = POOL + 0x100;
    const DESCRIPTOR: u32 = POOL + 0x3000;
    const SOURCE: u32 = DESCRIPTOR + 8;
    const STACK: u32 = DESCRIPTOR + 0x100;
    const OLD: u32 = 0xabc1_def2;
    const BOUND: u32 = 0x1357_9bdf;
    const EMPTY: u32 = 0x7777_8888;

    fn memory() -> Guest {
        let mut mem = Guest::new();
        for (addr, bytes) in [
            (hs_pool::GLOBALS & !0xfff, 0x1000),
            (0x0047_0000, 0x4000),
            (POOL, 0x4000),
        ] {
            mem.map_zeroed(addr, bytes, Permissions::RW).unwrap();
        }
        mem.set_u32(hs_pool::GLOBALS, POOL).unwrap();
        mem.set_u32(POOL + layout::DATA, CELLS).unwrap();
        mem.set_u32(POOL + layout::MAXIMUM_COUNT, hs_pool::EXTERNALS + 3)
            .unwrap();
        mem.set_u32(hs::EXTERNAL_GLOBAL_TABLE + NUMBER * 4, DESCRIPTOR)
            .unwrap();
        mem.set_u32(EMPTY_STRING_POINTER, EMPTY).unwrap();
        mem.set_u32(SOURCE, BOUND).unwrap();
        mem.set_u32(CELLS + NUMBER * 8 + 4, OLD).unwrap();
        mem.set_u32(CELLS + (hs_pool::EXTERNALS + NUMBER) * 8 + 4, !OLD)
            .unwrap();
        mem
    }

    #[test]
    fn global_read_preserves_narrow_cells_and_string_pointers() {
        let mut mem = memory();
        assert_eq!(predict(&mem, NUMBER as u16).unwrap().value, Cell(!OLD));
        for (ty, source, expected) in [
            (HsType::SHORT, SOURCE, (OLD & 0xffff0000) | (BOUND & 0xffff)),
            (HsType::BOOLEAN, 0, OLD & !0xff),
            (HsType::STRING, SOURCE, BOUND),
            (HsType::STRING, 0, EMPTY),
            (HsType::LAST_OBJECT_NAME, SOURCE, OLD),
        ] {
            mem.set_u16(DESCRIPTOR, ty.0).unwrap();
            mem.set_u32(DESCRIPTOR + 4, source).unwrap();
            let read = predict(&mem, NUMBER as u16 | 0x8000).unwrap();
            assert_eq!(read.value.0, expected);
            assert_eq!(mem.u32(read.address).unwrap(), OLD);
        }
        assert!(predict(&mem, 3).is_none());
        assert!(predict(&mem, 0xffff).is_none());
    }

    #[test]
    fn check_bound_scalar_store_by_declared_width() {
        let mut mem = memory();
        mem.set_u16(DESCRIPTOR, HsType::SHORT.0).unwrap();
        mem.set_u32(DESCRIPTOR + 4, SOURCE).unwrap();
        let store = predict_store(&mem, 0x8000 | NUMBER as u16).unwrap();
        let cpu = CpuAbi::default();
        assert!(compare_store(&store, &cpu, &mem).is_some());
        mem.set_u16(SOURCE, OLD as u16).unwrap();
        assert!(compare_store(&store, &cpu, &mem).is_none());
        mem.set_u16(DESCRIPTOR, HsType::OBJECT_LIST.0).unwrap();
        assert!(predict_store(&mem, 0x8000 | NUMBER as u16).is_none());
    }

    #[test]
    fn global_read_against_lifted_guest() {
        let Some(image) = lifted::image::Image::get() else {
            return;
        };
        let mut code = image.original();
        let mut ram = image.ram.clone();
        let mut mem = memory();
        for (addr, len) in [
            (hs_pool::GLOBALS, 4),
            (EMPTY_STRING_POINTER, 4),
            (hs::EXTERNAL_GLOBAL_TABLE + NUMBER * 4, 4),
            (POOL, 0x4000),
        ] {
            let mut data = vec![0; len];
            mem.read_bytes(addr, &mut data, 1).unwrap();
            assert!(ram.load(addr, &data));
        }
        for index in [NUMBER as u16, NUMBER as u16 | 0x8000] {
            for ty in 0..HsType::COUNT {
                for source in [0, SOURCE] {
                    mem.set_u16(DESCRIPTOR, ty).unwrap();
                    mem.set_u32(DESCRIPTOR + 4, source).unwrap();
                    let read = predict(&mem, index).unwrap();
                    assert!(ram.load(DESCRIPTOR, &ty.to_le_bytes()));
                    assert!(ram.load(DESCRIPTOR + 4, &source.to_le_bytes()));
                    assert!(ram.load(read.address, &mem.u32(read.address).unwrap().to_le_bytes()));
                    let exit = runtime::exec::HARNESS_EXIT;
                    assert!(ram.load(STACK, &exit.to_le_bytes()));
                    let mut cpu = Cpu {
                        eip: points::HS_GLOBAL_READ.base,
                        ..Cpu::default()
                    };
                    cpu.gpr[4] = STACK;
                    cpu.gpr[6] = 0x1234_0000 | u32::from(index);
                    lifted::code::run_until(&mut code, &mut cpu, &mut ram, 120, |c| c.eip == exit)
                        .unwrap();
                    let pos = read.address as usize;
                    assert_eq!(
                        (cpu.gpr[0], &ram.bytes[pos..pos + 4]),
                        (read.value.0, &read.value.0.to_le_bytes()[..]),
                        "index {index:x}, type {ty}, source {source:x}"
                    );
                    assert_eq!(cpu.gpr[4], STACK + 4);
                }
            }
        }
    }
}
