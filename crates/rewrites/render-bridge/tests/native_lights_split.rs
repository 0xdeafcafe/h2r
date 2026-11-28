//! Drive the actual layer-entry loop on an invented builder result.
//! The confirmed prefix must suppress exactly those handler calls, even
//! with a repeated item, an exclusion, and a D3D suffix sharing a shader.
#![cfg(feature = "native-render")]
use guest_layout::memory::{FlatMemory, GuestMemory};
use engine::render_loop::{Regs, Step, layer};
use native_render::camera::GuestRead;
use native_render::light_list::{HANDLER_RETURN, sorted_at};
use native_render::skip::{GEOMETRY_BUILD, GEOMETRY_DRAW, RENDER_RECORDS};

struct Read<'a>(&'a FlatMemory);
impl GuestRead for Read<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> { self.0.u32(addr).ok() }
}

fn dispatches(confirm: bool, writable: bool, caller: u32) -> Vec<u32> {
    let mut m = FlatMemory::new(0x400000, 0x280000);
    let (list, table, sentinel, stack) = (0x500000, 0x501000, 0x502000, 0x640000);
    let mut r = Regs { edi: list, ebx: 12, esp: stack, esi: 991, ..Default::default() };
    for (a, v) in [(list, 4), (list+4, 4), (list+12, 1), (list+20, table), (stack, 982),
        (sentinel+0x100, 0), (sentinel+0x200, u32::MAX), (RENDER_RECORDS+4, u32::MAX)] {
        m.set_u32(a, v).unwrap();
    }
    for id in 1..=3 {
        let item = table + id * layer::RECORD_SIZE;
        for (offset, value) in [(0, GEOMETRY_BUILD), (4, GEOMETRY_DRAW), (8, 0xabcd_1234),
            (12, id), (16, 1 << 12), (20, u32::from(id == 2) << 8)] {
            m.set_u32(item+offset, value).unwrap();
        }
    }
    let (mut machine, build) = layer::Entries::start(&mut r, &mut m).unwrap();
    assert!(matches!(build, Step::Call { target: layer::LAYER_BUILD, .. }));
    let base = r.eax;
    for (n, id) in [3, 1, 3, 2, 1].into_iter().enumerate() {
        let slot = base + n as u32 * layer::ENTRY_SIZE;
        m.set_u32(slot, id).unwrap();
        m.set_u32(slot+4, 7).unwrap();
        m.set_u32(slot+16, if n == 1 { sentinel+0x100 } else { sentinel }).unwrap();
    }
    r.eax = 5;
    r.esp += 16;
    let mut first = true;
    let mut d3d = Vec::new();
    loop {
        match machine.resume(&mut r, &mut m).unwrap() {
            Step::Return { to } => { assert_eq!((to, r.esi), (982, 991)); break; }
            Step::Call { target, site, cleanup } => {
                assert_eq!((target, site, cleanup), (GEOMETRY_DRAW, HANDLER_RETURN, 28));
                let esp = r.esp - 4;
                m.set_u32(esp, caller).unwrap();
                let mut skip = false;
                if first {
                    first = false;
                    let plan = sorted_at(&Read(&m), list, r.ebx, esp, r.esi, caller).unwrap();
                    // This fixture's second item requires its own handler.
                    let suffix = table + 2 * layer::RECORD_SIZE;
                    let prefix = plan.entries.iter().take_while(|e| e.handler_item != suffix).count();
                    assert_eq!(prefix, 2);
                    if confirm {
                        skip = plan.consume(prefix, &mut r.esi, |a, v| writable && m.set_u32(a, v).is_ok());
                    }
                }
                if !skip { d3d.push(m.u16(r.esi).unwrap().into()); }
                r.esp += cleanup;
            }
        }
    }
    for id in 1..=3 {
        assert_eq!(m.u32(table + id * layer::RECORD_SIZE + 16).unwrap(), 1 << 12);
    }
    d3d
}

#[test]
fn a_split_consumes_only_its_confirmed_sorted_prefix() {
    assert_eq!(dispatches(true, true, 0xfeed1234), [2, 1], "ported loop continuation");
    assert_eq!(dispatches(true, true, HANDLER_RETURN), [2, 1]);
    assert_eq!(dispatches(true, false, HANDLER_RETURN), [3, 3, 2, 1], "fault before commit");
    assert_eq!(dispatches(false, true, HANDLER_RETURN), [3, 3, 2, 1], "second copy refused");
}
