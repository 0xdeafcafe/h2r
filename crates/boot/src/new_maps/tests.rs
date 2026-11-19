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
    base_files: HashMap<String, vfs::HostFile>,
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
    fn map_file(&mut self, name: &str) -> Result<Option<vfs::HostFile>> {
        Ok(self.base_files.get(&name.to_ascii_lowercase()).cloned())
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
            empty.clone(),
            empty.clone(),
            empty.clone(),
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

fn device_path(offer: u64, map: &str) -> String {
    format!("{}\\{}", vfs::DATA_DEVICE, content_path(offer, map))
}

fn mounted(data: &Path, report: &Report) -> vfs::Vfs {
    let cfg = vfs::Config::new(data, HALO2_TITLE);
    cfg.content_maps.replace(report.mounts.clone());
    vfs::Vfs::with_storage(cfg).unwrap()
}

fn read(vfs: &mut vfs::Vfs, path: &str) -> Vec<u8> {
    let handle = vfs.open(path, vfs::OpenOptions::default()).unwrap().handle;
    let len = vfs.query_information(handle).unwrap().metadata.end_of_file;
    let mut bytes = vec![0; len as usize];
    assert_eq!(
        vfs.read(handle, vfs::Offset::Absolute(0), &mut bytes)
            .unwrap(),
        bytes.len()
    );
    vfs.close(handle).unwrap();
    bytes
}

fn served(data: &Path, report: &Report, offer: u64, map: &str) -> Vec<u8> {
    read(&mut mounted(data, report), &device_path(offer, map))
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
    let mut d = disc();
    let r = install(data, &mut d).unwrap();
    assert_eq!(r.installed.len(), 1, "{r:?}");
    let (name, offer, id) = r.installed[0].clone();
    assert_eq!(name, "coldsnap");
    assert!((map_list::LIST_ID_FIRST..=map_list::LIST_ID_LAST).contains(&id));
    assert_eq!(offer, vfs::offer_id(HALO2_TITLE, 0x8000_0000 | id));
    assert_eq!(dirs(data), [vfs::content_dir_name(offer)]);
    let dir = vfs::Config::new(data, HALO2_TITLE).content_dir(offer);

    // The map, through the resolver: here the layer's own bytes.
    assert_eq!(served(data, &r, offer, "coldsnap"), body(3));
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
    assert_eq!(again.installed, first.installed);
    assert!(again.removed.is_empty());
    // A display change rewrites patch.lvl, not the map.
    let manifest = data.join("maps/coldsnap/manifest.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(&manifest, text.replace("Name of coldsnap", "Coldsnap 2")).unwrap();
    let renamed = install(data, &mut d).unwrap();
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
    let new_offer = changed.installed[0].1;
    assert_ne!(new_offer, offer);
    assert_eq!(
        changed.removed,
        [vfs::Config::new(data, HALO2_TITLE).content_dir(offer)]
    );
    assert_eq!(dirs(data), [vfs::content_dir_name(new_offer)]);
    assert_eq!(served(data, &changed, new_offer, "coldsnap"), body(7));

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
    // A package whose base map is missing produces no offer.
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
    let r = install(data, &mut disc()).unwrap();
    assert!(r.installed.is_empty());
    assert_eq!(r.skipped.len(), 1);
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
    assert_eq!(
        d.previews_asked, 1,
        "an image, or no base found: retail's frame, once"
    );
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
            assert!(!dir.join("shot.png").exists());
            assert_eq!(&e.preview[(9 * 220 + 9) * 4..][..4], [30, 20, 10, 0xbf]);
            // The border is retail's (the disc's preview), as a retail
            // row's picture: Select Map showed ours with none.
            for (x, y) in [(0, 0), (219, 0), (0, 206), (100, 206), (219, 100)] {
                assert_eq!(
                    &e.preview[(y * 220 + x) * 4..][..4],
                    [0x42, 0x42, 0x42, 0xff],
                    "{x},{y}"
                );
            }
            assert_eq!(e.sort, map_list::DEFAULT_SORT);
        } else {
            assert_eq!(e.sort, 77);
            let mut card = map_list::generated_card(
                &content::Library::open(data.join("maps"))
                    .packages()
                    .iter()
                    .find(|p| p.dir_name() == "plain")
                    .unwrap()
                    .id,
            );
            map_list::with_frame(&mut card, &[0x42; PREVIEW_LEN]);
            assert_eq!(e.preview, card, "a generated card in retail's frame");
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
    assert_eq!(r.installed.len(), 2);
    assert_eq!(r.skipped.len(), 1, "{r:?}");
    assert!(r.skipped.iter().any(|s| s.contains("FATX")));
    let bad = r.installed.iter().find(|(map, ..)| map == "bad").unwrap();
    let mut vfs = mounted(data, &r);
    assert_eq!(
        vfs.open(&device_path(bad.1, "bad"), vfs::OpenOptions::default())
            .unwrap_err(),
        vfs::Error::FileCorrupt
    );
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

#[test]
fn a_converted_map_installs_the_sandbox_the_setting_names() {
    let t = Temp::new("sandbox");
    let data = &t.0;
    let d = data.join("maps").join("h1_gulch");
    for (s, seed) in [("h1", 7), ("h2", 9)] {
        std::fs::create_dir_all(d.join(s)).unwrap();
        std::fs::write(d.join(s).join("h1_gulch.map"), body(seed)).unwrap();
    }
    let variant = |s: &str, seed: u8| {
        format!(
            "[[variant]]\nfile = \"{s}/h1_gulch.map\"\nsha256 = \"{}\"\n\
             origin = {{ game = \"halo1\", map = \"gulch\", sandbox = \"{s}\", source_sha256 = \"{}\" }}\n",
            Sha256::of(&body(seed)),
            Sha256([7; 32])
        )
    };
    std::fs::write(
        d.join("manifest.toml"),
        format!(
            "[package]\nformat = 1\nname = \"Gulch\"\n[base]\nmap = \"h1_gulch\"\nderived = true\n{}{}",
            variant("h1", 7),
            variant("h2", 9)
        ),
    )
    .unwrap();
    let ids = std::cell::RefCell::new(Vec::new());
    let installed = |data: &Path| {
        let r = install(data, &mut disc()).unwrap();
        assert_eq!(r.installed.len(), 1, "{r:?}");
        ids.borrow_mut().push((r.installed[0].1, r.installed[0].2));
        served(data, &r, r.installed[0].1, "h1_gulch")
    };
    // Halo 1's sandbox by default.
    assert_eq!(installed(data), body(7));
    // The setting asks for Halo 2's: the next boot writes that one.
    std::fs::write(
        settings::path_in(data),
        "[multiplayer]\nhalo1_sandbox = \"h2\"\n",
    )
    .unwrap();
    assert_eq!(installed(data), body(9));
    // One map in the list either way: the same offer and list ID.
    let ids = ids.into_inner();
    assert_eq!(ids[0], ids[1]);
    assert_eq!(dirs(data).len(), 1);
}

/// The install reports both variants of a converted map at the path 1.0
/// opens, and a VFS given them serves the one chosen for the next game,
/// whatever the boot materialised.
#[test]
fn a_converted_map_is_served_in_the_sandbox_chosen_per_game() {
    let t = Temp::new("per-game");
    let data = &t.0;
    let d = data.join("maps").join("h1_gulch");
    for (s, seed) in [("h1", 7), ("h2", 9)] {
        std::fs::create_dir_all(d.join(s)).unwrap();
        std::fs::write(d.join(s).join("h1_gulch.map"), body(seed)).unwrap();
    }
    let variant = |s: &str, seed: u8| {
        format!(
            "[[variant]]\nfile = \"{s}/h1_gulch.map\"\nsha256 = \"{}\"\n\
             origin = {{ game = \"halo1\", map = \"gulch\", sandbox = \"{s}\", source_sha256 = \"{}\" }}\n",
            Sha256::of(&body(seed)),
            Sha256([7; 32])
        )
    };
    std::fs::write(
        d.join("manifest.toml"),
        format!(
            "[package]\nformat = 1\nname = \"Gulch\"\n[base]\nmap = \"h1_gulch\"\nderived = true\n{}{}",
            variant("h1", 7),
            variant("h2", 9)
        ),
    )
    .unwrap();
    let r = install(data, &mut disc()).unwrap();
    let offer = r.installed[0].1;
    assert_eq!(r.variants.len(), 1, "{r:?}");
    let (path, pair) = &r.variants[0];
    assert_eq!(path, &content_path(offer, "h1_gulch"));
    assert!(path.starts_with("TDATA\\4D530064\\$c\\"), "{path}");
    assert_eq!(std::fs::read(&pair.h1.file).unwrap(), body(7));
    assert_eq!(std::fs::read(&pair.h2.file).unwrap(), body(9));
    assert_ne!(pair.h1.id, pair.h2.id);
    // The path names the file the install wrote.
    let host = data.join("disk").join("Partition1").join(path.replace('\\', "/"));
    assert!(host.is_file(), "{}", host.display());

    let mut cfg = vfs::Config::new(data, HALO2_TITLE);
    for (p, pair) in &r.variants {
        cfg.variants.register(p, pair.clone());
    }
    let choice = cfg.variants.choice().clone();
    let mut v = vfs::Vfs::with_storage(cfg).unwrap();
    let name = format!("{}\\{path}", vfs::DATA_DEVICE);
    assert_eq!(v.host_file(&name).unwrap().path, pair.h1.file);
    choice.set_halo1(false);
    let served = v.host_file(&name).unwrap();
    assert_eq!(served.path, pair.h2.file);
    assert_eq!(served.len, body(9).len() as u64);
    assert_eq!(v.map_id(&name), pair.h2.id);
}

fn retail_package(data: &Path, dir: &str, name: &str, map: &str, base: &[u8], value: u8) -> MapId {
    let dir = data.join("maps").join(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let patch = [b"PATCH\0\x08\0\0\x01".as_slice(), &[value], b"EOF"].concat();
    std::fs::write(dir.join("a.ips"), &patch).unwrap();
    let mut manifest = content::Manifest::retail(map, Sha256::of(base));
    manifest.package.name = name.into();
    manifest.patches.push(content::manifest::Layer {
        file: "a.ips".into(),
        sha256: Sha256::of(&patch),
    });
    std::fs::write(dir.join("manifest.toml"), manifest.to_toml()).unwrap();
    manifest.map_id()
}

#[test]
fn legacy_choices_migrate_to_independent_disc_and_map_pack_mods() {
    use crate::map_select::{self, Category};

    let t = Temp::new("legacy");
    let data = &t.0;
    let mut base = vec![0; 0x801];
    base[..4].copy_from_slice(b"daeh");
    base[0x198..0x19d].copy_from_slice(b"level");
    let iso = {
        // The source disc fixture fits one sector.
        let mut disc_bytes = base.clone();
        disc_bytes.truncate(0x800);
        crate::map_override::tests::iso_at(data, &disc_bytes)
    };
    let disc_base = &base[..0x800];
    let pack_offer = vfs::offer_id(HALO2_TITLE, 0x101);
    let pack_dir = vfs::Config::new(data, HALO2_TITLE).content_dir(pack_offer);
    std::fs::create_dir_all(&pack_dir).unwrap();
    std::fs::write(pack_dir.join("turf.map"), &base).unwrap();
    let pack = Entry {
        scenario: "scenarios\\multi\\turf\\turf".into(),
        map_id: 0x3e8,
        names: std::array::from_fn(|_| "Turf".into()),
        descriptions: Default::default(),
        sort: 100,
        flags: 3,
        teams: [2; 16],
        preview: vec![0x56; PREVIEW_LEN],
    };
    let pack_record = pack.to_v2().unwrap();
    std::fs::write(pack_dir.join("patch.lvl"), &pack_record).unwrap();

    let snow = retail_package(data, "a-snow", "Level Snow", "level", disc_base, 9);
    let rockets = retail_package(data, "b-rockets", "Level Rockets", "level", disc_base, 7);
    let turf = retail_package(data, "c-turf", "Turf Night", "turf", &base, 5);
    let active = data.join("maps/active.toml");
    let legacy = "level = \"a-snow\"\nturf = false\n";
    std::fs::write(&active, legacy).unwrap();

    let mut d = disc();
    d.slots[0] = slot(1, "level", 3, [4; 16], 10);
    let retail = &mut d.slots[0];
    let utf16 = "Level"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    retail[0xc..0xc + utf16.len()].copy_from_slice(&utf16);
    d.base_files.insert(
        "level".into(),
        vfs::HostFile {
            path: iso.clone(),
            offset: 37 * 2048,
            len: 0x800,
        },
    );
    let before = d.slots.clone();

    let report = install(data, &mut d).unwrap();
    assert_eq!(report.installed.len(), 3, "{report:?}");
    assert_eq!(report.overflow, 0);
    assert_eq!(std::fs::read_to_string(&active).unwrap(), legacy);
    assert_eq!(d.slots, before);
    assert_eq!(
        std::fs::read(pack_dir.join("patch.lvl")).unwrap(),
        pack_record
    );
    let cat = map_select::read(&d.slots, &content_root(data));
    assert_eq!(
        cat.maps(Category::Halo2)
            .iter()
            .find(|map| map.id == 1)
            .unwrap()
            .name,
        "Level"
    );
    assert_eq!(cat.maps(Category::MapPack)[0].name, "Turf");
    assert_eq!(cat.maps(Category::MapPack)[0].id, 0x3e8);
    assert_eq!(
        cat.maps(Category::Mods)
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        ["Level Rockets", "Level Snow", "Turf Night"]
    );

    let cfg = vfs::Config::new(data, HALO2_TITLE);
    cfg.content_maps.replace(report.mounts.clone());
    let mut vfs = crate::map_override::mount(
        cfg.clone(),
        &iso,
        None::<crate::map_override::tests::Upper>,
        None,
        &[],
    )
    .unwrap();
    let disc_path = format!("{}\\maps\\level.map", vfs::CDROM_DEVICE);
    let pack_path = device_path(pack_offer, "turf");
    for (name, offer, id) in &report.installed {
        assert_ne!(*id, 1);
        assert_ne!(*id, 0x3e8);
        let path = device_path(*offer, name);
        let id = vfs.map_id(&path).unwrap();
        let bytes = read(&mut vfs, &path);
        let value = if id == snow {
            9
        } else if id == rockets {
            7
        } else {
            assert_eq!(id, turf);
            5
        };
        assert_eq!(bytes[0x800], value);
        assert!(bytes[0x198..0x1b8].starts_with(name.as_bytes()));
        assert_eq!(crate::map_override::map_id(&mut vfs, name), Some(id));
        assert_eq!(
            cfg.content_dir(*offer)
                .join(format!("{name}.map"))
                .metadata()
                .unwrap()
                .len(),
            0
        );
        let lvl = std::fs::read(cfg.content_dir(*offer).join("patch.lvl")).unwrap();
        assert_eq!(
            Entry::from_v2(&lvl).unwrap().preview,
            if id == turf {
                pack.preview.clone()
            } else {
                d.preview.clone().unwrap()
            }
        );
    }
    assert_eq!(read(&mut vfs, &disc_path), disc_base);
    assert_eq!(read(&mut vfs, &pack_path), base);
    assert_eq!(vfs.map_id(&disc_path), None);
    assert_eq!(vfs.map_id(&pack_path), None);
    assert!(
        !data.join("maps/cache").exists(),
        "an IPS mod streams without a patched file"
    );
}

#[test]
fn slot_plan_preserves_retail_and_uses_the_mods_own_preview() {
    let t = Temp::new("slot-preview");
    let data = &t.0;
    let mut d = disc();
    let base = b"alpha";
    let source = data.join("alpha.map");
    std::fs::write(&source, base).unwrap();
    d.base_files.insert(
        "alpha".into(),
        vfs::HostFile {
            path: source,
            offset: 0,
            len: base.len() as u64,
        },
    );
    let id = retail_package(data, "snow", "Alpha Snow", "alpha", base, 10);
    let dir = data.join("maps/snow");
    let manifest = dir.join("manifest.toml");
    let mut m = content::Manifest::from_toml(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
    m.package.image = Some("snow.png".into());
    std::fs::write(&manifest, m.to_toml()).unwrap();
    let mut png = png::Encoder::new(File::create(dir.join("snow.png")).unwrap(), 2, 2);
    png.set_color(png::ColorType::Rgb);
    png.set_depth(png::BitDepth::Eight);
    png.write_header()
        .unwrap()
        .write_image_data(&[10, 20, 30].repeat(4))
        .unwrap();

    let slots = d.slots.clone();
    let mut simulated_slots: Vec<_> = slots
        .iter()
        .map(|slot| u32::from_le_bytes(slot[..4].try_into().unwrap()))
        .collect();
    let report = install(data, &mut d).unwrap();
    let (_, offer, list_id) = report.installed[0];
    assert_eq!(slot_plan(&slots, &[], &[id]).0, [list_id]);
    assert!(!simulated_slots.contains(&list_id));
    let first_free = simulated_slots
        .iter()
        .position(|&id| id == u32::MAX)
        .unwrap();
    simulated_slots[first_free] = list_id;
    assert_eq!(&simulated_slots[..first_free], [1, 2]);
    assert_eq!(d.slots, slots);
    let record = std::fs::read(
        vfs::Config::new(data, HALO2_TITLE)
            .content_dir(offer)
            .join("patch.lvl"),
    )
    .unwrap();
    let mod_slot = Entry::from_v2(&record).unwrap();
    assert_eq!(mod_slot.map_id, list_id);
    assert_eq!(mod_slot.names[0], "Alpha Snow");
    assert_eq!(&mod_slot.preview[(8 * 220 + 8) * 4..][..3], [30, 20, 10]);
    assert_eq!(&d.preview.as_ref().unwrap()[..3], [0x42; 3]);
}

#[test]
fn slot_overflow_keeps_all_sources_and_only_registers_the_offers_that_fit() {
    let t = Temp::new("overflow");
    let data = &t.0;
    let mut d = disc();
    d.slots.truncate(3);
    for name in ["one", "two", "three"] {
        package(data, name, name, &body(3), "");
    }
    let r = install(data, &mut d).unwrap();
    assert_eq!(r.installed.len(), 1);
    assert_eq!(r.overflow, 2);
    assert_eq!(dirs(data).len(), 1);
    assert_eq!(Library::open(data.join("maps")).packages().len(), 3);
    assert!(r.line().unwrap().contains("2 mods not shown, list full"));
    let kept = Library::open(data.join("maps")).packages()[0].dir.clone();
    std::fs::remove_dir_all(kept).unwrap();
    let next = install(data, &mut d).unwrap();
    assert_eq!(next.installed.len(), 1);
    assert_eq!(next.overflow, 1);
    assert_ne!(next.installed, r.installed);
}
