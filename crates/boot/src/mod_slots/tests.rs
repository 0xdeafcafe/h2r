use super::*;
use content::Sha256;
use content::map_list::PREVIEW_LEN;

const BITM: u32 = u32::from_be_bytes(*b"bitm");
const LIST: u32 = 0x8000_0000;

fn slot_bytes(id: u32, path: &str, bitmap: u32, name: &str) -> Vec<u8> {
    let mut s = vec![0u8; SLOT_LEN];
    s[..4].copy_from_slice(&id.to_le_bytes());
    s[4..8].copy_from_slice(&BITM.to_le_bytes());
    s[8..12].copy_from_slice(&bitmap.to_le_bytes());
    let n = utf16_field(name, 0x20);
    for lang in 0..9 {
        s[MP_NAME + lang * 0x40..][..0x40].copy_from_slice(&n);
    }
    s[MP_PATH..MP_PATH + path.len()].copy_from_slice(path.as_bytes());
    s[MP_SORT..MP_SORT + 4].copy_from_slice(&10u32.to_le_bytes());
    s[FLAGS] = 3;
    s[TEAMS..TEAMS + 16].copy_from_slice(&[4; 16]);
    s
}

fn slots(list: &[(u32, &str, u32, &str)]) -> Vec<Slot> {
    list.iter()
        .enumerate()
        .map(|(i, &(id, path, bitmap, name))| Slot {
            at: LIST + (i * SLOT_LEN) as u32,
            bytes: slot_bytes(id, path, bitmap, name),
        })
        .collect()
}

fn retail() -> Vec<Slot> {
    slots(&[
        (0x0a, "scenarios\\multi\\lockout\\lockout", 0x100, "Lockout"),
        (
            0x14,
            "scenarios\\multi\\ascension\\ascension",
            0x101,
            "Ascension",
        ),
        (
            0x3e8,
            "t:\\$C\\4D53006400000101\\triplicate",
            0x102,
            "Containment",
        ),
        (FREE, "", 0x200, ""),
        (FREE, "", 0x201, ""),
    ])
}

fn fill(map: &str, name: &str, preview: Option<Vec<u8>>) -> Fill {
    Fill {
        map: map.into(),
        package: MapId([7; 32]),
        names: std::array::from_fn(|_| name.to_string()),
        descriptions: std::array::from_fn(|_| format!("About {name}")),
        preview,
    }
}

/// What the steps do to the slots, `0x19be00` included (the slot cleared
/// but its bitmap reference, filled from the record).
fn apply(slots: &mut [Slot], steps: &[Step]) {
    let find = |slots: &mut [Slot], at: u32| -> Option<(usize, usize)> {
        slots
            .iter()
            .position(|s| (s.at..s.at + SLOT_LEN as u32).contains(&at))
            .map(|i| {
                let off = (at - slots[i].at) as usize;
                (i, off)
            })
    };
    for step in steps {
        match step {
            Step::Write { at, bytes } => {
                let (i, off) = find(slots, *at).unwrap();
                slots[i].bytes[off..off + bytes.len()].copy_from_slice(bytes);
            }
            Step::Record {
                slot,
                bitmap,
                dir,
                record,
                ..
            } => {
                let (i, _) = find(slots, *slot).unwrap();
                let e = Entry::from_v2(record).unwrap();
                let s = &mut slots[i].bytes;
                s.fill(0);
                s[..4].copy_from_slice(&e.map_id.to_le_bytes());
                s[4..8].copy_from_slice(&bitmap[0].to_le_bytes());
                s[8..12].copy_from_slice(&bitmap[1].to_le_bytes());
                for (l, n) in e.names.iter().enumerate() {
                    s[MP_NAME + l * 0x40..][..0x40].copy_from_slice(&utf16_field(n, 0x20));
                }
                for (l, d) in e.descriptions.iter().enumerate() {
                    s[0x24c + l * 0x100..][..0x100].copy_from_slice(&utf16_field(d, 0x80));
                }
                let base = e.scenario.rsplit('\\').next().unwrap();
                let path = format!("{dir}\\{base}");
                s[MP_PATH..MP_PATH + path.len()].copy_from_slice(path.as_bytes());
                s[MP_SORT..MP_SORT + 4].copy_from_slice(&(e.sort as u32).to_le_bytes());
                s[FLAGS] = e.flags;
                s[TEAMS..TEAMS + 16].copy_from_slice(&e.teams);
            }
        }
    }
}

fn name_of(s: &Slot, lang: usize) -> String {
    let f = &s.bytes[MP_NAME + lang * 0x40..][..0x40];
    let u: Vec<u16> = f
        .chunks(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&u)
}

#[test]
fn a_mod_without_a_picture_renames_its_map_s_slot_and_keeps_the_picture() {
    let mut list = retail();
    let mut state = State::default();
    let fills = [fill("lockout", "Lockout Snow", None)];
    let steps = plan(&list, &fills, &mut state);
    assert_eq!(
        steps,
        [Step::Write {
            at: LIST + MP_NAME as u32,
            bytes: texts(&fills[0]),
        }]
    );
    apply(&mut list, &steps);
    for lang in 0..9 {
        assert_eq!(name_of(&list[0], lang), "Lockout Snow");
    }
    assert_eq!(list[0].bitmap(), [BITM, 0x100], "the map's own picture");
    // Ascension and the rest untouched; nothing more to do.
    assert_eq!(list[1].bytes, retail()[1].bytes);
    assert!(plan(&list, &fills, &mut state).is_empty());
}

#[test]
fn a_mod_with_a_picture_takes_a_free_slot_s_bitmap_and_the_game_s_applier() {
    let mut list = retail();
    let mut state = State::default();
    let px = vec![0x5a; PREVIEW_LEN];
    let fills = [fill("lockout", "Lockout Snow", Some(px.clone()))];
    let steps = plan(&list, &fills, &mut state);
    let [
        Step::Record {
            slot,
            bitmap,
            dir,
            record,
            ..
        },
    ] = &steps[..]
    else {
        panic!("{steps:?}");
    };
    assert_eq!(*slot, LIST);
    assert_eq!(*bitmap, [BITM, 0x200], "the first free slot's bitmap");
    assert_eq!(dir, "scenarios\\multi\\lockout");
    // The record is the slot itself with the package's texts and picture:
    // it passes 1.0's check, and the path the applier makes is the slot's.
    let e = Entry::from_v2(record).unwrap();
    assert_eq!(e.map_id, 0x0a);
    assert_eq!(e.scenario, "scenarios\\multi\\lockout\\lockout");
    assert_eq!((e.sort, e.flags, e.teams), (10, 3, [4; 16]));
    assert_eq!(e.names[0], "Lockout Snow");
    assert_eq!(e.descriptions[8], "About Lockout Snow");
    assert_eq!(e.preview, px);
    apply(&mut list, &steps);
    assert_eq!(list[0].path(), "scenarios\\multi\\lockout\\lockout");
    assert_eq!(list[0].bitmap(), [BITM, 0x200]);
    assert_eq!(name_of(&list[0], 3), "Lockout Snow");
    // Filled: nothing more, and the next fill keeps the same bitmap.
    assert!(plan(&list, &fills, &mut state).is_empty());
    let other = [Fill {
        package: MapId([8; 32]),
        ..fill("lockout", "Lockout Night", Some(px))
    }];
    let again = plan(&list, &other, &mut state);
    let [Step::Record { bitmap, .. }] = &again[..] else {
        panic!("{again:?}");
    };
    assert_eq!(*bitmap, [BITM, 0x200], "its own spare, not another");
}

#[test]
fn a_package_gone_puts_the_slot_s_own_bytes_back() {
    let mut list = retail();
    let mut state = State::default();
    let fills = [fill("lockout", "Lockout Snow", Some(vec![1; PREVIEW_LEN]))];
    let steps = plan(&list, &fills, &mut state);
    apply(&mut list, &steps);
    let back = plan(&list, &[], &mut state);
    assert_eq!(
        back,
        [Step::Write {
            at: LIST + 4,
            bytes: retail()[0].bytes[4..MP_PATH].to_vec(),
        }]
    );
    apply(&mut list, &back);
    assert_eq!(name_of(&list[0], 0), "Lockout");
    assert_eq!(list[0].bitmap(), [BITM, 0x100]);
    assert!(plan(&list, &[], &mut state).is_empty());
    // A fresh load forgets: nothing to put back.
    state.reset();
    assert!(plan(&list, &[], &mut state).is_empty());
}

#[test]
fn a_picture_dropped_gives_the_map_its_own_bitmap_back() {
    let mut list = retail();
    let mut state = State::default();
    let with = [fill("lockout", "Lockout Snow", Some(vec![1; PREVIEW_LEN]))];
    let p = plan(&list, &with, &mut state);
    apply(&mut list, &p);
    let without = [fill("lockout", "Lockout Snow", None)];
    let steps = plan(&list, &without, &mut state);
    assert_eq!(
        steps,
        [Step::Write {
            at: LIST + 4,
            bytes: retail()[0].bytes[4..12].to_vec(),
        }]
    );
}

#[test]
fn with_no_free_bitmap_a_mod_keeps_the_map_s_picture() {
    let mut list = slots(&[
        (0x0a, "scenarios\\multi\\lockout\\lockout", 0x100, "Lockout"),
        // A free slot whose bitmap a used slot already shows.
        (FREE, "", 0x100, ""),
    ]);
    let mut state = State::default();
    let fills = [fill("lockout", "Lockout Snow", Some(vec![1; PREVIEW_LEN]))];
    let steps = plan(&list, &fills, &mut state);
    assert!(matches!(&steps[..], [Step::Write { at, .. }] if *at == LIST + MP_NAME as u32));
    apply(&mut list, &steps);
    assert_eq!(list[0].bitmap(), [BITM, 0x100]);
}

#[test]
fn which_slots_a_mod_takes() {
    let list = slots(&[
        (0x0a, "scenarios\\multi\\lockout\\lockout", 0x100, "Lockout"),
        (
            0x3e8,
            "t:\\$C\\4D53006400000101\\triplicate",
            0x101,
            "Containment",
        ),
        // A Map Pack map of a disc map's file name: the disc's.
        (
            0x3f2,
            "t:\\$C\\4D53006400000102\\lockout",
            0x102,
            "Pack Lockout",
        ),
        // A new map's content package: never refilled.
        (
            0x2_0001,
            "t:\\$C\\4D53006480020001\\coldsnap",
            0x103,
            "Coldsnap",
        ),
    ]);
    let mut state = State::default();
    let at = |steps: Vec<Step>| -> Vec<u32> {
        steps
            .iter()
            .map(|s| match s {
                Step::Write { at, .. } => (at - LIST) / SLOT_LEN as u32,
                Step::Record { slot, .. } => (slot - LIST) / SLOT_LEN as u32,
            })
            .collect()
    };
    assert_eq!(
        at(plan(&list, &[fill("lockout", "L", None)], &mut state)),
        [0]
    );
    state.reset();
    assert_eq!(
        at(plan(&list, &[fill("triplicate", "T", None)], &mut state)),
        [1]
    );
    state.reset();
    assert!(plan(&list, &[fill("coldsnap", "C", None)], &mut state).is_empty());
    assert!(plan(&list, &[fill("midship", "M", None)], &mut state).is_empty());
}

#[test]
fn texts_cut_to_the_slot_s_fields() {
    let long = "x".repeat(40);
    let t = texts(&fill("lockout", &long, None));
    assert_eq!(t.len(), MP_PATH - MP_NAME);
    // 31 units and a NUL.
    assert_eq!(&t[62..64], &[0, 0]);
    assert_eq!(t[60], b'x');
}

// ---- reading the list from the guest ----

#[derive(Default)]
struct Mem(BTreeMap<u32, u8>);

impl Gm for Mem {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        for (i, b) in out.iter_mut().enumerate() {
            *b = *self.0.get(&(addr + i as u32)).unwrap_or(&0);
        }
        true
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        for (i, b) in data.iter().enumerate() {
            self.0.insert(addr + i as u32, *b);
        }
        true
    }
}

#[test]
fn the_slots_are_read_from_the_main_menu_s_globals_only() {
    let mut m = Mem::default();
    let (header, matg, ui) = (0x1000, 0x2000, 0x3000);
    m.set_u32(CACHE_HEADER, header);
    m.set_u16(header + 0x10, 2);
    m.set_u32(GLOBALS_TAG, matg);
    m.set_u32(matg + 0x178, 1);
    m.set_u32(matg + 0x17c, ui);
    m.set_u32(ui + 0x10, 2);
    m.set_u32(ui + 0x14, LIST);
    for s in &retail()[..2] {
        m.wr(s.at, &s.bytes);
    }
    let got = read_slots(&m);
    assert_eq!(got, retail()[..2].to_vec());
    // A game map (cache type 1): none.
    m.set_u16(header + 0x10, 1);
    assert!(read_slots(&m).is_empty());
    // No UI level data: none.
    m.set_u16(header + 0x10, 2);
    m.set_u32(matg + 0x178, 0);
    assert!(read_slots(&m).is_empty());
}

// ---- the packages ----

struct Temp(std::path::PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("h2-mod-slots-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A package that replaces `map`, with a 2x2 PNG when `image`.
pub(crate) fn replacing_package(data: &Path, dir: &str, map: &str, name: &str, image: bool) {
    let d = data.join("maps").join(dir);
    std::fs::create_dir_all(&d).unwrap();
    let patch = b"PPF30 not read here".to_vec();
    std::fs::write(d.join("mod.ppf"), &patch).unwrap();
    let mut toml =
        format!("[package]\nformat = 1\nname = \"{name}\"\ndescription = \"About {name}\"\n");
    if image {
        let mut png = Vec::new();
        {
            let mut e = png::Encoder::new(&mut png, 2, 2);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            let mut w = e.write_header().unwrap();
            w.write_image_data(&[200u8; 16]).unwrap();
        }
        std::fs::write(d.join("preview.png"), png).unwrap();
        toml += "image = \"preview.png\"\n";
    }
    toml += &format!(
        "\n[base]\nmap = \"{map}\"\nsha256 = \"{}\"\n\n[[patch]]\nfile = \"mod.ppf\"\nsha256 = \"{}\"\n",
        Sha256::of(b"the disc's map"),
        Sha256::of(&patch)
    );
    std::fs::write(d.join("manifest.toml"), toml).unwrap();
}

#[test]
fn a_replacing_package_fills_with_its_name_description_and_image() {
    let t = Temp::new("fills");
    assert!(replacing(&t.0).is_empty(), "no maps folder");
    replacing_package(&t.0, "live-0001", "lockout", "Lockout Snow", true);
    replacing_package(&t.0, "live-0002", "ascension", "Ascension Night", false);
    let fills = replacing(&t.0);
    assert_eq!(fills.len(), 2, "{fills:?}");
    let a = fills.iter().find(|f| f.map == "ascension").unwrap();
    assert_eq!(a.names[0], "Ascension Night");
    assert_eq!(a.descriptions[4], "About Ascension Night");
    assert_eq!(a.preview, None);
    let l = fills.iter().find(|f| f.map == "lockout").unwrap();
    assert_eq!(l.names[7], "Lockout Snow");
    let px = l.preview.as_ref().expect("the package's image");
    assert_eq!(px.len(), PREVIEW_LEN);
}

#[test]
fn each_maps_picture_is_its_slots_bitmap() {
    let mut list = retail();
    // A Map Pack map of a disc map's name: the disc's slot wins.
    list.extend(slots(&[(
        0x3e9,
        "t:\\$C\\4D53006400000101\\lockout",
        0x103,
        "Lockout Again",
    )]));
    let p = pictures(&list);
    assert_eq!(p.get("lockout"), Some(&0x100));
    assert_eq!(p.get("ascension"), Some(&0x101));
    assert_eq!(p.get("triplicate"), Some(&0x102), "a Map Pack map's own");
    assert_eq!(p.len(), 3, "free slots show nothing");
}
