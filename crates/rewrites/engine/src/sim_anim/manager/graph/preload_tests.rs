//! Compare the graph table with the ordered guest-memory walk at both
//! cache boundaries. A zero count bypasses the table on the reference run.
use crate::sim_anim::manager;
use runtime::{CpuAbi, Guest, TransferAbi, guest::Permissions};

const GRAPH: u32 = 0x10000;
const RESOURCES: u32 = GRAPH + 0x400;
const SP: u32 = GRAPH + 0x800;
const CLIPS: u32 = 0x20000;
const ALIAS: u32 = 0x30000;
const ROW: u32 = 108;
const FIRST_RETURN: u32 = 0x1dda4e;
const LINK_RETURN: u32 = 0x1dda8d;
const EXIT: u32 = 0x45000;

fn fixture(count: u32) -> Guest {
    let mut g = Guest::new();
    g.map(GRAPH, 0x1000, 0, Permissions::RW).unwrap();
    for base in [CLIPS, ALIAS] {
        g.map(base, 0x1000, 0x1000, Permissions::RW).unwrap();
    }
    g.map(0x1dd9d0, 0xe0, 0x3000, Permissions::RX).unwrap();
    g.set_u32(GRAPH + 0x2c, count).unwrap();
    g.set_u32(GRAPH + 0x30, CLIPS).unwrap();
    g.set_u32(GRAPH + 176, RESOURCES).unwrap();
    for (i, next) in [1, 2, 3, 0xffff].into_iter().enumerate() {
        let clip = CLIPS + i as u32 * ROW;
        g.set_u16(clip + 50, next).unwrap();
        g.set_u8(clip + 22, 16).unwrap();
        g.set_u32(clip + 36, i as u32).unwrap();
    }
    g.set_u8(RESOURCES + 2 * 20 + 16, 0x83).unwrap();
    for (offset, value) in [(0, 0xa101), (4, 0xb202), (8, EXIT)] {
        g.set_u32(SP + offset, value).unwrap();
    }
    g
}

fn boundaries(count: u32) -> Vec<(TransferAbi, CpuAbi, [u8; 24])> {
    let mut g = fixture(count);
    let mut captures = Vec::new();
    let cpu = CpuAbi {
        eax: 0xf1a2b3c4,
        ecx: 0x7823f6,
        esi: CLIPS,
        edi: GRAPH,
        esp: SP,
        ..Default::default()
    };
    for repetition in 0..2 {
        let mut c = cpu;
        let request = manager::run(&mut c, &mut g, FIRST_RETURN).unwrap().unwrap();
        assert_eq!(request, TransferAbi::call(0x1236f0, LINK_RETURN));
        assert_eq!((c.esi, c.eax), (CLIPS + ROW, RESOURCES + 2 * 20));
        captures.push((request, c, g.read(SP - 4).unwrap()));

        // The callback returns through the same saved frame. On the last
        // run it edits a link through a physical alias, skipping clip 2.
        if repetition == 1 {
            g.set_u16(ALIAS + ROW + 50, 0).unwrap();
            g.set_u16(ALIAS + 50, 0xffff).unwrap();
        }
        c.esp += 4;
        let done = manager::run(&mut c, &mut g, LINK_RETURN).unwrap().unwrap();
        assert_eq!(done, TransferAbi::ret(EXIT));
        let end = if repetition == 0 { CLIPS + 3 * ROW } else { CLIPS };
        assert_eq!((c.eax, c.ecx), (end, end));
        captures.push((done, c, g.read(SP - 4).unwrap()));
    }

    // No successor: the caller's EAX and ECX are live across the walk.
    let mut terminal = CpuAbi { esi: CLIPS + 3 * ROW, ..cpu };
    let done = manager::run(&mut terminal, &mut g, FIRST_RETURN).unwrap().unwrap();
    assert_eq!((terminal.eax, terminal.ecx), (cpu.eax, cpu.ecx));
    captures.push((done, terminal, g.read(SP - 4).unwrap()));
    captures
}

#[test]
fn cold_warm_and_aliased_callback_match_the_ordered_preload() {
    assert_eq!(boundaries(4), boundaries(0));
}
