//! Invented UI templates and globals for the screen builders' offline tests.

use map::build::{Block, TagPayload, TagSet};
use map::{CacheType, FourCC, TagId};

/// List skins and animation collections the invented `wigl` holds (past
/// every index our layouts and styles use).
const SKINS: usize = 64;
const ANIMATIONS: usize = 40;

/// The retail screens' ids the plan takes templates from, in the fixture.
pub(super) const TEMPLATES: [i16; 4] = [39, 222, 223, 240];

fn add(set: &mut TagSet, group: &[u8; 4], name: &str, block: Block) -> TagId {
    set.add_group(FourCC(*group), None, None);
    set.add_tag(FourCC(*group), name, TagPayload::Built(block))
        .unwrap()
}

/// An invented retail screen with this id: a pane with an animation, a
/// list, two frame bitmaps (at x -300 and 100) and one text block, the
/// strings tag and a header.
fn template_screen(set: &mut TagSet, id: i16, unic: TagId, bitm: TagId) -> TagId {
    let mut root = Block::zeroed(0x68);
    root.put_u16(4, id as u16);
    root.put_u32(0, 0x08);
    root.tag_ref(0x18, unic);
    root.string_id(0x2c, "template_header");
    let mut pane = Block::zeroed(0x4c);
    pane.put_u16(2, 5);
    let mut list = Block::zeroed(0x18);
    list.put_u16(4, 2)
        .put_u16(6, 6)
        .put_u16(8, (-254i16) as u16)
        .put_u16(0xa, 87)
        .put_u16(0xc, 7);
    pane.block(0x0c, 1, list);
    let mut bitmaps = Block::zeroed(0x38 * 2);
    for (i, x) in [-300i16, 100].into_iter().enumerate() {
        bitmaps.put_u16(0x38 * i + 0x0c, x as u16);
        bitmaps.tag_ref(0x38 * i as u32 + 0x18, bitm);
    }
    pane.block(0x24, 2, bitmaps);
    let mut texts = Block::zeroed(0x2c);
    texts.string_id(0x24, "template_text");
    pane.block(0x1c, 1, texts);
    root.block(0x20, 1, pane);
    add(set, b"wgit", &format!("ui\\screens\\retail_{id}"), root)
}

/// A mainmenu shaped like the real one where the UI cares: one `wigl`
/// with skins and animation collections and one `wgtz` listing the templates.
pub fn menu() -> Vec<u8> {
    interface(CacheType::MainMenu, "mainmenu", &TEMPLATES)
}

/// An invented interface with the named map's class and template IDs.
pub fn interface(cache_type: CacheType, name: &str, templates: &[i16]) -> Vec<u8> {
    let scenario = format!(r"ui\{name}");
    let mut set = TagSet::new(cache_type, name, &scenario);
    set.globals = add(&mut set, b"matg", "globals", Block::zeroed(0x284));
    set.scenario = add(&mut set, b"scnr", &scenario, Block::zeroed(0x3e0));
    let unic = add(&mut set, b"unic", "ui\\strings", Block::zeroed(0x20));
    let bitm = add(&mut set, b"bitm", "ui\\frame", Block::zeroed(0x4c));
    let mut skin = Block::zeroed(0x3c);
    skin.tag_ref(4, TagId::NULL);
    skin.block(0x14, 3, Block::zeroed(0x30));
    let skin = add(&mut set, b"skin", "ui\\skin", skin);
    let mut wigl = Block::zeroed(0x1c4);
    for i in 0..16 {
        wigl.tag_ref(0x90 + 8 * i, TagId::NULL);
    }
    for at in [0x110, 0x118, 0x140, 0x148, 0x150, 0x1b8] {
        wigl.tag_ref(at, TagId::NULL);
    }
    wigl.block(0x120, ANIMATIONS as u32, Block::zeroed(0x2c * ANIMATIONS));
    let mut skins = Block::zeroed(8 * SKINS);
    for i in 0..SKINS {
        skins.tag_ref(8 * i as u32, skin);
    }
    wigl.block(0x138, SKINS as u32, skins);
    let wigl = add(&mut set, b"wigl", "ui\\globals", wigl);
    let screens: Vec<TagId> = templates
        .iter()
        .copied()
        .map(|id| template_screen(&mut set, id, unic, bitm))
        .collect();
    let mut list = Block::zeroed(8 * screens.len());
    for (i, s) in screens.iter().enumerate() {
        list.tag_ref(8 * i as u32, *s);
    }
    let mut wgtz = Block::zeroed(0x20);
    wgtz.tag_ref(0, wigl);
    wgtz.block(8, screens.len() as u32, list);
    wgtz.tag_ref(0x10, TagId::NULL);
    wgtz.tag_ref(0x18, unic);
    add(&mut set, b"wgtz", "ui\\wgtz", wgtz.clone());
    if matches!(
        cache_type,
        CacheType::Shared | CacheType::SinglePlayerShared
    ) {
        add(&mut set, b"wgtz", "ui\\wgtz_second", wgtz);
    }
    set.build().unwrap().0
}
