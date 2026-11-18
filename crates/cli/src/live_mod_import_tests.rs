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
    assert_eq!(out.dir, input.parent().unwrap().join("lockout_snipers.package"));
    let m = &out.manifest;
    assert_eq!(m.package.name, "Lockout Snipers");
    assert_eq!(m.package.author.as_deref(), Some("An Old Modder"));
    assert_eq!(m.package.description.as_deref(), Some("Snipers on every rack."));
    assert_eq!(m.base.map, "lockout");
    assert_eq!(m.base.sha256, Some(Sha256::of(&f.lockout)), "the disc's own map");
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
    let input = f.mod_dir("wrong", "wrong.bps", &other, Some("base_map = \"lockout\"\n"));
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
