use super::*;
use engine::memory::FlatMemory;

const CACHE: u32 = 0x0050_0000;
const POOL: u32 = 0x0050_0100;
const ENTRIES: u32 = 0x0050_1000;
const TEXTURE_CACHE: u32 = 0x0050_2000;
const TEXTURE_ENTRIES: u32 = 0x0050_3000;
const BLOCK: u32 = 0x0050_4000;
const IMAGE: u32 = 0x0050_5000;

/// Both caches at frame 9 (one resource cache serving both), the texture
/// stamp 20.
fn mem() -> FlatMemory {
    let mut m = FlatMemory::new(0x0040_0000, 0x0012_0000);
    let mut set = |at: u32, v: u32| m.set_u32(at, v).unwrap();
    set(RESOURCES, CACHE);
    set(TEXTURE_RESOURCES, CACHE);
    set(CACHE + 0x64, POOL);
    set(POOL + 0x44, ENTRIES);
    set(CACHE + 0x38, 9);
    set(TEXTURES, TEXTURE_CACHE);
    set(TEXTURE_CACHE + 0x44, TEXTURE_ENTRIES);
    set(TEXTURE_STAMP, 20);
    set(LEVEL_TWO, 2f32.to_bits());
    set(LEVEL_ONE, 1f32.to_bits());
    m
}

/// A loaded block stamps its entry; an empty one is not resident with
/// no write; one to stream is named.
#[test]
fn a_block_is_resident_empty_or_cold() {
    let mut m = mem();
    m.set_u8(BLOCK + 0x1e, 1).unwrap();
    m.set_u32(BLOCK + 0x20, 0xabcd_0002).unwrap();
    assert_eq!(resident(&mut m, BLOCK), Ok(true));
    assert_eq!(m.u32(ENTRIES + 2 * 0x18 + 0x14).unwrap(), 9);
    m.set_u8(BLOCK + 0x1e, 0).unwrap();
    assert_eq!(resident(&mut m, BLOCK), Ok(false), "no size");
    m.set_u32(BLOCK + 8, 0x100).unwrap();
    m.set_u32(BLOCK + 0xc, u32::MAX).unwrap();
    assert_eq!(resident(&mut m, BLOCK), Ok(false), "no offset");
    m.set_u32(BLOCK + 0xc, 0).unwrap();
    assert_eq!(resident(&mut m, BLOCK), Err("texture_cold"));
}

/// A current image is done with no write; a cached one not current takes
/// its level-0 entry's texture and the stamp past the cache's; one the
/// cache does not hold is the request.
#[test]
fn a_stamp_takes_the_cached_texture_or_names_the_request() {
    let mut m = mem();
    m.set_u32(IMAGE + 0x70, 21).unwrap();
    m.set_u32(IMAGE + 0x50, 0x77).unwrap();
    assert_eq!(stamp(&mut m, IMAGE), Ok(()));
    assert_eq!(m.u32(ENTRIES + 0x14).unwrap(), 0, "nothing stamped");

    let mut m = mem();
    m.set_u8(IMAGE + 0xf, 2).unwrap();
    m.set_f32(IMAGE + 0x58, 1.0).unwrap();
    m.set_u32(IMAGE + 0x28, 3).unwrap();
    m.set_u8(TEXTURE_ENTRIES + 3 * 40 + 3, 1).unwrap();
    assert_eq!(stamp(&mut m, IMAGE), Ok(()));
    assert_eq!(m.u32(ENTRIES + 3 * 0x18 + 0x14).unwrap(), 9);
    assert_eq!(m.u32(IMAGE + 0x50).unwrap(), TEXTURE_ENTRIES + 3 * 40 + 0x14);
    assert_eq!(m.u32(IMAGE + 0x70).unwrap(), 22);

    // Not loaded: the entry is stamped, the request named.
    m.set_u8(TEXTURE_ENTRIES + 3 * 40 + 3, 0).unwrap();
    m.set_u32(IMAGE + 0x70, 0).unwrap();
    m.set_u32(IMAGE + 0x50, 0).unwrap();
    assert_eq!(stamp(&mut m, IMAGE), Err("texture_request"));
    // No handle, or a format the gate's check holds back, the same.
    let mut m = mem();
    m.set_u8(IMAGE + 0xf, 2).unwrap();
    m.set_f32(IMAGE + 0x58, 1.0).unwrap();
    m.set_u32(IMAGE + 0x28, u32::MAX).unwrap();
    assert_eq!(stamp(&mut m, IMAGE), Err("texture_request"));
    m.set_u32(IMAGE + 0x28, 3).unwrap();
    m.set_u8(TEXTURE_ENTRIES + 3 * 40 + 3, 1).unwrap();
    m.set_u8(FORMAT_GATE, 1).unwrap();
    m.set_u8(IMAGE + 0x17, 12).unwrap();
    assert_eq!(stamp(&mut m, IMAGE), Ok(()), "format 12 passes the check");
    m.set_u32(IMAGE + 0x70, 0).unwrap();
    m.set_u8(IMAGE + 0x17, 1).unwrap();
    assert_eq!(stamp(&mut m, IMAGE), Err("texture_request"));
}

/// An image the cache does not keep (`+0xf` bit 1 clear) is its texture.
#[test]
fn an_uncached_image_is_its_texture() {
    let mut m = mem();
    m.set_u32(IMAGE + 0x50, 0x55).unwrap();
    assert_eq!(lookup(&mut m, IMAGE), Ok(0x55));
    assert_eq!(stamp(&mut m, IMAGE), Ok(()));
    assert!((1..=11).chain(17..=18).all(format_check));
    assert!(![0, 12, 16, 19].into_iter().any(format_check));
}
