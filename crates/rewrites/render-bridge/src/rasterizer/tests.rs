use super::*;
use engine::entry::{self, Regs};
use engine::testing::FakeGuest;

const BLOCK: u32 = 0x0070_0000;
const STACK: u32 = 0x0030_0800;

/// The image's globals, a block and a stack, with a 640 x 480 window.
fn guest() -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(0x0040_0000, 0x0013_0000).map(BLOCK, 0x1000).map(0x0030_0000, 0x1000).map(0x8060_0000, 0x4_0000);
    g.set_u16(WINDOW, 0).unwrap();
    g.set_u16(WINDOW + 2, 0).unwrap();
    g.set_u16(WINDOW + 4, 480).unwrap();
    g.set_u16(WINDOW + 6, 640).unwrap();
    g
}

#[test]
fn the_surfaces_are_the_windows_size_and_name_no_ram() {
    let mut g = guest();
    surfaces(&mut g, BLOCK).unwrap();
    let w = |g: &FakeGuest, at: u32| g.u32(at).unwrap();
    assert_eq!(w(&g, BLOCK + BLOCK_BACK + 0x10), TEXTURE_SIZE, "640 x 480, pitch 2560");
    assert_eq!(w(&g, BLOCK + BLOCK_BACK + 4), BACK_DATA);
    assert_eq!(w(&g, BLOCK + BLOCK_DISPLAY + 4), DISPLAY_DATA);
    assert_eq!(w(&g, BLOCK + BLOCK_DEPTH + 4), DEPTH_DATA);
    assert_eq!(w(&g, BLOCK + BLOCK_DEPTH + 0xc), LIN_D24S8);
    g.set_u8(MODE_FLOAT_DEPTH, 1).unwrap();
    surfaces(&mut g, BLOCK).unwrap();
    assert_eq!(w(&g, BLOCK + BLOCK_DEPTH + 0xc), LIN_F24S8);
}

#[test]
fn the_frame_buffers_name_the_devices_surfaces() {
    let mut g = guest();
    surfaces(&mut g, BLOCK).unwrap();
    assert!(frame_buffers(&mut g, BLOCK).unwrap());
    let w = |at: u32| g.u32(at).unwrap();
    assert_eq!([w(BACK), w(DISPLAY), w(DEPTH)], [BLOCK + BLOCK_BACK, BLOCK + BLOCK_DISPLAY, BLOCK + BLOCK_DEPTH]);
    let (current, previous) = (w(FRAME_TEXTURES[0]), w(FRAME_TEXTURES[1]));
    assert_eq!([w(current), w(current + 4), w(current + 0xc), w(current + 0x10)], [TEXTURE_COMMON, BACK_DATA, LIN_A8R8G8B8, TEXTURE_SIZE]);
    assert_eq!(w(previous + 4), DISPLAY_DATA);
    let alias = w(DEPTH_ALIAS);
    assert_eq!([w(alias), w(alias + 4), w(alias + 0x10)], [SURFACE_COMMON, DEPTH_DATA, TEXTURE_SIZE], "the back buffer's header over the depth");
    assert_eq!([w(w(DEPTH_TEXTURE) + 4), w(w(DEPTH_TEXTURE) + 0xc)], [DEPTH_DATA, LIN_A8R8G8B8]);
    assert_eq!([w(w(DEPTH_SWAPPED) + 4), w(w(DEPTH_SWAPPED) + 0xc)], [DEPTH_DATA, LIN_A8B8G8R8]);
    assert!(w(DEPTH_SWAPPED) + TEXTURE <= BLOCK + BLOCK_LEN);
    frame_buffers_dispose(&mut g).unwrap();
    assert!([BACK, DISPLAY, DEPTH, FRAME_TEXTURES[0], FRAME_TEXTURES[1], DEPTH_TEXTURE, DEPTH_SWAPPED, DEPTH_ALIAS].iter().all(|&a| g.u32(a).unwrap() == 0));
}

#[test]
fn the_game_device_keeps_the_mode_and_clamps_it() {
    let mut g = guest();
    let mode = 0x0043_ed80;
    g.set_u32(mode, 0x0007_0001).unwrap(); // kind 7
    g.set_u32(mode + 4, 9).unwrap(); // refresh index 9
    g.set_u32(mode + 8, 0x55).unwrap();
    g.set_u32(MODE_TABLE + 4 * (MODE_TABLE_WORDS - 1), 0xabcd).unwrap();
    assert!(game_device_create(&mut g, BLOCK, mode).unwrap());
    assert_eq!(g.u16(MODE_KIND).unwrap(), 1, "a kind past 2 becomes 1");
    assert_eq!(g.u32(MODE_INDEX).unwrap(), 3, "an index past 5 becomes 3");
    assert_eq!(g.u32(MODE + 8).unwrap(), 0x55);
    assert_eq!(g.u32(MODE_TABLE_COPY + 4 * (MODE_TABLE_WORDS - 1)).unwrap(), 0xabcd);
    assert_eq!(MODE_WORDS.map(|a| g.u32(a).unwrap()), [u32::MAX; 2]);
    assert_eq!(g.u8(REBUILD).unwrap(), 1);
    assert_eq!(g.u32(GAME_DEVICE).unwrap(), BLOCK, "the game's device pointer is set");
    assert_eq!(g.u32(BLOCK + BLOCK_BACK + 4).unwrap(), BACK_DATA);
}

#[test]
fn a_state_block_stores_each_states_slot() {
    let mut g = guest();
    let (block, table, entries) = (0x0048_0000, 0x0048_1000, 0x0048_2000);
    g.set_u32(block + 0x20, table).unwrap();
    g.set_u32(table + 0x14, 0x0048_3000).unwrap();
    // Two entries from index 1: state 0x10 = 7, state 0x95 (ZBias) = 0.
    let first = 0x0048_3000 + 5;
    g.set_u8(first, 0x10).unwrap();
    g.set_u32(first + 1, 7).unwrap();
    g.set_u8(first + 5, 0x95).unwrap();
    g.set_u32(first + 6, 0).unwrap();
    g.set_u32(STATE_SETTERS + 4 * 0x95, 0x003f_59b0).unwrap();
    g.set_u16(entries, 2 << 9 | 1).unwrap();
    state_block(&mut g, block, entries).unwrap();
    assert_eq!(g.u32(RENDER_STATE + 4 * 0x10).unwrap(), 7);
    assert_eq!(g.u32(device_state::render_state(78)).unwrap(), (-0.0f32).to_bits(), "ZBias 0 offsets by -0");
    assert_eq!(g.u32(device_state::render_state(149)).unwrap(), 0);
}

#[test]
fn a_cube_face_follows_the_last_faces_chain() {
    let mut g = guest();
    let cube = 0x0048_0000;
    // A4R4G4B4 (kind 4, 16 bits), 4 x 4, one level.
    g.set_u32(cube + 0xc, 2 << 24 | 2 << 20 | 1 << 16 | 4 << 8).unwrap();
    g.set_u8(FORMAT_BITS + 4, 0x10).unwrap();
    assert_eq!(face_offset(&g, cube, 0).unwrap(), 0);
    assert_eq!(face_offset(&g, cube, 1).unwrap(), 0x80, "32 bytes, rounded up to 128");
    assert_eq!(face_offset(&g, cube, 5).unwrap(), 0x280);
}

fn header(out: u32) -> u32 {
    0x0048_0000 + (out & 0xff) * 0x20
}

fn data(out: u32) -> u32 {
    0x0060_0000 + (out & 0xff) * 0x400
}

#[test]
fn the_start_up_calls_the_games_functions_in_order() {
    let mut g = guest();
    g.set_u32(0x0046_8788, 0x0046_8790).unwrap();
    g.set_u32(0x0046_8790, 0x11).unwrap();
    g.set_u32(PALETTE_BYTES + 4 * PALETTE_KIND, 4).unwrap();
    g.set_u32(PALETTE_SOURCE, 0x0102_0304).unwrap();
    let f = |cx: Cx| -> EntryFuture { Box::pin(initialize(cx, BLOCK)) };
    let regs = Regs { esp: STACK, ..Default::default() };
    let run = entry::testing::run(&f, g, regs, |c, m| {
        // An allocator writes a header of its own for each out word.
        if let Some(out) = c.ebx {
            m.set_u32(header(out), 0).unwrap();
            m.set_u32(header(out) + 4, data(out)).unwrap();
            m.set_u32(out, header(out)).unwrap();
        }
        Regs { eax: 1, ..Default::default() }
    });
    assert_eq!(run.result, Ok(Ret::new(1, 0)));
    let targets: Vec<u32> = run.calls.iter().map(|c| c.target).filter(|&t| t != points::RASTERIZER_BIT_CLEAR.base).collect();
    let want = [
        &points::DISPLAY_DEFAULTS,
        &points::DISPLAY_WINDOW,
        &points::LAYER_SETUP,
        &points::RASTERIZER_TABLES_RESET,
        &points::TEXTURE_SWITCH_RESET,
        &points::PALETTE_ALLOCATE,
        &points::RASTERIZER_ARRAYS_RESET,
        &points::SMALL_TARGETS_CREATE,
        &points::TARGETS_CREATE,
        &points::TEXTURE_ALLOCATE,
        &points::VOLUME_ALLOCATE,
        &points::CUBE_ALLOCATE,
        &points::RASTERIZER_BUFFER_CREATE,
        &points::RASTERIZER_POOL_CREATE,
        &points::FADE_RECORD_CREATE,
        &points::GEOMETRY_CACHE_CREATE,
        &points::TEXTURE_CACHE_CREATE,
        &points::GAME_STATE_SIZE_CRC,
        &points::RASTERIZER_STATE_CREATE,
    ]
    .map(|p| p.base);
    assert_eq!(targets, want);
    assert_eq!(run.calls.iter().filter(|c| c.target == points::RASTERIZER_BIT_CLEAR.base).count(), 0x1ff);
    for c in &run.calls {
        let p = points::by_base(c.target).expect("a named point");
        assert_eq!(c.shape_error(&p.sig), None, "the call to {}", p.name);
    }
    let g = run.guest;
    // The palette's out word is the body's local below its entry ESP.
    let local = STACK - 4;
    assert_eq!(g.u32(PALETTE).unwrap(), header(local));
    assert_eq!(g.u32(0x8000_0000 | data(local)).unwrap(), 0x0102_0304, "the palette's colours");
    let flat = 0x8000_0000 | data(DEFAULT_TEXTURES[0]);
    assert_eq!([g.u16(flat).unwrap(), g.u16(flat + 2).unwrap()], CHECKER, "the checker texels");
    assert_eq!(g.u32(device_state::render_state(143)).unwrap(), 2, "ZEnable");
    assert_eq!(g.u32(0x0040_75c0).unwrap(), 0x8006);
    assert_eq!(g.u32(device_state::texture_state(3, 0x1c)).unwrap(), 3);
    assert_eq!(g.u32(0x0040_7490).unwrap(), 0, "no dirty word");
    assert_eq!(g.u32(0x0040_864c).unwrap(), 0, "no push-buffer size");
    assert_eq!(g.u32(BACK).unwrap(), BLOCK + BLOCK_BACK);
    assert_eq!(g.u32(0x004b_72b4).unwrap(), 0x11);
    assert_eq!(g.u32(0x004b_72c0).unwrap(), u32::MAX);
    assert_eq!(g.u8(0x0048_5a88).unwrap(), 1);
    assert_ne!(g.u32(GAME_DEVICE).unwrap(), 0, "0x12000 tests it");
}

#[test]
fn the_shutdown_clears_and_calls_in_order() {
    let mut g = guest();
    g.set_u32(BACK, 1).unwrap();
    g.set_u32(DEFAULT_TEXTURES[2], 1).unwrap();
    g.set_u32(SMALL_TARGETS[0], 1).unwrap();
    g.set_u8(0x004b_62a0, 1).unwrap();
    g.set_u32(0x004b_62ac, 0x0048_0000).unwrap();
    g.set_u32(0x0048_0050, 9).unwrap();
    g.set_u32(GAME_DEVICE, BLOCK).unwrap();
    g.set_u32(BOOT_DEVICE, 1).unwrap();
    let f = |cx: Cx| -> EntryFuture { Box::pin(dispose(cx)) };
    let run = entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, |_, _| Regs { eax: 1, ..Default::default() });
    assert_eq!(run.result, Ok(Ret::new(0, 0)));
    let targets: Vec<u32> = run.calls.iter().map(|c| c.target).collect();
    let want = [
        &points::RASTERIZER_BUFFER_DISPOSE,
        &points::RASTERIZER_POOL_DISPOSE,
        &points::CACHE_BLOCK_UNLINK,
        &points::TEXTURE_CACHE_DISPOSE,
        &points::GEOMETRY_CACHE_DISPOSE,
    ]
    .map(|p| p.base);
    assert_eq!(targets, want, "the block's own flag is clear, so nothing is freed");
    let g = run.guest;
    for at in [BACK, DEFAULT_TEXTURES[2], SMALL_TARGETS[0], 0x0048_0050, GAME_DEVICE, BOOT_DEVICE] {
        assert_eq!(g.u32(at).unwrap(), 0, "{at:#x}");
    }
    assert_eq!(g.u8(0x004b_62a0).unwrap(), 0);
}
