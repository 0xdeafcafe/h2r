use super::*;
use map::CacheType;
use map::build::{Block, RawRef, TagPayload};
use vfs::Rewrite;

/// A map shaped like mainmenu where it matters: [`TEMPLATE`] (8x6 linear
/// `ARGB8888`) and one more picture.
fn mainmenu() -> Vec<u8> {
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
        let raw = set.add_raw(vec![fill; 256], 0x200);
        let mut image = Block::zeroed(rgba_bitmap::IMAGE_LEN);
        image
            .put_u32(0, FourCC(*b"bitm").as_u32())
            .put_u16(4, 8)
            .put_u16(6, 6)
            .put_u16(0xc, rgba_bitmap::FORMAT_ARGB8888)
            .put_u16(0xe, 0x290)
            .put_u32(0x34, 256)
            .raw(0x1c, RawRef::Local(raw));
        image.bytes[8] = 1;
        let mut header = Block::zeroed(rgba_bitmap::HEADER_LEN);
        header
            .put_u16(0, 4)
            .block(0x3c, 0, Block::zeroed(0))
            .block(0x44, 1, image);
        set.add_tag(FourCC(*b"bitm"), name, TagPayload::Built(header))
            .unwrap();
    }
    set.build().unwrap().0
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
    let (bytes, table) = add_cards(&original, &[card("a10", 3, 2), card("b30", 5, 5)]).unwrap();
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
        let rgba = cover(want.width, want.height, &want.rgba, 8, 6);
        assert_eq!(px, rgba_bitmap::argb8888_linear(8, 6, &rgba).unwrap());
    }
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
    let (bytes, table) = add_cards(&original, &cards).unwrap();
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
    for c in &cards {
        let id = TagId(table[&c.map]);
        let t = Template::read(&mut m, id).unwrap();
        assert_eq!(t.header[..0x3c], retail.header[..0x3c], "{}", c.map);
        assert_eq!(t.size(), (220, 207));
        let ptr = u32::from_le_bytes(t.image[0x1c..0x20].try_into().unwrap());
        let size = u32::from_le_bytes(t.image[0x34..0x38].try_into().unwrap());
        assert_eq!(size, 0x2d480);
        let px = m.read_file_range(u64::from(ptr), u64::from(size)).unwrap();
        let rgba = cover(139, 113, &c.rgba, 220, 207);
        assert_eq!(
            px,
            rgba_bitmap::argb8888_linear(220, 207, &rgba).unwrap(),
            "{}",
            c.map
        );
    }
}
