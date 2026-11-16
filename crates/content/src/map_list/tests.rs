use super::*;
use crate::{Sha256, manifest::Layer};

fn derived(name: &str, map: &str) -> Manifest {
    let mut m = Manifest::retail(map, Sha256([1; 32]));
    m.package.name = name.into();
    m.base.sha256 = None;
    m.base.derived = true;
    m.patches = vec![Layer {
        file: format!("{map}.map"),
        sha256: Sha256([2; 32]),
    }];
    m
}

fn base_entry() -> BaseEntry {
    BaseEntry {
        map_id: 7,
        map: "alpha".into(),
        sort: 30,
        flags: 0x5,
        teams: [3; 16],
    }
}

pub(crate) fn png_bytes(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, w, h);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    let mut wr = e.write_header().unwrap();
    let px: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .flat_map(|(x, y)| f(x, y))
        .collect();
    wr.write_image_data(&px).unwrap();
    drop(wr);
    out
}

#[test]
fn a_record_from_a_manifest_passes_1_0s_check() {
    let mut m = derived("Coldsnap", "coldsnap");
    m.package.description = Some("Ice everywhere".into());
    let e = Entry::for_manifest(&m, 0x1234_5678, None, generated_card(&m.map_id()));
    let r = e.to_v2().unwrap();
    assert_eq!(r.len(), V2_LEN);
    patch_lvl::validate_v2(&r).unwrap();
    assert_eq!(u32_at(&r, VERSION), 2);
    assert_eq!(u32_at(&r, KIND), 1);
    let mut zeroed = r.clone();
    zeroed[CRC..CRC + 4].fill(0);
    assert_eq!(u32_at(&r, CRC), patch_lvl::crc32(&zeroed));
    assert_eq!(patch_lvl::map_name(&r).unwrap(), "coldsnap");
    assert_eq!(patch_lvl::v2_map_id(&r), 0x1234_5678);
    assert_eq!(u32_at(&r, UNUSED), u32::MAX);
    assert_eq!(Entry::from_v2(&r).unwrap(), e, "round trip");
}

#[test]
fn fields_come_from_the_manifest_and_the_base() {
    let mut m = derived("Coldsnap", "coldsnap");
    let e = Entry::for_manifest(&m, LIST_ID_FIRST, None, vec![0; PREVIEW_LEN]);
    assert_eq!(e.scenario, "scenarios\\multi\\coldsnap\\coldsnap");
    assert!(e.names.iter().all(|n| n == "Coldsnap"), "every language");
    assert!(e.descriptions.iter().all(String::is_empty));
    assert_eq!(
        (e.sort, e.flags, e.teams),
        (DEFAULT_SORT, DEFAULT_FLAGS, DEFAULT_TEAMS)
    );

    m.package.description = Some("Cold".into());
    m.list = Some(ListInfo {
        base: Some("alpha".into()),
        sort: Some(-5),
        name: [("ja".to_string(), "コールド".to_string())].into(),
        description: [("pt".to_string(), "Frio".to_string())].into(),
    });
    m.validate().unwrap();
    let id = m.map_id();
    m.list = None;
    assert_eq!(m.map_id(), id, "[list] is not part of the MapId");
    m.list = Some(ListInfo {
        base: Some("alpha".into()),
        sort: Some(-5),
        name: [("ja".to_string(), "コールド".to_string())].into(),
        description: [("pt".to_string(), "Frio".to_string())].into(),
    });
    let b = base_entry();
    let e = Entry::for_manifest(&m, LIST_ID_FIRST, Some(&b), vec![0; PREVIEW_LEN]);
    assert_eq!(e.names[0], "Coldsnap");
    assert_eq!(e.names[1], "コールド");
    assert_eq!(e.descriptions[8], "Frio");
    assert_eq!(e.descriptions[2], "Cold");
    assert_eq!((e.sort, e.flags, e.teams), (-5, 0x5, [3; 16]));
    let back = Entry::from_v2(&e.to_v2().unwrap()).unwrap();
    assert_eq!(back, e);
}

#[test]
fn list_section_is_checked() {
    let mut m = derived("X", "x");
    m.list = Some(ListInfo {
        base: Some("../a".into()),
        ..Default::default()
    });
    assert!(m.validate().unwrap_err().to_string().contains("base"));
    m.list = Some(ListInfo {
        name: [("xx".to_string(), "a".to_string())].into(),
        ..Default::default()
    });
    assert!(m.validate().unwrap_err().to_string().contains("language"));
    let toml = "[package]\nformat = 1\nname = \"X\"\n[base]\nmap = \"x\"\nderived = true\n[[patch]]\nfile = \"x.map\"\nsha256 = \"0202020202020202020202020202020202020202020202020202020202020202\"\n[list]\nbase = \"lockout\"\nsort = 9\nname.ja = \"y\"\n";
    let parsed = Manifest::from_toml(toml).unwrap();
    assert_eq!(parsed.list.as_ref().unwrap().name["ja"], "y");
    assert_eq!(Manifest::from_toml(&parsed.to_toml()).unwrap(), parsed);
}

#[test]
fn long_text_is_cut_with_room_for_the_terminator() {
    let mut m = derived(&"N".repeat(40), "x");
    m.package.description = Some("é".repeat(200));
    let mut e = Entry::for_manifest(&m, LIST_ID_FIRST, None, vec![0; PREVIEW_LEN]);
    e.names[3] = "😀".repeat(20); // surrogate pairs are never split
    let back = Entry::from_v2(&e.to_v2().unwrap()).unwrap();
    assert_eq!(back.names[0], "N".repeat(31));
    assert_eq!(back.descriptions[0], "é".repeat(127));
    assert_eq!(back.names[3].encode_utf16().count(), 30);
    assert!(back.names[3].ends_with('😀'));
    let mut bad = e.clone();
    bad.preview.pop();
    assert!(bad.to_v2().is_err());
    bad = e.clone();
    bad.scenario = "x".repeat(256);
    assert!(bad.to_v2().is_err());
}

#[test]
fn the_alpha_mask_has_the_shipped_shape() {
    let count = |a: u8| {
        (0..PREVIEW_H)
            .flat_map(|y| (0..PREVIEW_W).map(move |x| (x, y)))
            .filter(|&(x, y)| preview_alpha(x, y) == a)
            .count()
    };
    assert_eq!(count(0), 136, "a 16-pixel triangle");
    assert_eq!(count(0xff), 834, "the frame and the cut's edge");
    assert_eq!(count(0xbf), PREVIEW_W * PREVIEW_H - 136 - 834);
    assert_eq!(preview_alpha(PREVIEW_W - 1, PREVIEW_H - 1), 0);
    assert_eq!(preview_alpha(204, 206), 0);
    assert_eq!(preview_alpha(203, 206), 0xff);
    assert_eq!(preview_alpha(1, 1), 0xbf);
}

#[test]
fn a_png_is_scaled_cropped_and_stored_bgra() {
    // A 440x207 image, left half red, right half blue: covering keeps the
    // scale at 1 and crops the middle 220 columns, half red, half blue.
    let png = png_bytes(
        440,
        207,
        |x, _| if x < 220 { [255, 0, 0] } else { [0, 0, 255] },
    );
    let (w, h, rgba) = decode_image(&png).unwrap();
    assert_eq!((w, h), (440, 207));
    let p = preview_from_rgba(w, h, &rgba);
    assert_eq!(p.len(), PREVIEW_LEN);
    let px = |x: usize, y: usize| &p[(y * PREVIEW_W + x) * 4..][..4];
    assert_eq!(px(10, 100), [0, 0, 255, 0xbf], "red is B=0, G=0, R=255");
    assert_eq!(px(200, 100), [255, 0, 0, 0xbf], "blue");
    assert_eq!(px(0, 0)[3], 0xff);
    // A small square image is scaled up to cover, keeping its colour.
    let png = png_bytes(4, 4, |_, _| [10, 20, 30]);
    let (w, h, rgba) = decode_image(&png).unwrap();
    let p = preview_from_rgba(w, h, &rgba);
    assert_eq!(&p[(50 * PREVIEW_W + 50) * 4..][..3], [30, 20, 10]);
    // Top row first: a vertical gradient keeps its direction.
    let png = png_bytes(220, 207, |_, y| [y as u8, 0, 0]);
    let (w, h, rgba) = decode_image(&png).unwrap();
    let p = preview_from_rgba(w, h, &rgba);
    assert_eq!(p[(5 * PREVIEW_W + 5) * 4 + 2], 5);
    assert_eq!(p[(200 * PREVIEW_W + 5) * 4 + 2], 200);
    assert!(decode_image(b"GIF89a").is_err());
    assert!(
        decode_image(&[0xff, 0xd8, 0xff, 0xe0, 0, 0]).is_err(),
        "a broken JPEG is refused"
    );
}

#[test]
fn preview_falls_back_to_the_base_then_a_card() {
    let dir = std::env::temp_dir().join(format!("h2-content-preview-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut m = derived("X", "x");
    let base = vec![9u8; PREVIEW_LEN];
    assert_eq!(preview_for(&m, &dir, Some(base.clone())).unwrap(), base);
    let card = preview_for(&m, &dir, None).unwrap();
    assert_eq!(card, generated_card(&m.map_id()));
    assert_eq!(card.len(), PREVIEW_LEN);
    assert_eq!(card[3], 0xff);
    assert_eq!(
        preview_for(&m, &dir, Some(vec![1, 2])).unwrap(),
        card,
        "a wrong-sized base is ignored"
    );
    std::fs::write(dir.join("p.png"), png_bytes(8, 8, |_, _| [1, 2, 3])).unwrap();
    m.package.image = Some("p.png".into());
    let img = preview_for(&m, &dir, Some(base)).unwrap();
    assert_eq!(
        &img[(9 * PREVIEW_W + 9) * 4..][..4],
        [3, 2, 1, 0xbf],
        "the image wins"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn ids_are_in_range_stable_and_never_collide() {
    let ids: Vec<MapId> = (0..50u8).map(|i| MapId([i; 32])).collect();
    let got = list_ids(&ids, &[]);
    assert!(
        got.iter()
            .all(|i| (LIST_ID_FIRST..=LIST_ID_LAST).contains(i))
    );
    let mut rev = ids.clone();
    rev.reverse();
    let mut back = list_ids(&rev, &[]);
    back.reverse();
    assert_eq!(got, back, "independent of order");
    assert_eq!(got[3], preferred_id(&ids[3]));
    // Two MapIds preferring the same id, and a retail id in the way.
    let a = MapId([0; 32]);
    let mut b = MapId([0; 32]);
    b.0[31] = 1;
    assert_eq!(preferred_id(&a), preferred_id(&b));
    let p = preferred_id(&a);
    let got = list_ids(&[b, a], &[p]);
    assert_eq!(got, [p + 2, p + 1]);
    // Wrapping at the top of the range.
    let mut top = MapId([0; 32]);
    top.0[..4].copy_from_slice(&(LIST_ID_LAST - LIST_ID_FIRST).to_le_bytes());
    assert_eq!(preferred_id(&top), LIST_ID_LAST);
    assert_eq!(list_ids(&[top], &[LIST_ID_LAST]), [LIST_ID_FIRST]);
}

#[test]
fn offers_are_apart_from_the_map_packs() {
    assert_eq!(offer_content(LIST_ID_FIRST), 0x8001_0000);
    assert!(is_new_map_offer(offer_content(LIST_ID_LAST)));
    for low in [0x0101, 0x03ff, 1, 3, 0x8000_0000, 0xffff_ffff] {
        assert!(!is_new_map_offer(low), "{low:#x}");
    }
}

#[test]
fn base_entries_read_from_a_slot() {
    let mut slot = vec![0u8; SLOT_LEN];
    slot[..4].copy_from_slice(&0x55u32.to_le_bytes());
    slot[SLOT_PATH..SLOT_PATH + 13].copy_from_slice(b"a\\multi\\bravo");
    slot[SLOT_SORT..SLOT_SORT + 4].copy_from_slice(&40u32.to_le_bytes());
    slot[SLOT_FLAGS] = 2;
    slot[SLOT_TEAMS..SLOT_TEAMS + 16].copy_from_slice(&[4; 16]);
    let b = BaseEntry::from_slot(&slot).unwrap();
    assert_eq!(
        b,
        BaseEntry {
            map_id: 0x55,
            map: "bravo".into(),
            sort: 40,
            flags: 2,
            teams: [4; 16]
        }
    );
    slot[..4].fill(0xff);
    assert_eq!(BaseEntry::from_slot(&slot), None, "an empty slot");
    assert_eq!(BaseEntry::from_slot(&slot[..10]), None);
}
