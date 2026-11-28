//! An invented multiplayer map the pieces go into (feature `fixture`, and
//! this crate's tests): no game data, only what [`crate::blockout::Facts`] reads, the
//! `tex_bump` template and its default bitmaps and a globals naming two
//! materials, and one shader of the "level" for `Map` materials.

use map::build::{Block, TagPayload, TagSet};
use map::{CacheType, FourCC};

use crate::blockout::texture;

/// The invented level shader a `Map` material can name.
pub const ROCK_SHADER: &str = r"invented\shaders\rock";

/// The global materials the fixture's globals name, in order.
pub const MATERIALS: [&str; 2] = ["soft_organic_flesh", h1objects::rebuild::PIECE_MATERIAL.0];

/// The default bitmaps `tex_bump` needs.
const DEFAULT_BITMAPS: [&str; 4] = [
    r"shaders\default_bitmaps\bitmaps\default_vector",
    r"shaders\default_bitmaps\bitmaps\default_detail",
    r"shaders\default_bitmaps\bitmaps\color_white",
    r"rasterizer\linear_corner_fade",
];

/// The fixture map's bytes.
pub fn fixture() -> Vec<u8> {
    let mut set = TagSet::new(CacheType::Multiplayer, "yard", r"invented\yard");
    for group in [b"scnr", b"matg", b"stem", b"bitm", b"shad"] {
        set.add_group(FourCC(*group), None, None);
    }
    let globals = {
        let mut names = Block::zeroed(MATERIALS.len() * 0xb4);
        for (i, m) in MATERIALS.iter().enumerate() {
            names.string_id((i * 0xb4) as u32, m);
        }
        let mut root = Block::zeroed(0x284);
        root.block(0x150, MATERIALS.len() as u32, names);
        root
    };
    set.globals = set
        .add_tag(FourCC(*b"matg"), r"globals\globals", TagPayload::Built(globals))
        .expect("globals");
    set.scenario = set
        .add_tag(
            FourCC(*b"scnr"),
            r"invented\yard",
            TagPayload::Built(Block::zeroed(0x3e0)),
        )
        .expect("scenario");
    set.add_tag(
        FourCC(*b"stem"),
        h1convert::shad::TEX_BUMP.stem,
        TagPayload::Built(Block::zeroed(0x60)),
    )
    .expect("template");
    let grey = dev_texture::Image {
        width: 4,
        height: 4,
        rgba: vec![128; 64],
    };
    let encoded = texture::encode(&grey).expect("a 4x4 grey");
    for path in DEFAULT_BITMAPS {
        texture::bitmap(&encoded)
            .expect("a default bitmap")
            .install(&mut set, path)
            .expect("installs");
    }
    set.add_tag(
        FourCC(*b"shad"),
        ROCK_SHADER,
        TagPayload::Built(Block::zeroed(0x54)),
    )
    .expect("the rock shader");
    set.build().expect("the fixture builds").0
}
