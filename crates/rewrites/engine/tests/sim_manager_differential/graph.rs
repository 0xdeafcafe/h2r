//! Inherited tags, clip chains with a mutating cache callback, sampling
//! views and signed name lookups with duplicate and missing keys.
use super::*;

const GRAPH: u32 = DATA + 0x1000;
const PARENT: u32 = DATA + 0x2000;
const CLIPS: u32 = DATA + 0x3000;
const TAGS: u32 = DATA + 0x4000;
const RESOURCES: u32 = DATA + 0x5000;
const VIEW: u32 = DATA + 0x6000;
const PARENTS: u32 = DATA + 0x7000;

fn graphs(mem: &mut FlatMemory) {
    word(mem, GRAPH + 48, CLIPS);
    word(mem, PARENT + 48, CLIPS);
    word(mem, GRAPH + 76, 1);
    word(mem, GRAPH + 80, PARENTS);
    word(mem, PARENTS + 4, 0x7934_0001);
    word(mem, 0x4e3b44, TAGS);
    word(mem, TAGS + 16 + 8, PARENT);
    for index in 0..4 {
        let clip = CLIPS + index * 108;
        half(mem, clip + 48, 0xffff);
        half(mem, clip + 50, 0xffff);
    }
}

pub(super) fn cases(h: &mut Harness<'_>) {
    for (seed, handle) in [0xffff_ffff, 0x0001_ffff, 0x0002_0000].into_iter().enumerate() {
        h.case(0x1daea0, seed as u32, |c, m| {
            graphs(m);
            c.edx = GRAPH;
            word(m, STACK + 4, handle);
        }, |target, _, c, _| {
            assert_eq!(target, 0x1dd9d0);
            c.eax = 0x6314_00af;
            4
        });
    }

    for seed in 0..5 {
        h.case(0x1dd9d0, seed, |c, m| {
            graphs(m);
            c.edx = GRAPH;
            // The original clip aliases 1, which chains through 2 to 3.
            half(m, CLIPS + 48, 1);
            half(m, CLIPS + 108 + 50, 2);
            half(m, CLIPS + 2 * 108 + 50, 3);
            for graph in [GRAPH, PARENT] {
                word(m, graph + 176, RESOURCES);
            }
            m.bytes[(CLIPS + 2 * 108 + 22) as usize] = 16;
            m.bytes[(RESOURCES + 16) as usize] = u8::from(seed != 3);
            m.bytes[0x510c20] = u8::from(seed != 0);
            m.bytes[0x510c21] = u8::from(seed != 1);
            word(m, CLIPS + 2 * 108 + 36, 0);
            let handle = match seed { 2 => 0xffff_ffff, 3 => 0x0000_ffff, _ => 0 };
            word(m, STACK + 4, handle);
        }, |target, _, c, m| {
            match target {
                0x1dafc0 => c.eax = PARENT,
                0x1dd840 => c.eax = 0x2573_7789,
                0x1236f0 => {
                    // Reload the current link on return: the resource
                    // request can change which clip follows this one.
                    half(m, CLIPS + 108 + 50, 3);
                    c.ecx = 0xdead_cafe;
                }
                _ => panic!("unexpected graph call {target:#x}"),
            }
            0
        });
    }

    for seed in 0..2 {
        h.case(0x1ddb40, seed, |c, m| {
            c.edi = VIEW;
            word(m, STACK + 4, GRAPH);
            word(m, STACK + 8, if seed == 0 { 0xffff_ffff } else { 0x0001_ffff });
            m.bytes[(CLIPS + 17) as usize] = 0xe9;
            m.bytes[(CLIPS + 19) as usize] = 17;
            half(m, CLIPS + 20, 0x8701);
        }, |target, _, c, _| {
            c.eax = match target {
                0x1daea0 => CLIPS,
                0x1dd7c0 => DATA + 0x7800,
                _ => panic!("unexpected sample-view call {target:#x}"),
            };
            4
        });
    }

    let names = [-20, -5, -1, 0, 3, 17, 102];
    for (seed, wanted) in [-20, -5, 102, 3, -21, 4, 103, 0, -1].into_iter().enumerate() {
        h.case(0x1dd560, seed as u32, |_, m| {
            word(m, STACK + 4, GRAPH);
            word(m, STACK + 8, wanted as u32);
            word(m, STACK + 12, 20);
            word(m, GRAPH, if seed == 8 { 0 } else { names.len() as u32 });
            word(m, GRAPH + 4, CLIPS);
            for (i, &name) in names.iter().enumerate() {
                word(m, CLIPS + 20 * i as u32, name as u32);
            }
            if seed == 7 {
                word(m, CLIPS + 40, 0); // both indices 2 and 3 match
            }
        }, |target, _, _, _| panic!("lookup called {target:#x}"));
    }
}
