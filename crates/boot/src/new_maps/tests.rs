use super::*;
use content::{map_list::PREVIEW_LEN, patch_lvl, Sha256};

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("h2-new-maps-{tag}-{}", std::process::id()));
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

/// A disc with the given maps and level list.
#[derive(Default)]
struct FakeDisc {
    maps: Vec<&'static str>,
    slots: Vec<Vec<u8>>,
    preview: Option<Vec<u8>>,
    /// Panic on any read: for the retail-unchanged checks.
    untouchable: bool,
    previews_asked: usize,
}
impl Disc for FakeDisc {
    fn has_map(&mut self, name: &str) -> Result<bool> {
        assert!(!self.untouchable, "the disc was read");
        Ok(self.maps.iter().any(|m| m.eq_ignore_ascii_case(name)))
    }
    fn slots(&mut self) -> Result<Vec<Vec<u8>>> {
        assert!(!self.untouchable, "the disc was read");
        Ok(self.slots.clone())
    }
    fn preview(&mut self, _: &[u8]) -> Result<Vec<u8>> {
        self.previews_asked += 1;
        self.preview.clone().context("no preview")
    }
}

fn slot(id: u32, map: &str, flags: u8, teams: [u8; 16], sort: u32) -> Vec<u8> {
    let mut s = vec![0u8; map_list::SLOT_LEN];
    s[..4].copy_from_slice(&id.to_le_bytes());
    let path = format!("scenarios\\multi\\{map}\\{map}");
    s[0xb4c..0xb4c + path.len()].copy_from_slice(path.as_bytes());
    s[0xc4c..0xc50].copy_from_slice(&sort.to_le_bytes());
    s[0xc50] = flags;
    s[0xc54..0xc64].copy_from_slice(&teams);
    s
}

fn disc() -> FakeDisc {
    let mut empty = slot(u32::MAX, "", 0, [0; 16], 0);
    empty[..4].fill(0xff);
    FakeDisc {
        maps: vec!["mainmenu", "alpha", "bravo"],
        slots: vec![
            slot(1, "alpha", 0x3, [2; 16], 10),
            slot(2, "bravo", 0, [4; 16], 5),
            empty,
        ],
        preview: Some(vec![0x42; PREVIEW_LEN]),
        ..Default::default()
    }
}

/// Writes a derived package `dir` for map `map` whose map file is `body`.
fn package(data: &Path, dir: &str, map: &str, body: &[u8], extra: &str) {
    let d = data.join("maps").join(dir);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(format!("{map}.map")), body).unwrap();
    let toml = format!(
        "[package]\nformat = 1\nname = \"Name of {map}\"\ndescription = \"About {map}\"\n{extra}\n[base]\nmap = \"{map}\"\nderived = true\n\n[[patch]]\nfile = \"{map}.map\"\nsha256 = \"{}\"\n",
        Sha256::of(body)
    );
    std::fs::write(d.join("manifest.toml"), toml).unwrap();
}

fn content_root(data: &Path) -> PathBuf {
    vfs::Config::new(data, HALO2_TITLE)
        .title_data_dir()
        .join(vfs::CONTENT_DIR)
}

fn dirs(data: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(content_root(data))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn body(seed: u8) -> Vec<u8> {
    (0..0x1000u32)
        .map(|i| (i as u8).wrapping_mul(seed) ^ 0x5a)
        .collect()
}

#[test]
fn installs_a_new_map_as_a_content_package() {
    let t = Temp::new("install");
    let data = &t.0;
    package(
        data,
        "coldsnap-1",
        "coldsnap",
        &body(3),
        "[list]\nbase = \"alpha\"\nname.ja = \"J\"",
    );
    // A derived package for a disc map stays a replacement, not a new map.
    package(data, "alpha-remake", "alpha", &body(5), "");
    let mut d = disc();
    let r = install(data, &mut d).unwrap();
    assert_eq!(r.installed.len(), 1, "{r:?}");
    let (name, offer, id) = r.installed[0].clone();
    assert_eq!(name, "coldsnap");
    assert_eq!(r.materialised, ["coldsnap"]);
    assert!((map_list::LIST_ID_FIRST..=map_list::LIST_ID_LAST).contains(&id));
    assert_eq!(offer, vfs::offer_id(HALO2_TITLE, 0x8000_0000 | id));
    assert_eq!(dirs(data), [vfs::content_dir_name(offer)]);
    let dir = vfs::Config::new(data, HALO2_TITLE).content_dir(offer);

    // The map, through the resolver: here the layer's own bytes.
    assert_eq!(std::fs::read(dir.join("coldsnap.map")).unwrap(), body(3));
    // patch.lvl passes 1.0's check and names the map beside it.
    let lvl = std::fs::read(dir.join("patch.lvl")).unwrap();
    patch_lvl::validate_v2(&lvl).unwrap();
    assert_eq!(patch_lvl::map_name(&lvl).unwrap(), "coldsnap");
    assert_eq!(patch_lvl::v2_map_id(&lvl), id);
    let e = Entry::from_v2(&lvl).unwrap();
    assert_eq!((e.flags, e.teams), (0x3, [2; 16]), "from the base's entry");
    assert_eq!(e.names[0], "Name of coldsnap");
    assert_eq!(e.names[1], "J");
    assert_eq!(e.descriptions[4], "About coldsnap");
    assert_eq!(
        e.preview,
        vec![0x42; PREVIEW_LEN],
        "no image: the base's preview"
    );
    assert_eq!(d.previews_asked, 1);

    // ContentMeta.xbx: signed as XAPI checks it, with the offer ID.
    let meta = std::fs::read(dir.join("ContentMeta.xbx")).unwrap();
    assert_eq!(
        meta[..20],
        kernel::content::signature(&meta, HALO2_TITLE).unwrap()
    );
    assert_eq!(meta[0x20] & 1, 1, "installed flag");
    assert_eq!(
        u64::from_le_bytes(meta[0x28..0x30].try_into().unwrap()),
        offer
    );
    let at = |o: usize| u32::from_le_bytes(meta[o..o + 4].try_into().unwrap()) as usize;
    let names = &meta[at(0x34)..at(0x34) + at(0x38)];
    assert_eq!(meta[0x3c..0x50], sha1(names));
    let text = String::from_utf16(
        &names
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(
        text.starts_with("\u{feff}[Default]\r\nName=Name of coldsnap\r\n"),
        "{text}"
    );
    assert!(text.contains("[JA]\r\nName=J\r\n"));
    let table = &meta[at(0x50)..at(0x50) + at(0x54)];
    assert_eq!(meta[0x58..0x6c], sha1(table));
    assert_eq!(at(0x50) + at(0x54), meta.len());
    // A changed header byte fails the check.
    let mut bad = meta.clone();
    bad[0x30] ^= 1;
    assert_ne!(
        bad[..20],
        kernel::content::signature(&bad, HALO2_TITLE).unwrap()
    );

    // The VFS serves it where the game enumerates content.
    let marker: Marker =
        toml::from_str(&std::fs::read_to_string(dir.join(MARKER)).unwrap()).unwrap();
    assert_eq!(marker.source, "coldsnap-1");
    assert!(r.line().unwrap().contains("coldsnap"));
}

#[test]
fn refreshes_on_a_new_map_id_and_removes_what_is_gone() {
    let t = Temp::new("refresh");
    let data = &t.0;
    package(data, "coldsnap", "coldsnap", &body(3), "");
    let mut d = disc();
    let first = install(data, &mut d).unwrap();
    let offer = first.installed[0].1;
    // Again: nothing to write.
    let again = install(data, &mut d).unwrap();
    assert!(again.materialised.is_empty());
    assert_eq!(again.installed, first.installed);
    assert!(again.removed.is_empty());
    // A display change rewrites patch.lvl, not the map.
    let manifest = data.join("maps/coldsnap/manifest.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(&manifest, text.replace("Name of coldsnap", "Coldsnap 2")).unwrap();
    let renamed = install(data, &mut d).unwrap();
    assert!(renamed.materialised.is_empty());
    let dir = vfs::Config::new(data, HALO2_TITLE).content_dir(offer);
    assert_eq!(
        Entry::from_v2(&std::fs::read(dir.join("patch.lvl")).unwrap())
            .unwrap()
            .names[0],
        "Coldsnap 2"
    );

    // New map bytes: a new MapId, so a new list id and offer; the old
    // directory goes.
    package(data, "coldsnap", "coldsnap", &body(7), "");
    let changed = install(data, &mut d).unwrap();
    assert_eq!(changed.materialised, ["coldsnap"]);
    let new_offer = changed.installed[0].1;
    assert_ne!(new_offer, offer);
    assert_eq!(
        changed.removed,
        [vfs::Config::new(data, HALO2_TITLE).content_dir(offer)]
    );
    assert_eq!(dirs(data), [vfs::content_dir_name(new_offer)]);
    let dir = vfs::Config::new(data, HALO2_TITLE).content_dir(new_offer);
    assert_eq!(std::fs::read(dir.join("coldsnap.map")).unwrap(), body(7));

    // Other content is never touched: the Map Pack's and any other offer.
    let pack = content_root(data).join("4d53006400000101");
    let other = content_root(data).join("4d53006400000002");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    // The source package is gone: its directory is removed.
    std::fs::remove_dir_all(data.join("maps/coldsnap")).unwrap();
    let gone = install(data, &mut d).unwrap();
    assert!(gone.installed.is_empty());
    assert_eq!(gone.removed.len(), 1);
    assert_eq!(dirs(data), ["4d53006400000002", "4d53006400000101"]);
}

#[test]
fn retail_is_unchanged_without_new_maps() {
    let t = Temp::new("retail");
    let data = &t.0;
    let mut untouchable = FakeDisc {
        untouchable: true,
        ..Default::default()
    };
    // No maps folder at all.
    let r = install(data, &mut untouchable).unwrap();
    assert!(r.line().is_none());
    assert!(!data.join("disk").exists(), "nothing created");
    // Packages, none derived: the disc is not read either.
    let d = data.join("maps/alpha-mod");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("manifest.toml"),
        format!(
            "[package]\nformat = 1\nname = \"m\"\n[base]\nmap = \"alpha\"\nsha256 = \"{}\"\n",
            Sha256::of(b"x")
        ),
    )
    .unwrap();
    let r = install(data, &mut untouchable).unwrap();
    assert!(r.line().is_none());
    assert!(!data.join("disk").exists());
    // A derived package for a disc map: the disc is asked, nothing written.
    package(data, "alpha-remake", "alpha", &body(5), "");
    let r = install(data, &mut disc()).unwrap();
    assert!(r.line().is_none(), "{r:?}");
    assert!(!data.join("disk").exists());
}

#[test]
fn ids_avoid_retail_ids_and_each_other() {
    let t = Temp::new("ids");
    let data = &t.0;
    for (i, n) in ["one", "two", "three", "four"].iter().enumerate() {
        package(data, n, n, &body(i as u8 + 1), "");
    }
    let mut d = disc();
    let r = install(data, &mut d).unwrap();
    let ids: BTreeSet<u32> = r.installed.iter().map(|x| x.2).collect();
    assert_eq!(ids.len(), 4);
    assert!(!ids.contains(&1) && !ids.contains(&2));
    let offers: BTreeSet<u64> = r.installed.iter().map(|x| x.1).collect();
    assert_eq!(offers.len(), 4);
    assert_eq!(dirs(data).len(), 4);
}

#[test]
fn images_defaults_and_the_map_pack_template() {
    let t = Temp::new("image");
    let data = &t.0;
    // An installed Map Pack package gives the header's shape.
    let pack = content_root(data).join("4d53006400000101");
    std::fs::create_dir_all(&pack).unwrap();
    let mut tpl = vec![0u8; 0x100];
    tpl[0x14..0x18].copy_from_slice(b"XCSF");
    tpl[0x1c] = 7;
    tpl[0x30] = 9;
    tpl[0x70] = 0x33;
    std::fs::write(pack.join("ContentMeta.xbx"), &tpl).unwrap();
    // A package with an image and no base; one with neither.
    package(data, "pic", "pic", &body(2), "image = \"shot.png\"");
    let png = {
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, 2, 2);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        let mut w = e.write_header().unwrap();
        w.write_image_data(&[10, 20, 30].repeat(4)).unwrap();
        drop(w);
        out
    };
    std::fs::write(data.join("maps/pic/shot.png"), &png).unwrap();
    package(
        data,
        "plain",
        "plain",
        &body(4),
        "[list]\nbase = \"nowhere\"\nsort = 77",
    );
    let mut d = disc();
    let r = install(data, &mut d).unwrap();
    assert_eq!(r.installed.len(), 2, "{r:?}");
    assert_eq!(d.previews_asked, 0, "an image, or no base found");
    assert!(r.notices.iter().any(|n| n.contains("nowhere")));
    for (name, offer, _) in &r.installed {
        let dir = vfs::Config::new(data, HALO2_TITLE).content_dir(*offer);
        let e = Entry::from_v2(&std::fs::read(dir.join("patch.lvl")).unwrap()).unwrap();
        let meta = std::fs::read(dir.join("ContentMeta.xbx")).unwrap();
        assert_eq!(
            (meta[0x1c], meta[0x30], meta[0x70]),
            (7, 9, 0x33),
            "the template's words"
        );
        assert_eq!(
            meta[..20],
            kernel::content::signature(&meta, HALO2_TITLE).unwrap()
        );
        assert_eq!(
            (e.flags, e.teams),
            (0, [4; 16]),
            "no base: the list's first retail entry"
        );
        if name == "pic" {
            assert_eq!(std::fs::read(dir.join("shot.png")).unwrap(), png);
            assert_eq!(&e.preview[(9 * 220 + 9) * 4..][..4], [30, 20, 10, 0xbf]);
            assert_eq!(e.sort, map_list::DEFAULT_SORT);
        } else {
            assert_eq!(e.sort, 77);
            assert_eq!(
                e.preview,
                map_list::generated_card(
                    &content::Library::open(data.join("maps"))
                        .packages()
                        .iter()
                        .find(|p| p.dir_name() == "plain")
                        .unwrap()
                        .id
                )
            );
        }
    }
    // The Map Pack's package is left alone.
    assert_eq!(std::fs::read(pack.join("ContentMeta.xbx")).unwrap(), tpl);
}

#[test]
fn a_bad_package_is_skipped_not_fatal() {
    let t = Temp::new("bad");
    let data = &t.0;
    package(data, "good", "good", &body(2), "");
    package(data, "bad", "bad", &body(3), "");
    std::fs::write(data.join("maps/bad/bad.map"), b"changed after hashing").unwrap();
    package(data, "long", &"x".repeat(39), &body(4), "");
    let r = install(data, &mut disc()).unwrap();
    assert_eq!(r.installed.len(), 1);
    assert_eq!(r.skipped.len(), 2, "{r:?}");
    assert!(r.skipped.iter().any(|s| s.contains("FATX")));
    assert_eq!(dirs(data).len(), 1, "the failed one's directory is removed");
}

/// The user's disc: a record for a derived map with a real retail base,
/// checked against that base's own entry. Reads the disc, writes nothing
/// to it. `H2_DISC=disc/halo2.iso cargo test -p boot --lib real_disc -- --ignored --nocapture`
#[test]
#[ignore]
fn real_disc_base_entry_and_record() {
    let path = std::env::var_os("H2_DISC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../disc/halo2.iso"));
    let mut d = IsoDisc::new(&path);
    assert!(d.has_map("lockout").unwrap());
    assert!(!d.has_map("coldsnap").unwrap());
    let slots = d.slots().unwrap();
    let bases: Vec<BaseEntry> = slots
        .iter()
        .filter_map(|s| BaseEntry::from_slot(s))
        .collect();
    eprintln!("{} slots, {} used", slots.len(), bases.len());
    for b in &bases {
        eprintln!(
            "  {:>12} id {:#x} sort {} flags {:#x} teams {:02x?}",
            b.map, b.map_id, b.sort, b.flags, b.teams
        );
        assert!(
            b.map_id < map_list::LIST_ID_FIRST,
            "retail ids stay below the new range"
        );
    }
    let (lockout, slot) = bases
        .iter()
        .zip(slots.iter().filter(|s| BaseEntry::from_slot(s).is_some()))
        .find(|(b, _)| b.map == "lockout")
        .expect("lockout has an entry");
    // Language order: Japanese (index 1) is not ASCII on the retail entry.
    // Language order: the names are in nine languages, Chinese (index 7)
    // in Han characters on the retail entry.
    let units = |i: usize| -> Vec<u16> {
        slot[0xc + i * 64..0xc + i * 64 + 64]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|&u| u != 0)
            .collect()
    };
    assert!(units(0).iter().all(|&u| u < 0x80), "names[0] is English");
    assert!(
        units(7).iter().any(|&u| (0x4e00..0xa000).contains(&u)),
        "names[7] is Chinese"
    );
    // The base's picture decodes to a preview.
    let px = d.preview(slot).unwrap();
    assert_eq!(px.len(), PREVIEW_LEN);
    let mut m = content::Manifest::retail("coldsnap", Sha256([1; 32]));
    m.base.sha256 = None;
    m.base.derived = true;
    m.patches = vec![content::manifest::Layer {
        file: "coldsnap.map".into(),
        sha256: Sha256([2; 32]),
    }];
    m.list = Some(content::manifest::ListInfo {
        base: Some("lockout".into()),
        ..Default::default()
    });
    let ids = map_list::list_ids(
        &[m.map_id()],
        &bases.iter().map(|b| b.map_id).collect::<Vec<_>>(),
    );
    let e = Entry::for_manifest(&m, ids[0], Some(lockout), px);
    let r = e.to_v2().unwrap();
    patch_lvl::validate_v2(&r).unwrap();
    // The record's fields sit where 1.0 copies them from into a slot:
    // flags and team counts equal to the base slot's own bytes.
    assert_eq!(r[0xc58], slot[0xc50]);
    assert_eq!(r[0xc5c..0xc6c], slot[0xc54..0xc64]);
    assert_eq!(Entry::from_v2(&r).unwrap().teams, lockout.teams);
    assert!(!bases.iter().any(|b| b.map_id == ids[0]));
    // The Map Pack's records, if its ISO is beside the disc, have the alpha
    // shape our previews get.
    let pack = path.with_file_name("mappack.iso");
    if let Ok(f) = File::open(&pack) {
        let mut iso = xiso::Xiso::open(BufReader::new(f)).unwrap();
        let mut checked = 0;
        for e in iso.entries().unwrap() {
            if !e.path.to_ascii_lowercase().ends_with("patch_v3.lvl") {
                continue;
            }
            let v3 = iso.read(&e).unwrap();
            for rec in patch_lvl::v3_records(&v3).unwrap() {
                let v2 = patch_lvl::v2_from_v3(rec).unwrap();
                let e = Entry::from_v2(&v2).unwrap();
                for (i, p) in e.preview.as_chunks::<4>().0.iter().enumerate() {
                    assert_eq!(p[3], map_list::preview_alpha(i % 220, i / 220));
                }
                let cjk = |t: &str| {
                    t.chars()
                        .filter(|c| {
                            ('\u{3040}'..'\u{a000}').contains(c)
                                || ('\u{ac00}'..'\u{d7b0}').contains(c)
                        })
                        .count()
                };
                eprintln!(
                    "  map pack {} sort {} flags {:#x} cjk per language {:?}",
                    patch_lvl::map_name(&v2).unwrap(),
                    e.sort,
                    e.flags,
                    e.names.iter().map(|n| cjk(n)).collect::<Vec<_>>()
                );
                assert!(e.map_id < map_list::LIST_ID_FIRST);
                checked += 1;
            }
        }
        eprintln!("{checked} map pack records have the preview alpha shape");
    }
}
