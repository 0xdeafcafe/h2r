use super::*;
use map::CacheType;
use map::build::{Block, RawRef, TagPayload};
use vfs::Rewrite;

/// An invented 8x6 picture in a frame shaped like retail's: a one-pixel
/// opaque border, the bottom-right corner cut on the diagonal (clear past
/// it), the inside at alpha 191. Each inside pixel's colour is its own, so
/// a card that kept any of it would show.
fn framed_template() -> Vec<u8> {
    let (w, h) = (8i32, 6i32);
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            // Distance past the cut: x + y against the corner's.
            let past = x + y - (w - 1 + h - 1 - 2);
            let p = if past > 0 {
                [0, 0, 0, 0]
            } else if past == 0 || x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                [154, 171, 189, 255]
            } else {
                [x as u8, y as u8, 0x5a, 191]
            };
            px.extend_from_slice(&p);
        }
    }
    px
}

/// A map shaped like mainmenu where it matters: [`TEMPLATE`] (8x6 linear
/// `ARGB8888`, [`framed_template`]'s pixels) and one more picture.
fn mainmenu() -> Vec<u8> {
    mainmenu_with(&framed_template())
}

fn mainmenu_with(template: &[u8]) -> Vec<u8> {
    let mut set = TagSet::new(CacheType::MainMenu, "mainmenu", "ui\\mainmenu");
    for g in [b"matg", b"scnr", b"bitm"] {
        set.add_group(FourCC(*g), None, None);
    }
    set.globals = set
        .add_tag(
            FourCC(*b"matg"),
            "globals",
            TagPayload::Built(Block::zeroed(0x284)),
        )
        .unwrap();
    set.scenario = set
        .add_tag(
            FourCC(*b"scnr"),
            "ui\\mainmenu",
            TagPayload::Built(Block::zeroed(0x3e0)),
        )
        .unwrap();
    for (name, fill) in [
        (TEMPLATE, 0x11),
        ("ui\\code_global_bitmaps\\campaign1", 0x22),
    ] {
        let pixels = if name == TEMPLATE {
            rgba_bitmap::argb8888_linear(8, 6, template).unwrap()
        } else {
            vec![fill; 384]
        };
        let size = pixels.len() as u32;
        let raw = set.add_raw(pixels, 0x200);
        add_bitmap(&mut set, name, RawRef::Local(raw), size);
    }
    set.build().unwrap().0
}

fn add_bitmap(set: &mut TagSet, name: &str, raw: RawRef, size: u32) -> TagId {
    let mut image = Block::zeroed(rgba_bitmap::IMAGE_LEN);
    image
        .put_u32(0, FourCC(*b"bitm").as_u32())
        .put_u16(4, 8)
        .put_u16(6, 6)
        .put_u16(0xc, rgba_bitmap::FORMAT_ARGB8888)
        .put_u16(0xe, 0x290)
        .put_u32(0x34, size)
        .raw(0x1c, raw);
    image.bytes[8] = 1;
    let mut header = Block::zeroed(rgba_bitmap::HEADER_LEN);
    header
        .put_u16(0, 4)
        .block(0x3c, 0, Block::zeroed(0))
        .block(0x44, 1, image);
    set.add_tag(FourCC(*b"bitm"), name, TagPayload::Built(header))
        .unwrap()
}

fn card(map: &str, w: u32, h: u32) -> Card {
    Card {
        map: map.into(),
        name: tag_name("halo1", map),
        width: w,
        height: h,
        rgba: (0..w * h)
            .flat_map(|i| [i as u8, 0x40, 0x80, 0xff])
            .collect(),
    }
}

#[test]
fn a_card_covers_the_picture_cropping_the_overflow() {
    // 4x2 onto 2x2: scale 1 by height, the middle two columns.
    let rgba: Vec<u8> = (0..8u8).flat_map(|i| [i * 10, 0, 0, 255]).collect();
    let out = cover(4, 2, &rgba, 2, 2);
    let reds: Vec<u8> = out.chunks(4).map(|p| p[0]).collect();
    assert_eq!(reds, [10, 20, 50, 60]);
    // The same size: unchanged.
    assert_eq!(cover(4, 2, &rgba, 4, 2), rgba);
    // Halo 1's 139x113 onto retail's 220x207: every pixel written.
    let big = cover(139, 113, &vec![200; 139 * 113 * 4], 220, 207);
    assert_eq!(big.len(), 220 * 207 * 4);
    assert!(big.iter().all(|&b| b == 200));
}

#[test]
fn cards_go_into_mainmenu_as_new_bitmaps_and_retail_s_stay() {
    let original = mainmenu();
    let mut before = Map::from_reader(Cursor::new(original.clone())).unwrap();
    let retail: Vec<(TagId, Vec<u8>)> = before
        .tags()
        .to_vec()
        .iter()
        .map(|t| (t.id, before.read_tag_bytes(t.id).unwrap()))
        .collect();
    let added = add_cards(&original, &[card("a10", 3, 2), card("b30", 5, 5)]).unwrap();
    assert!(added.framed);
    let (bytes, table) = (added.map, added.table);
    let mut m = Map::from_reader(Cursor::new(bytes)).unwrap();
    m.validate_checksum().unwrap();
    assert_eq!(m.tags().len(), retail.len() + 2);
    for (id, b) in &retail {
        assert_eq!(&m.read_tag_bytes(*id).unwrap(), b, "{id:?} changed");
    }
    assert_eq!(table.len(), 2);
    for (map, want) in [("a10", card("a10", 3, 2)), ("b30", card("b30", 5, 5))] {
        let id = TagId(table[map]);
        let t = m.tag(id).unwrap().clone();
        assert_eq!(t.name.as_deref(), Some(tag_name("halo1", map).as_str()));
        let template = Template::read(&mut m, id).unwrap();
        assert_eq!(template.size(), (8, 6), "the template's size");
        let ptr = u32::from_le_bytes(template.image[0x1c..0x20].try_into().unwrap());
        let size = u32::from_le_bytes(template.image[0x34..0x38].try_into().unwrap());
        let px = m.read_file_range(u64::from(ptr), u64::from(size)).unwrap();
        let rgba = framed(&cover(want.width, want.height, &want.rgba, 8, 6), &framed_template());
        assert_eq!(px, rgba_bitmap::argb8888_linear(8, 6, &rgba).unwrap());
    }
}

/// Retail's frame is in the picture's pixels: a card takes the template's
/// border and cut corner exactly, and its own colours inside at the
/// template's alpha. None of the template's inside colours survive.
#[test]
fn a_card_takes_the_template_s_frame_pixel_by_pixel() {
    let t = framed_template();
    assert!(has_frame(&t));
    let px: Vec<u8> = (0..48u8).flat_map(|i| [200, i, 100, 255]).collect();
    let out = framed(&px, &t);
    assert_eq!(out.len(), t.len());
    for (i, (o, t)) in out.chunks(4).zip(t.chunks(4)).enumerate() {
        match t[3] {
            255 => assert_eq!(o, t, "border pixel {i}"),
            0 => assert_eq!(o, [0; 4], "corner pixel {i}"),
            a => assert_eq!(o, [200, i as u8, 100, a], "inside pixel {i}"),
        }
    }
    // The corner: the last pixel is clear, the diagonal is border.
    assert_eq!(out[out.len() - 4..], [0; 4]);
    assert_eq!(out[(5 * 8 + 5) * 4 + 3], 255);
    assert_eq!(out[(5 * 8 + 6) * 4 + 3], 0);
    // A plain (opaque) or empty template is no frame: the card goes in as
    // it is, never covered by the template's own picture.
    assert!(!has_frame(&[9u8, 9, 9, 255].repeat(48)));
    assert!(!has_frame(&[0u8; 48 * 4]));
    let plain = mainmenu_with(&[9u8, 9, 9, 255].repeat(48));
    let added = add_cards(&plain, &[card("a10", 8, 6)]).unwrap();
    assert!(!added.framed);
    let mut m = Map::from_reader(Cursor::new(added.map)).unwrap();
    let id = TagId(added.table["a10"]);
    let tpl = Template::read(&mut m, id).unwrap();
    assert_eq!(tpl.rgba(&mut m).unwrap(), card("a10", 8, 6).rgba);
}

#[test]
fn the_rewrite_wants_mainmenu_only_and_fills_the_table() {
    let table = Table::default();
    assert!(CardRewrite::new(Vec::new(), table.clone()).is_none());
    let mut r = CardRewrite::new(vec![card("a10", 2, 2)], table.clone()).unwrap();
    assert!(r.wants("maps\\MAINMENU.map"));
    assert!(!r.wants("maps\\shared.map"));
    assert!(!r.wants("maps\\fonts\\mainmenu.map"));
    // Not a map: served as it is, the table empty.
    assert!(r.rewrite(MAINMENU, b"not a map").is_none());
    assert!(table.lock().unwrap().is_empty());
    let out = r.rewrite(MAINMENU, &mainmenu()).unwrap();
    assert!(Map::from_reader(Cursor::new(out)).is_ok());
    assert_eq!(table.lock().unwrap().keys().collect::<Vec<_>>(), ["a10"]);
}

#[cfg(feature = "gpu")]
#[test]
fn native_bitmap_reads_from_the_rewritten_mainmenu_resource() {
    use crate::native_map::open_from_vfs;
    use crate::native_world_glue::decode_first;

    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    let dir = Temp(std::env::temp_dir().join(format!(
        "native-map-resources-{}",
        std::process::id()
    )));
    let disc = dir.0.join("disc");
    let maps = disc.join("maps");
    std::fs::create_dir_all(&maps).unwrap();

    let menu = mainmenu();
    let mut original = Map::from_reader(Cursor::new(menu.as_slice())).unwrap();
    let template_id = original
        .tags()
        .iter()
        .find(|tag| tag.name.as_deref() == Some(TEMPLATE))
        .unwrap()
        .id;
    let template = Template::read(&mut original, template_id).unwrap();
    let offset = u32::from_le_bytes(template.image[0x1c..0x20].try_into().unwrap());
    let size = u32::from_le_bytes(template.image[0x34..0x38].try_into().unwrap());
    std::fs::write(maps.join("mainmenu.map"), &menu).unwrap();

    let mut level = TagSet::new(CacheType::Campaign, "level", "level");
    level.add_group(FourCC(*b"bitm"), None, None);
    let bitmap = add_bitmap(
        &mut level,
        "menu_bitmap",
        RawRef::External {
            file: map::ResourceFile::MainMenu,
            offset,
        },
        size,
    );
    std::fs::write(maps.join("level.map"), level.build().unwrap().0).unwrap();

    let table = Table::default();
    let picture = card("level", 8, 6);
    let rewrite = CardRewrite::new(vec![picture.clone()], table.clone()).unwrap();
    let config = vfs::Config::new(dir.0.join("data"), 0x4D530064);
    let mut vfs = crate::map_override::mount(config, &disc, Some(rewrite), None, &[]).unwrap();
    assert!(vfs.host_file(r"\Device\CdRom0\maps\mainmenu.map").is_none());
    let vfs = Mutex::new(vfs);

    let (mut level, mut resources) = open_from_vfs(&vfs, "level").unwrap();
    let image = decode_first(&mut level, &mut resources, bitmap).unwrap();
    assert_eq!((image.width, image.height), (8, 6));
    assert_eq!(image.rgba, framed_template());

    let card_id = TagId(table.lock().unwrap()["level"]);
    let (mut menu, mut resources) = open_from_vfs(&vfs, "mainmenu").unwrap();
    let image = decode_first(&mut menu, &mut resources, card_id).unwrap();
    assert_eq!(image.rgba, framed(&picture.rgba, &framed_template()));
}

#[test]
fn cards_are_read_from_each_campaign_s_folder() {
    let dir = std::env::temp_dir().join(format!("campaign-cards-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cards = dir.join("campaigns").join("halo1").join(CARDS_DIR);
    std::fs::create_dir_all(&cards).unwrap();
    let c = card("a10", 3, 2);
    let f = std::fs::File::create(cards.join("a10.png")).unwrap();
    let mut e = png::Encoder::new(f, 3, 2);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&c.rgba).unwrap();
    std::fs::write(cards.join("a30.png"), b"not a png").unwrap();
    let levels: Vec<(String, String)> = ["A10", "a30", "a50"]
        .iter()
        .map(|m| ("halo1".to_owned(), (*m).to_owned()))
        .collect();
    let got = read_cards(&dir, &levels);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(got, [c]);
}

/// The disc's mainmenu with ten cards: every retail tag's bytes as they
/// were, ten new pictures shaped like `campaign0`, the checksum good, and
/// under the rewrite's size limit.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn the_disc_s_mainmenu_takes_ten_cards() {
    let path = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(std::fs::File::open(path).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some("mainmenu.map"))
        .unwrap();
    let original = iso.read(&e).unwrap();
    let mut before = Map::from_reader(Cursor::new(original.clone())).unwrap();
    let cards: Vec<Card> = [
        "a10", "a30", "a50", "b30", "b40", "c10", "c20", "c40", "d20", "d40",
    ]
    .iter()
    .map(|m| card(m, 139, 113))
    .collect();
    let started = std::time::Instant::now();
    let added = add_cards(&original, &cards).unwrap();
    assert!(added.framed, "campaign0 carries the frame");
    let (bytes, table) = (added.map, added.table);
    println!(
        "mainmenu {} -> {} bytes, {} ms",
        original.len(),
        bytes.len(),
        started.elapsed().as_millis()
    );
    assert!(bytes.len() as u64 <= vfs::MAX_REWRITE_BYTES);
    let mut m = Map::from_reader(Cursor::new(bytes)).unwrap();
    m.validate_checksum().unwrap();
    assert_eq!(m.tags().len(), before.tags().len() + 10);
    // Tag memory: index and metadata, from the index base to the
    // metadata's end, grows by at most two pages.
    let memory = |m: &Map<Cursor<Vec<u8>>>| {
        let s = m.metadata_space().unwrap();
        s.virtual_base + s.size - m.index_space().virtual_base
    };
    let (was, now) = (memory(&before), memory(&m));
    println!("tag memory {was:#x} -> {now:#x}");
    assert!(now <= was + 0x2000, "{was:#x} -> {now:#x}");
    for t in before.tags().to_vec() {
        if t.location.is_none() {
            continue;
        }
        assert_eq!(
            m.read_tag_bytes(t.id).unwrap(),
            before.read_tag_bytes(t.id).unwrap(),
            "{:?}",
            t.name
        );
    }
    let template_id = before
        .tags()
        .iter()
        .find(|t| t.name.as_deref() == Some(TEMPLATE))
        .unwrap()
        .id;
    let retail = Template::read(&mut before, template_id).unwrap();
    // Retail's frame (V): every used picture has the same alpha, a border
    // of 834 opaque pixels, 136 clear past the cut corner, the rest 191.
    let frame = retail.rgba(&mut before).unwrap();
    let alpha = |px: &[u8]| -> Vec<u8> { px.chunks(4).map(|p| p[3]).collect() };
    let count = |a: u8| frame.chunks(4).filter(|p| p[3] == a).count();
    assert_eq!((count(255), count(0), count(191)), (834, 136, 220 * 207 - 970));
    assert_eq!(frame[frame.len() - 4..], [0; 4], "the corner is cut");
    for i in 1..15 {
        let id = before
            .tags()
            .iter()
            .find(|t| t.name.as_deref() == Some(&*format!("ui\\code_global_bitmaps\\campaign{i}")))
            .unwrap()
            .id;
        let t = Template::read(&mut before, id).unwrap();
        assert_eq!(alpha(&t.rgba(&mut before).unwrap()), alpha(&frame), "campaign{i}");
    }
    for c in &cards {
        let id = TagId(table[&c.map]);
        let t = Template::read(&mut m, id).unwrap();
        assert_eq!(t.header[..0x3c], retail.header[..0x3c], "{}", c.map);
        assert_eq!(t.size(), (220, 207));
        let ptr = u32::from_le_bytes(t.image[0x1c..0x20].try_into().unwrap());
        let size = u32::from_le_bytes(t.image[0x34..0x38].try_into().unwrap());
        assert_eq!(size, 0x2d480);
        let px = m.read_file_range(u64::from(ptr), u64::from(size)).unwrap();
        let rgba = framed(&cover(139, 113, &c.rgba, 220, 207), &frame);
        assert_eq!(
            px,
            rgba_bitmap::argb8888_linear(220, 207, &rgba).unwrap(),
            "{}",
            c.map
        );
    }
}
