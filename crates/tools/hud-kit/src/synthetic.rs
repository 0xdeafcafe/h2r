//! The synthetic source's invented map, built with `map::build`: a player
//! `nhdt` whose motion tracker is a flat disc and whose shield meter,
//! grenade pills and counts are flat boxes on the retail rectangles ([`crate::rects`]), the strip
//! meter's bitmap and shader under the retail names, a `hudg` with a
//! waypoint arrow and marker, the biped naming the HUD, and a scoreboard
//! `nhdt` with Assault's arming bar (`bomb_defusal`, at its retail place)
//! for the arming bar's `Art::Widget`. Nothing comes from a disc; `cargo
//! test` draws the sheet from it.

use anyhow::{Result, anyhow};
use h1map::bitmap::{BITM, Bitmap, IMAGE_LEN, Image, Sequence, Sprite};
use hud_edit::layout::{self, bitmap as bw, text as tw};
use map::build::{Block, TagPayload, TagSet};
use map::{CacheType, FourCC, TagId};
use tags::defs::{self, Val};

use crate::hudg;
use crate::templates::{STRIP_BITMAP, STRIP_SHADER};

pub const SHAD: FourCC = FourCC(*b"shad");
/// The invented player HUD's name.
pub const PLAYER: &str = "synthetic\\hud\\player";
/// The scoreboard `nhdt`, under the retail name the arming bar's art
/// names, and its bar's bitmap.
pub const SCOREBOARD: &str = "ui\\hud\\scoreboard";
pub const SCOREBOARD_BAR_BITMAP: &str = "ui\\hud\\bitmaps\\new_hud\\bomb_meter";

/// Shown on word A `0x1000` (the retail shield meter's show word).
const ALWAYS: [u8; 20] = [
    0x00, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
/// The frag grenade widgets' state flags (retail, V; docs/halo1/hud.md).
const FRAG_STATE: [u8; 20] = [
    0x04, 0, 0, 0, 0, 0, 0, 0, 0x2a, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
/// The plasma grenade widgets' (retail, V; `h1objects::h1hud::GRENADE_STATES`).
const PLASMA_STATE: [u8; 20] = [
    0x08, 0, 0, 0, 0, 0, 0, 0, 0x26, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
/// The invented grenade row, top left, as retail's: one panel of two
/// pills that the frag and the plasma widgets both draw at [`PANEL_AT`],
/// the plasma pill its first [`PILL_WIDTH`] pixels and the frag pill its
/// last, and the counts centred at x 65 (frag) and 12 (plasma), the
/// retail counts' offsets (V: `ui\hud\masterchief`).
pub const PANEL_AT: (i16, i16) = (0, 0);
pub const PANEL_SIZE: (u16, u16) = (103, 19);
pub const PILL_WIDTH: u16 = 50;
pub const COUNTS: [(i16, i16); 2] = [(65, -1), (12, -1)];
/// The frag pill's texels: its red is its x in the pill, so a crop's
/// texels say where they came from.
pub fn frag_texel(x: u32) -> [u8; 4] {
    [x as u8 * 4, 120, 200, 200]
}

/// A flat bitmap: a `size` power-of-two image whose sequence 0 is one
/// sprite of `sprite` pixels at its top left, each texel `texel(x, y)`
/// (RGBA).
fn bitmap(
    set: &mut TagSet,
    name: &str,
    size: (u16, u16),
    sprite: (u16, u16),
    texel: impl Fn(u32, u32) -> [u8; 4],
) -> Result<TagId> {
    let mut e = [0u8; IMAGE_LEN];
    e[0..4].copy_from_slice(&BITM.0.to_le_bytes());
    // Width, height, depth, 2D, A8R8G8B8 (11), linear and power of two.
    for (at, v) in [
        (4, size.0),
        (6, size.1),
        (8, 1),
        (0xa, 0),
        (0xc, 11),
        (0xe, 0x11),
    ] {
        e[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    let mut img = Image::parse(&e).map_err(|e| anyhow!("{name}: {e}"))?;
    let len = h1_assets::bitmaps::decode::layout_len(&img).map_err(|e| anyhow!("{name}: {e}"))?;
    img.pixel_size = len.next_multiple_of(h1_assets::bitmaps::convert::LOD_ALIGN) as u32;
    let mut px = vec![0u8; img.pixel_size as usize];
    for y in 0..u32::from(sprite.1) {
        for x in 0..u32::from(sprite.0) {
            let [r, g, b, a] = texel(x, y);
            let i = ((y * u32::from(size.0) + x) * 4) as usize;
            // B, G, R, A.
            px[i..i + 4].copy_from_slice(&[b, g, r, a]);
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
        color_plate: [size.0, size.1],
        blur: 0.0,
        alpha_bias: 0.0,
        mipmap_count: 0,
        sprite_usage: 0,
        sprite_spacing: 0,
        sequences: vec![Sequence {
            name: "sprite".into(),
            first_image: 0,
            image_count: 1,
            sprites: vec![Sprite {
                image: 0,
                bounds: [
                    0.0,
                    f32::from(sprite.0) / f32::from(size.0),
                    0.0,
                    f32::from(sprite.1) / f32::from(size.1),
                ],
                registration: [0.0, 0.0],
            }],
        }],
        images: vec![img],
    };
    let (out, _) = h1_assets::bitmaps::convert::convert_with(&bm, |_| Ok(px.clone()))
        .map_err(|e| anyhow!("{name}: {e}"))?;
    out.install(set, name).map_err(|e| anyhow!("{name}: {e}"))
}

fn shader(set: &mut TagSet, name: &str) -> Result<TagId> {
    Ok(set.add_tag(SHAD, name, TagPayload::Built(Block::zeroed(0x10)))?)
}

/// A bitmap widget of the invented player HUD shown on word A `0x1000`:
/// anchor 2 (bottom left), registered at its top left.
fn bitmap_widget(
    name: &str,
    offset: (i16, i16),
    bitmap: TagId,
    shader: TagId,
    special: u16,
) -> Block {
    let mut b = gated_widget(name, 2, ALWAYS, offset, bitmap, shader, special);
    // Split-screen offsets half the fullscreen.
    for at in &bw::OFFSETS[1..] {
        b.put_u16(*at, (offset.0 / 2) as u16);
        b.put_u16(at + 2, (offset.1 / 2) as u16);
    }
    b
}

/// A bitmap widget at `anchor`, gated on `state`, at `offset` in every
/// screen mode.
fn gated_widget(
    name: &str,
    anchor: u16,
    state: [u8; 20],
    offset: (i16, i16),
    bitmap: TagId,
    shader: TagId,
    special: u16,
) -> Block {
    let mut b = Block::zeroed(layout::BITMAP_SIZE);
    b.bytes[bw::STATE..bw::STATE_END].copy_from_slice(&state);
    b.put_u16(bw::ANCHOR, anchor).put_u16(bw::SPECIAL, special);
    for at in bw::OFFSETS {
        b.put_u16(at, offset.0 as u16);
        b.put_u16(at + 2, offset.1 as u16);
    }
    b.string_id(bw::NAME as u32, name);
    b.tag_ref(bw::BITMAP as u32, bitmap);
    b.tag_ref(bw::SHADER as u32, shader);
    b
}

fn nhdt(bitmaps: Vec<Block>, texts: Vec<Block>) -> Block {
    let mut root = Block::zeroed(layout::ROOT_SIZE);
    for (at, list) in [(layout::ROOT_BITMAPS, bitmaps), (layout::ROOT_TEXTS, texts)] {
        if list.is_empty() {
            continue;
        }
        let n = list.len() as u32;
        let mut block = Block::zeroed(0);
        for w in list {
            let base = block.bytes.len() as u32;
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

/// The invented map's bytes.
pub fn build() -> Result<Vec<u8>> {
    let mut set = TagSet::new(CacheType::Multiplayer, "hudkit", "synthetic\\hudkit");
    for g in [b"bitm", b"shad", b"nhdt", b"bipd", b"hudg"] {
        set.add_group(FourCC(*g), None, None);
    }
    // The tracker: a grey disc, 83 across, clear outside its rim as the
    // retail one is round (the charge segments' arcs pass its corners).
    let tracker = bitmap(
        &mut set,
        "synthetic\\hud\\tracker",
        (128, 128),
        (83, 83),
        |x, y| {
            let (dx, dy) = (x as f32 + 0.5 - 41.5, y as f32 + 0.5 - 41.5);
            if dx.hypot(dy) <= 41.5 {
                [70, 110, 90, 220]
            } else {
                [0; 4]
            }
        },
    )?;
    // The shield meter: what shows of it is its top-left 83 by 25.
    let shield = bitmap(
        &mut set,
        "synthetic\\hud\\shield_meter",
        (128, 32),
        (128, 32),
        |x, y| {
            if x < 83 && y < 25 {
                [90, 160, 230, 230]
            } else {
                [0, 0, 0, 0]
            }
        },
    )?;
    // The strip: 92 by 8, its blue rising from 0 at column 13 to 1 at the
    // right, as docs/halo1/health.md has the retail one.
    bitmap(&mut set, STRIP_BITMAP, (128, 8), (92, 8), |x, _| {
        let blue = (x.saturating_sub(13) * 255 / 78).min(255) as u8;
        [80, 190, blue, 255]
    })?;
    // The arrow: 16 by 16, a triangle pointing down, as the navpoints'.
    let arrow = bitmap(
        &mut set,
        "synthetic\\hud\\waypoint_arrow",
        (16, 16),
        (16, 16),
        |x, y| {
            let half = (16 - y) / 2;
            if x + half >= 8 && x < 8 + half {
                [240, 220, 90, 255]
            } else {
                [0, 0, 0, 0]
            }
        },
    )?;
    // The grenade panel: the plasma pill a flat box, a gap, the frag pill.
    let frag_from = u32::from(PANEL_SIZE.0 - PILL_WIDTH);
    let pill = bitmap(
        &mut set,
        "synthetic\\hud\\grenade_panel",
        (128, 32),
        PANEL_SIZE,
        |x, _| {
            if x < u32::from(PILL_WIDTH) {
                [60, 120, 200, 200]
            } else if x >= frag_from {
                frag_texel(x - frag_from)
            } else {
                [0, 0, 0, 0]
            }
        },
    )?;
    // The waypoint marker: a 12 by 12 diamond.
    let marker = bitmap(
        &mut set,
        "synthetic\\hud\\waypoint_marker",
        (16, 16),
        (12, 12),
        |x, y| {
            let d = (x as i32 - 6).abs() + (y as i32 - 6).abs();
            if d <= 6 {
                [230, 230, 230, 255]
            } else {
                [0, 0, 0, 0]
            }
        },
    )?;
    let plain = shader(&mut set, "synthetic\\hud\\shaders\\plain")?;
    let meter = shader(&mut set, STRIP_SHADER)?;
    let arrow_shader = shader(&mut set, "synthetic\\hud\\shaders\\waypoint_arrow")?;
    let text_shader = shader(&mut set, "synthetic\\hud\\shaders\\text")?;
    let player = nhdt(
        vec![
            bitmap_widget("motion_sensor", (-1, -84), tracker, plain, 0),
            bitmap_widget(
                "shield_meter",
                (-1, -98),
                shield,
                meter,
                layout::SPECIAL_SHIELD,
            ),
            gated_widget("frag_pill", 1, FRAG_STATE, PANEL_AT, pill, plain, 0),
            gated_widget("plasma_pill", 1, PLASMA_STATE, PANEL_AT, pill, plain, 0),
        ],
        [
            (
                "frag_count",
                crate::templates::FRAG_COUNT_INPUT,
                FRAG_STATE,
                COUNTS[0],
            ),
            (
                "plasma_count",
                crate::templates::PLASMA_COUNT_INPUT,
                PLASMA_STATE,
                COUNTS[1],
            ),
        ]
        .into_iter()
        .map(|(name, input, state, (x, y))| {
            // Top left, centred: a grenade count.
            let mut t = Block::zeroed(layout::TEXT_SIZE);
            t.bytes[tw::INPUTS] = input;
            t.bytes[tw::STATE..tw::STATE_END].copy_from_slice(&state);
            t.put_u16(tw::ANCHOR, 1)
                .put_u16(tw::FLAGS, 1)
                .put_u16(tw::JUSTIFICATION, 1);
            for at in tw::OFFSETS {
                t.put_u16(at, x as u16).put_u16(at + 2, y as u16);
            }
            t.string_id(tw::NAME as u32, name);
            t.string_id(tw::STRING as u32, "hud_input1");
            t.tag_ref(tw::SHADER as u32, text_shader);
            t
        })
        .collect(),
    );
    let player = set.add_tag(hud_edit::model::NHDT, PLAYER, TagPayload::Built(player))?;
    // The scoreboard's arming bar: a 140 by 13 capsule whose blue rises
    // left to right, centre (0, 110), (0, 27) in split screen (retail's), registered
    // at its centre, as retail's (docs/game/hud-kit.md, "The arming bar").
    let bomb = bitmap(
        &mut set,
        SCOREBOARD_BAR_BITMAP,
        (256, 16),
        (140, 13),
        |x, _| [200, 90, (4 + x * 249 / 139) as u8, 255],
    )?;
    let bomb_shader = shader(&mut set, "ui\\hud\\shaders\\bomb_defusal_meter")?;
    let mut bar = gated_widget("bomb_defusal", 4, [0; 20], (0, 110), bomb, bomb_shader, 0);
    for at in &bw::OFFSETS[1..] {
        bar.put_u16(at + 2, 27);
    }
    for at in bw::REGISTRATIONS {
        bar.put_f32(at, 0.5).put_f32(at + 4, 0.5);
    }
    set.add_tag(
        hud_edit::model::NHDT,
        SCOREBOARD,
        TagPayload::Built(nhdt(vec![bar], Vec::new())),
    )?;
    let mut bipd = Block::zeroed(0x1b0);
    let mut iface = Block::zeroed(8);
    iface.tag_ref(0, player);
    bipd.block(hud_edit::model::UNIT_HUD_INTERFACES as u32, 1, iface);
    set.add_tag(
        FourCC(*b"bipd"),
        "synthetic\\bipeds\\player",
        TagPayload::Built(bipd),
    )?;
    // The hudg: arrow_assets[0], through the definition.
    let def = hudg::def();
    let child = def.child("arrow_assets").expect("hudg arrow_assets");
    let mut row = defs::default_record(child);
    let put = |r: &mut defs::Record, name: &str, v: Val| {
        r.set(name, v).map_err(|e| anyhow!("hudg {name}: {e}"))
    };
    put(&mut row, "arrow_image", Val::tag(FourCC(*b"bitm"), arrow))?;
    put(&mut row, "arrow_material", Val::tag(SHAD, arrow_shader))?;
    put(&mut row, "sprite_sequence", Val::Int(0))?;
    let mchild = def
        .child("waypoint_markers")
        .expect("hudg waypoint_markers");
    let mut mrow = defs::default_record(mchild);
    put(
        &mut mrow,
        "marker_image",
        Val::tag(FourCC(*b"bitm"), marker),
    )?;
    put(&mut mrow, "marker_material", Val::tag(SHAD, arrow_shader))?;
    for seq in ["visible_sequence", "obscured_sequence", "outside_sequence"] {
        put(&mut mrow, seq, Val::Int(0))?;
    }
    let mut root = defs::default_record(def);
    // Rows 0 and 1 (the navpoints' arrow, hudg::NAVPOINT_ARROW) alike.
    put(
        &mut root,
        "arrow_assets",
        Val::Block(vec![row.clone(), row]),
    )?;
    put(&mut root, "waypoint_markers", Val::Block(vec![mrow]))?;
    let hudg_block = defs::to_built(def, &root, &mut |_| None).map_err(|e| anyhow!("hudg: {e}"))?;
    set.add_tag(
        hudg::HUDG,
        "synthetic\\globals\\hud_globals",
        TagPayload::Built(hudg_block),
    )?;
    let (bytes, _) = set.build()?;
    Ok(bytes)
}
