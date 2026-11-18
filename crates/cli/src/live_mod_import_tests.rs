//! `h2 live maps import` on synthetic maps: a "disc" directory whose maps
//! are a `daeh` tag and pseudo-random filler. No real map bytes.
use super::*;

/// Pseudo-random bytes (xorshift), so maps differ and do not compress
/// into each other.
fn filler(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn synthetic_map(seed: u64, len: usize) -> Vec<u8> {
    let mut m = filler(seed, len);
    m[..4].copy_from_slice(b"daeh");
    m
}

struct Fixture {
    root: PathBuf,
    disc: PathBuf,
    lockout: Vec<u8>,
    midship: Vec<u8>,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "h2-mod-import-{tag}-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        let disc = root.join("disc");
        std::fs::create_dir_all(disc.join("maps")).unwrap();
        let lockout = synthetic_map(1, 64 * 1024);
        let midship = synthetic_map(2, 48 * 1024);
        std::fs::write(disc.join("maps").join("lockout.map"), &lockout).unwrap();
        std::fs::write(disc.join("maps").join("midship.map"), &midship).unwrap();
        std::fs::write(disc.join("default.xbe"), b"XBEH invented").unwrap();
        Self {
            root,
            disc,
            lockout,
            midship,
        }
    }

    /// A mod directory holding `file` with `bytes`, and `mod_toml` if given.
    fn mod_dir(&self, name: &str, file: &str, bytes: &[u8], mod_toml: Option<&str>) -> PathBuf {
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(file), bytes).unwrap();
        if let Some(t) = mod_toml {
            std::fs::write(dir.join(MOD_FILE), t).unwrap();
        }
        dir.join(file)
    }

    fn opts(&self, input: PathBuf) -> ImportOptions {
        ImportOptions {
            input,
            disc: self.disc.clone(),
            ..ImportOptions::default()
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Lockout with some bytes changed and a block appended: not the same
/// length, so a BPS.
fn grown_mod(base: &[u8]) -> Vec<u8> {
    let mut m = base.to_vec();
    m[0x800..0x840].fill(0x5a);
    m[0x9000..0x9010].copy_from_slice(&[7; 16]);
    m.extend(filler(9, 4096));
    m
}

#[test]
fn a_whole_map_becomes_a_delta_over_the_disc_map() {
    let f = Fixture::new("map");
    let modded = grown_mod(&f.lockout);
    let input = f.mod_dir(
        "snipers",
        "lockout_snipers.map",
        &modded,
        Some(
            r#"
name = "Lockout Snipers"
author = "An Old Modder"
description = "Snipers on every rack."
source_url = "https://example.invalid/mods/42"
base_map = "lockout"
category = "weapons"
unknown_key = "ignored"
"#,
        ),
    );
    let out = import(&f.opts(input.clone())).unwrap();
    assert_eq!(
        out.dir,
        input.parent().unwrap().join("lockout_snipers.package")
    );
    let m = &out.manifest;
    assert_eq!(m.package.name, "Lockout Snipers");
    assert_eq!(m.package.author.as_deref(), Some("An Old Modder"));
    assert_eq!(
        m.package.description.as_deref(),
        Some("Snipers on every rack.")
    );
    assert_eq!(m.base.map, "lockout");
    assert_eq!(
        m.base.sha256,
        Some(Sha256::of(&f.lockout)),
        "the disc's own map"
    );
    assert!(!m.base.derived);
    assert_eq!(m.patches.len(), 1);
    assert_eq!(m.patches[0].file, "mod.bps", "the length changed");

    // The only blob is the delta: small, not the map, and it gives the mod
    // back from the disc's map.
    let layer = std::fs::read(out.dir.join("mod.bps")).unwrap();
    assert_eq!(Sha256::of(&layer), m.patches[0].sha256);
    assert!(layer.starts_with(b"BPS1"));
    assert!(!live_proto::maps::is_whole_map(&layer));
    assert!(
        layer.len() < modded.len() / 4,
        "{} bytes for a {} byte map",
        layer.len(),
        modded.len()
    );
    assert_eq!(apply(&f.lockout, &layer).unwrap(), modded);
    assert_eq!(out.layer_size, layer.len() as u64);

    // The package reads back as the runtime reads it, and keeps the
    // repository's extras for publish.
    let text = std::fs::read_to_string(out.dir.join(MANIFEST_FILE)).unwrap();
    assert_eq!(&Manifest::from_toml(&text).unwrap(), m);
    let (category, source) = extras_in(&out.dir).unwrap();
    assert_eq!(category.as_deref(), Some("weapons"));
    assert_eq!(source.as_deref(), Some("https://example.invalid/mods/42"));
    assert!(
        std::fs::read_dir(&out.dir)
            .unwrap()
            .flatten()
            .all(|e| std::fs::read(e.path()).unwrap() != modded),
        "the modded map itself is not in the package"
    );
}

#[test]
fn a_same_length_map_becomes_a_ppf_and_flags_win_over_the_sidecar() {
    let f = Fixture::new("ppf");
    let mut modded = f.lockout.clone();
    modded[0x1000..0x1100].fill(1);
    let input = f.mod_dir(
        "same",
        "whatever.bin",
        &modded,
        Some("name = \"From toml\"\nbase_map = \"midship\"\ncategory = \"other\"\n"),
    );
    let out_dir = f.root.join("out");
    let out = import(&ImportOptions {
        base: Some("Lockout".into()),
        out: Some(out_dir.clone()),
        name: Some("From flag".into()),
        category: Some("gameplay".into()),
        ..f.opts(input)
    })
    .unwrap();
    assert_eq!(out.dir, out_dir);
    assert_eq!(out.manifest.package.name, "From flag");
    assert_eq!(out.manifest.base.map, "lockout");
    assert_eq!(out.manifest.patches[0].file, "mod.ppf");
    let layer = std::fs::read(out_dir.join("mod.ppf")).unwrap();
    assert_eq!(apply(&f.lockout, &layer).unwrap(), modded);
    assert_eq!(extras_in(&out_dir).unwrap().0.as_deref(), Some("gameplay"));
}

#[test]
fn patches_are_checked_against_the_disc_and_kept() {
    let f = Fixture::new("patch");
    // An IPS that writes three bytes at 0x10.
    let ips = b"PATCH\x00\x00\x10\x00\x03abcEOF".to_vec();
    let input = f.mod_dir("ips", "mod.ips", &ips, None);
    let out = import(&ImportOptions {
        base: Some("lockout".into()),
        ..f.opts(input)
    })
    .unwrap();
    assert_eq!(out.manifest.patches[0].file, "mod.ips");
    assert_eq!(std::fs::read(out.dir.join("mod.ips")).unwrap(), ips);
    assert_eq!(out.manifest.package.name, "mod", "the file's stem");

    // A BPS made against the disc's map is kept as it is.
    let target = grown_mod(&f.lockout);
    let bps = content::patch::write::bps(&f.lockout, &target);
    let input = f.mod_dir("bps", "fog.bps", &bps, Some("base_map = \"lockout\"\n"));
    let out = import(&f.opts(input)).unwrap();
    assert_eq!(std::fs::read(out.dir.join("mod.bps")).unwrap(), bps);

    // One made against another map is refused: wrong base.
    let other = content::patch::write::bps(&f.midship, &grown_mod(&f.midship));
    let input = f.mod_dir(
        "wrong",
        "wrong.bps",
        &other,
        Some("base_map = \"lockout\"\n"),
    );
    let err = format!("{:#}", import(&f.opts(input)).unwrap_err());
    assert!(err.contains("does not apply"), "{err}");
}

#[test]
fn nothing_retail_and_nothing_empty_is_shared() {
    let f = Fixture::new("refuse");
    let base = |s: &str| ImportOptions {
        base: Some(s.into()),
        ..ImportOptions::default()
    };
    let run = |input: PathBuf, o: ImportOptions| {
        format!(
            "{:#}",
            import(&ImportOptions {
                input,
                disc: f.disc.clone(),
                ..o
            })
            .unwrap_err()
        )
    };

    // The disc's own map, unchanged.
    let same = f.mod_dir("same", "lockout.map", &f.lockout, None);
    assert!(run(same, base("lockout")).contains("nothing to share"));
    // Another retail map passed off as a mod of this one: its bytes would
    // be in the delta.
    let twin = f.mod_dir("twin", "midship.map", &f.midship, None);
    let err = run(twin, base("lockout"));
    assert!(err.contains("retail maps are never shared"), "{err}");
    // No base given, a base the disc lacks, a name that is not a map's.
    let modded = f.mod_dir("m", "m.map", &grown_mod(&f.lockout), None);
    assert!(run(modded.clone(), ImportOptions::default()).contains("--base"));
    assert!(run(modded.clone(), base("zanzibar")).contains("zanzibar"));
    assert!(run(modded, base("../lockout")).contains("not a map name"));
    // Not a map or a patch.
    let junk = f.mod_dir("junk", "junk.zip", b"PK\x03\x04", None);
    assert!(run(junk, base("lockout")).contains("not a map, PPF, IPS or BPS"));
    // A bad category or source in the sidecar.
    let bad = f.mod_dir(
        "bad",
        "bad.map",
        &grown_mod(&f.lockout),
        Some("base_map = \"lockout\"\ncategory = \"cheats\"\n"),
    );
    assert!(run(bad, ImportOptions::default()).contains("category"));
}

#[test]
fn input_kinds_by_name_and_signature() {
    assert_eq!(InputKind::named(" MAP "), Some(InputKind::Map));
    assert_eq!(InputKind::named("bps"), Some(InputKind::Bps));
    assert_eq!(InputKind::named("zip"), None);
    assert_eq!(InputKind::sniff(b"daeh\x08"), Some(InputKind::Map));
    assert_eq!(InputKind::sniff(b"PPF30"), Some(InputKind::Ppf));
    assert_eq!(InputKind::sniff(b"PATCHxx"), Some(InputKind::Ips));
    assert_eq!(InputKind::sniff(b"BPS1"), Some(InputKind::Bps));
    assert_eq!(InputKind::sniff(b"PK"), None);
}

#[test]
fn an_imported_mod_publishes_only_its_delta() {
    use crate::live_accounts::{self, LocalAccount};
    use crate::live_cmd::Client;

    let f = Fixture::new("publish");
    let modded = grown_mod(&f.lockout);
    let input = f.mod_dir(
        "pub",
        "p.map",
        &modded,
        Some(
            "name = \"Lockout Remix\"\nauthor = \"Someone Else\"\nbase_map = \"lockout\"\ncategory = \"maps\"\nsource_url = \"https://example.invalid/r\"\n",
        ),
    );
    let out = import(&f.opts(input)).unwrap();

    let server = live_server::start_embedded(
        live_server::MEMORY_DB,
        "127.0.0.1:0".parse().unwrap(),
        &f.root.join("server"),
        Default::default(),
    )
    .unwrap();
    let client = Client::at(&server.url).unwrap();
    let (created, key) = client.create_account("Importer", None).unwrap();
    let accounts = f.root.join("accounts.json");
    let mut list_of = Vec::new();
    live_accounts::upsert(
        &mut list_of,
        LocalAccount::new(
            created.xuid,
            created.gamertag.as_str(),
            &client.base,
            &client.discovery,
            &key,
        ),
    );
    live_accounts::save(&accounts, &list_of).unwrap();

    let answer = crate::live_maps::publish(&client, &accounts, None, &out.dir, None, None).unwrap();
    let s = &answer.map.summary;
    assert_eq!(s.name, "Lockout Remix");
    assert_eq!(s.credit.as_deref(), Some("Someone Else"));
    assert_eq!(s.author.gamertag, "Importer");
    assert_eq!(s.category.as_deref(), Some("maps"));
    assert_eq!(s.source_url.as_deref(), Some("https://example.invalid/r"));
    assert_eq!(s.size, out.layer_size, "only the delta was uploaded");
    assert!(s.size < modded.len() as u64 / 4);
    let listed = crate::live_maps::list(&client, Some("lockout".into()), None).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].category.as_deref(), Some("maps"));
    drop(server);
}

/// One synthetic XDVDFS directory table: entries chained down the
/// right-hand side, as `mappack_cmd`'s tests build them.
fn iso_table(items: &[(&str, u32, u32, bool)]) -> Vec<u8> {
    let mut recs: Vec<Vec<u8>> = Vec::new();
    for &(name, sector, size, dir) in items {
        let mut v = vec![0u8; 4];
        v.extend(sector.to_le_bytes());
        v.extend(size.to_le_bytes());
        v.push(if dir { 0x10 } else { 0 });
        v.push(name.len() as u8);
        v.extend(name.as_bytes());
        while !v.len().is_multiple_of(4) {
            v.push(0xFF);
        }
        recs.push(v);
    }
    let mut out = Vec::new();
    let mut offs = Vec::new();
    for r in &recs {
        offs.push(out.len());
        out.extend(r);
    }
    for i in 0..recs.len().saturating_sub(1) {
        let right = (offs[i + 1] / 4) as u16;
        out[offs[i] + 2..offs[i] + 4].copy_from_slice(&right.to_le_bytes());
    }
    out.resize(2048, 0xFF);
    out
}

/// A map pack shaped disc image: `/demos/dlc/00000001/` holding the given
/// maps (name, bytes), each 32 sectors apart from sector 40. No real bytes.
fn map_pack_iso(maps: &[(&str, &[u8])]) -> Vec<u8> {
    const S: usize = 2048;
    let s = S as u32;
    let mut img = vec![0u8; (40 + 32 * maps.len()) * S];
    let vd = 32 * S;
    img[vd..vd + 20].copy_from_slice(b"MICROSOFT*XBOX*MEDIA");
    img[vd + 20..vd + 24].copy_from_slice(&33u32.to_le_bytes());
    img[vd + 24..vd + 28].copy_from_slice(&s.to_le_bytes());
    let files: Vec<(&str, u32, u32, bool)> = maps
        .iter()
        .enumerate()
        .map(|(i, (name, bytes))| (*name, 40 + 32 * i as u32, bytes.len() as u32, false))
        .collect();
    let dirs = [
        (33, iso_table(&[("demos", 34, s, true)])),
        (34, iso_table(&[("dlc", 35, s, true)])),
        (35, iso_table(&[("00000001", 36, s, true)])),
        (36, iso_table(&files)),
    ];
    for (sector, t) in dirs {
        img[sector * S..(sector + 1) * S].copy_from_slice(&t);
    }
    for (i, (_, bytes)) in maps.iter().enumerate() {
        assert!(bytes.len() <= 32 * S);
        let at = (40 + 32 * i) * S;
        img[at..at + bytes.len()].copy_from_slice(bytes);
    }
    img
}

/// The fixture plus a map pack disc with containment and warlock.
struct PackFixture {
    f: Fixture,
    iso: PathBuf,
    containment: Vec<u8>,
    warlock: Vec<u8>,
}

impl PackFixture {
    fn new(tag: &str) -> Self {
        let f = Fixture::new(tag);
        let containment = synthetic_map(3, 64 * 1024);
        let warlock = synthetic_map(4, 48 * 1024);
        let iso = f.root.join("mappack.iso");
        let img = map_pack_iso(&[("containment.map", &containment), ("Warlock.map", &warlock)]);
        std::fs::write(&iso, img).unwrap();
        Self {
            f,
            iso,
            containment,
            warlock,
        }
    }

    fn opts(&self, input: PathBuf, base: &str) -> ImportOptions {
        ImportOptions {
            mappack: Some(self.iso.clone()),
            base: Some(base.into()),
            ..self.f.opts(input)
        }
    }
}

#[test]
fn a_map_pack_mod_becomes_a_delta_over_the_map_pack_map() {
    let p = PackFixture::new("pack");
    let modded = grown_mod(&p.containment);
    let input = p.f.mod_dir(
        "ai",
        "ai_war.map",
        &modded,
        Some("name = \"AI War\"\nbase_map = \"Containment\"\n"),
    );
    let out = import(&ImportOptions {
        mappack: Some(p.iso.clone()),
        ..p.f.opts(input)
    })
    .unwrap();
    let m = &out.manifest;
    assert_eq!(out.base_from, BaseFrom::MapPack);
    // Named as the game names it, checked by the map pack map's hash.
    assert_eq!(m.base.map, "containment");
    assert_eq!(m.base.sha256, Some(Sha256::of(&p.containment)));
    assert!(!m.base.derived);
    assert_eq!(m.patches.len(), 1);
    let layer = std::fs::read(out.dir.join(&m.patches[0].file)).unwrap();
    assert!(!live_proto::maps::is_whole_map(&layer));
    assert!(layer.len() < modded.len() / 4);
    assert_eq!(apply(&p.containment, &layer).unwrap(), modded);
    let text = std::fs::read_to_string(out.dir.join(MANIFEST_FILE)).unwrap();
    assert_eq!(&Manifest::from_toml(&text).unwrap(), m);
    assert!(
        std::fs::read_dir(&out.dir)
            .unwrap()
            .flatten()
            .all(|e| std::fs::read(e.path()).unwrap() != modded),
        "the modded map itself is not in the package"
    );

    // A patch made against a map pack map is checked against it and kept;
    // the map name's case on the disc does not matter.
    let mut same = p.warlock.clone();
    same[0x2000..0x2100].fill(9);
    let ppf = content::patch::diff(&p.warlock, &same, "w").bytes;
    let input = p.f.mod_dir("w", "w.ppf", &ppf, None);
    let out = import(&p.opts(input, "warlock")).unwrap();
    assert_eq!(out.base_from, BaseFrom::MapPack);
    assert_eq!(out.manifest.base.sha256, Some(Sha256::of(&p.warlock)));
    assert_eq!(std::fs::read(out.dir.join("mod.ppf")).unwrap(), ppf);

    // A Halo 2 disc map is still read from the disc with the map pack given.
    let input = p.f.mod_dir("l", "l.map", &grown_mod(&p.f.lockout), None);
    let out = import(&p.opts(input, "lockout")).unwrap();
    assert_eq!(out.base_from, BaseFrom::Disc);
    assert_eq!(out.manifest.base.sha256, Some(Sha256::of(&p.f.lockout)));
}

#[test]
fn no_retail_map_of_either_disc_is_shared() {
    let p = PackFixture::new("pack-refuse");
    let err = |o: ImportOptions| format!("{:#}", import(&o).unwrap_err());

    // A map pack map passed off as a mod of another: of one on its disc,
    // and of one on the Halo 2 disc.
    let twin = p.f.mod_dir("t1", "warlock.map", &p.warlock, None);
    let e = err(p.opts(twin.clone(), "containment"));
    assert!(e.contains("map pack's warlock.map"), "{e}");
    let e = err(p.opts(twin, "midship"));
    assert!(e.contains("retail maps are never shared"), "{e}");
    // A Halo 2 disc map passed off as a mod of a map pack map.
    let twin = p.f.mod_dir("t2", "midship.map", &p.f.midship, None);
    let e = err(p.opts(twin, "warlock"));
    assert!(e.contains("disc's own midship.map"), "{e}");
    // The map pack map unchanged.
    let same = p.f.mod_dir("t3", "c.map", &p.containment, None);
    assert!(err(p.opts(same, "containment")).contains("nothing to share"));

    // A map pack map with no map pack disc says where to look; a map on
    // neither disc says so.
    let modded = p.f.mod_dir("m", "m.map", &grown_mod(&p.containment), None);
    let e = err(ImportOptions {
        base: Some("containment".into()),
        ..p.f.opts(modded.clone())
    });
    assert!(e.contains("--mappack"), "{e}");
    let e = err(p.opts(modded, "turf"));
    assert!(e.contains("has no turf.map either"), "{e}");
}
