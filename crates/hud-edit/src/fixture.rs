//! An invented map for tests: one HUD bitmap, a player HUD with three
//! bitmap widgets and a number, a weapon HUD, and the biped and weapon
//! that name them. Built with `map::build`; nothing comes from a disc.

use h1map::bitmap::{BITM, Bitmap, IMAGE_LEN, Image, Sequence, Sprite};
use map::build::{Block, TagPayload, TagSet};
use map::{CacheType, FourCC, TagId};

use crate::layout::{self, bitmap as bw, text as tw};

/// The panel bitmap: 64 by 16, opaque, its blue rising left to right.
pub const PANEL: (u16, u16) = (64, 16);

/// The fixture's widgets' names, in draw order.
pub const BITMAP_WIDGETS: [&str; 3] = ["panel", "meter", "stray"];
pub const TEXT_WIDGETS: [&str; 1] = ["count"];

fn panel() -> (Bitmap, Vec<u8>) {
    let mut e = [0u8; IMAGE_LEN];
    e[0..4].copy_from_slice(&BITM.0.to_le_bytes());
    // Width, height, depth, 2D, A8R8G8B8 (11), linear and power of two.
    for (at, v) in [(4, PANEL.0), (6, PANEL.1), (8, 1), (0xa, 0), (0xc, 11), (0xe, 0x11)] {
        e[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    let mut img = Image::parse(&e).unwrap();
    let len = h1bitmaps::decode::layout_len(&img).unwrap();
    img.pixel_size = len.next_multiple_of(h1bitmaps::convert::LOD_ALIGN) as u32;
    let mut px = vec![0u8; img.pixel_size as usize];
    for y in 0..PANEL.1 as usize {
        for x in 0..PANEL.0 as usize {
            // B, G, R, A.
            let i = (y * PANEL.0 as usize + x) * 4;
            px[i..i + 4].copy_from_slice(&[(x * 4) as u8, 160, 60, 255]);
        }
    }
    let bm = Bitmap {
        kind: 4,
        encoding: 0,
        usage: 0,
        flags: 0,
        detail_fade: 0.0,
        sharpen: 0.0,
        bump_height: 0.0,
        sprite_budget: [0, 0],
        color_plate: [PANEL.0, PANEL.1],
        blur: 0.0,
        alpha_bias: 0.0,
        mipmap_count: 0,
        sprite_usage: 0,
        sprite_spacing: 0,
        sequences: vec![
            Sequence {
                name: "whole".into(),
                first_image: 0,
                image_count: 1,
                sprites: Vec::new(),
            },
            Sequence {
                name: "left half".into(),
                first_image: 0,
                image_count: 1,
                sprites: vec![Sprite {
                    image: 0,
                    bounds: [0.0, 0.5, 0.0, 1.0],
                    registration: [0.0, 0.0],
                }],
            },
        ],
        images: vec![img],
    };
    (bm, px)
}

/// A bitmap widget: name, anchor, fullscreen offset (the split-screen
/// ones half of it), sequence, input 0, registration.
#[allow(clippy::too_many_arguments)]
fn bitmap_widget(
    name: &str,
    anchor: u16,
    offset: (i16, i16),
    seq: i8,
    input: u8,
    reg: [f32; 2],
    bitmap: TagId,
    shader: TagId,
    state: [u8; 20],
) -> Block {
    let mut b = Block::zeroed(layout::BITMAP_SIZE);
    b.bytes[bw::INPUTS] = input;
    b.bytes[bw::STATE..bw::STATE_END].copy_from_slice(&state);
    b.put_u16(bw::ANCHOR, anchor);
    b.bytes[bw::SEQUENCES..bw::SEQUENCES + 3].fill(seq as u8);
    for (m, at) in bw::OFFSETS.iter().enumerate() {
        let d = if m == 0 { 1 } else { 2 };
        b.put_u16(*at, (offset.0 / d) as u16);
        b.put_u16(at + 2, (offset.1 / d) as u16);
    }
    for at in bw::REGISTRATIONS {
        b.put_f32(at, reg[0]);
        b.put_f32(at + 4, reg[1]);
    }
    b.string_id(bw::NAME as u32, name);
    b.tag_ref(bw::BITMAP as u32, bitmap);
    b.tag_ref(bw::SHADER as u32, shader);
    b
}

/// The frag grenade widgets' state flags (retail, V; docs/halo1/hud.md).
pub const FRAG_STATE: [u8; 20] = [0x04, 0, 0, 0, 0, 0, 0, 0, 0x2a, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
/// Shown on word A `0x1000` (the retail shield meter's show word).
pub const ALWAYS: [u8; 20] = [0x00, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

fn nhdt(bitmaps: Vec<Block>, texts: Vec<Block>) -> Block {
    let mut root = Block::zeroed(layout::ROOT_SIZE);
    for (at, list, size) in [
        (layout::ROOT_BITMAPS, bitmaps, layout::BITMAP_SIZE),
        (layout::ROOT_TEXTS, texts, layout::TEXT_SIZE),
    ] {
        if list.is_empty() {
            continue;
        }
        let n = list.len() as u32;
        let mut block = Block::zeroed(0);
        for w in list {
            let base = block.bytes.len() as u32;
            assert_eq!(w.bytes.len(), size);
            block.bytes.extend_from_slice(&w.bytes);
            for mut f in w.fixups {
                f.offset += base;
                block.fixups.push(f);
            }
        }
        root.block(at as u32, n, block);
    }
    root
}

/// The map's bytes and the datums: (map, player HUD, weapon HUD, bitmap).
pub fn build() -> (Vec<u8>, TagId, TagId, TagId) {
    let mut set = TagSet::new(CacheType::Multiplayer, "hudtest", "invented\\hudtest");
    for g in [b"bitm", b"shad", b"nhdt", b"weap", b"bipd"] {
        set.add_group(FourCC(*g), None, None);
    }
    let (bm, px) = panel();
    let (out, _) = h1bitmaps::convert::convert_with(&bm, |_| Ok(px.clone())).unwrap();
    let bitmap = out.install(&mut set, "invented\\hud\\panel").unwrap();
    let shader = |set: &mut TagSet, name: &str| {
        set.add_tag(FourCC(*b"shad"), name, TagPayload::Built(Block::zeroed(0x10)))
            .unwrap()
    };
    let meter = shader(&mut set, "invented\\hud\\shaders\\ammo_meter");
    let plain = shader(&mut set, "invented\\hud\\shaders\\plain");
    let text_flash = shader(&mut set, "invented\\hud\\shaders\\text_flash");
    // Bottom left: the panel (48..112, 404..420), the meter over its left
    // half one row up, and a stray copy that half covers the panel.
    let player = nhdt(
        vec![
            bitmap_widget("panel", 2, (0, -40), 0, 0, [0.0, 0.0], bitmap, plain, ALWAYS),
            bitmap_widget("meter", 2, (0, -56), 1, 17, [0.0, 0.0], bitmap, meter, ALWAYS),
            bitmap_widget("stray", 2, (40, -48), 0, 0, [0.0, 0.0], bitmap, plain, ALWAYS),
        ],
        vec![{
            // Top left at (20, 10), ending there: the frag count.
            let mut t = Block::zeroed(layout::TEXT_SIZE);
            t.bytes[tw::INPUTS] = 20;
            t.bytes[tw::STATE..tw::STATE_END].copy_from_slice(&FRAG_STATE);
            t.put_u16(tw::ANCHOR, 1).put_u16(tw::FLAGS, 1).put_u16(tw::JUSTIFICATION, 2);
            for at in tw::OFFSETS {
                t.put_u16(at, 20).put_u16(at + 2, 10);
            }
            t.string_id(tw::NAME as u32, "count");
            t.string_id(tw::STRING as u32, "hud_input1");
            t.tag_ref(tw::SHADER as u32, text_flash);
            t
        }],
    );
    let player = set
        .add_tag(FourCC(*b"nhdt"), "invented\\hud\\player", TagPayload::Built(player))
        .unwrap();
    let weapon = nhdt(
        vec![bitmap_widget("ammo", 0, (-64, 0), 0, 0x36, [0.0, 0.0], bitmap, meter, ALWAYS)],
        Vec::new(),
    );
    let weapon_hud = set
        .add_tag(FourCC(*b"nhdt"), "invented\\hud\\blaster", TagPayload::Built(weapon))
        .unwrap();
    let mut weap = Block::zeroed(0x300);
    weap.tag_ref(crate::model::WEAPON_HUD as u32, weapon_hud);
    set.add_tag(FourCC(*b"weap"), "invented\\weapons\\blaster", TagPayload::Built(weap))
        .unwrap();
    let mut bipd = Block::zeroed(0x1b0);
    let mut iface = Block::zeroed(8);
    iface.tag_ref(0, player);
    bipd.block(crate::model::UNIT_HUD_INTERFACES as u32, 1, iface);
    set.add_tag(FourCC(*b"bipd"), "invented\\bipeds\\chief", TagPayload::Built(bipd))
        .unwrap();
    let (bytes, _) = set.build().unwrap();
    (bytes, player, weapon_hud, bitmap)
}
