//! Details files, read and applied to an embedded server. Every byte is
//! invented.

use super::*;
use crate::live_accounts::{self, LocalAccount};
use crate::live_cmd::pick_account;
use crate::live_maps::publish;
use content::Manifest;
use content::manifest::{Layer, MANIFEST_FILE};
use live_proto::maps::Contents;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\ninvented shot";
const JPEG: &[u8] = b"\xff\xd8\xffinvented pack art";

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "h2-live-details-{tag}-{}-{}",
        std::process::id(),
        live_proto::sign::random_id()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn file(id: &str) -> String {
    format!(
        r#"
[[pack]]
id = "ai-revision"
name = "Halo 2 Remapped AI Revision"
authors = ["Zone 117"]
released = "2007-06"
description = "Custom maps with AI."
[[pack.image]]
file = "menu.jpg"
caption = "the pack's menu"
image_url = "https://example.invalid/menu.jpg"

[[mod]]
map_id = "{id}"
name = "Zombiebar"
slug = "zombiebar"
pack = "ai-revision"
description = """
Zanzibar overrun.
Two lines."""
features = ["scripts", "ai"]
description_sources = ["https://example.invalid/post"]
[[mod.image]]
file = "01.png"
caption = "the bar at night"
credit = "Badsmiley"
source_url = "https://example.invalid/post"
"#
    )
}

#[test]
fn details_files_parse_and_are_checked() {
    let id = "ab".repeat(32);
    let f = DetailsFile::parse(&file(&id)).unwrap();
    assert_eq!(f.packs[0].id, "ai-revision");
    assert_eq!(f.packs[0].images[0].file, "menu.jpg");
    assert_eq!(f.mods[0].slug.as_deref(), Some("zombiebar"));
    assert_eq!(f.mods[0].images[0].credit.as_deref(), Some("Badsmiley"));
    assert!(DetailsFile::parse("").unwrap().mods.is_empty());

    let twice = format!("[[mod]]\nmap_id = \"{id}\"\n[[mod]]\nmap_id = \"{id}\"\n");
    assert!(format!("{:#}", DetailsFile::parse(&twice).unwrap_err()).contains("twice"));
    assert!(DetailsFile::parse("[[mod]]\nmap_id = \"nope\"\n").is_err());
    assert!(DetailsFile::parse("[[pack]]\nid = \"AI Revision\"\nname = \"x\"\n").is_err());
    let no_slug = format!("[[mod]]\nmap_id = \"{id}\"\n[[mod.image]]\nfile = \"a.png\"\n");
    assert!(DetailsFile::parse(&no_slug).is_err(), "images need a slug");
    let bad_slug = format!("[[mod]]\nmap_id = \"{id}\"\nslug = \"../up\"\n");
    assert!(DetailsFile::parse(&bad_slug).is_err(), "a slug is no path");
    assert!(slug_ok("ppfnoobs-oldv2-burial_mounds"));
}

#[test]
fn the_published_mods_details_file_reads() {
    let f = DetailsFile::parse(include_str!("../../../../docs/game/mod-details.toml")).unwrap();
    assert!(f.mods.len() >= 80, "{}", f.mods.len());
    let packs: Vec<&str> = f.packs.iter().map(|p| p.id.as_str()).collect();
    for m in &f.mods {
        if let Some(p) = &m.pack {
            assert!(packs.contains(&p.as_str()), "{}: no pack {p}", m.map_id);
        }
        assert!(m.slug.is_some() && m.description.is_some(), "{}", m.map_id);
    }
    // Every text is within what the server takes (images aside, which
    // are read from disk when applied).
    for p in &f.packs {
        PackInfo {
            name: p.name.clone(),
            description: p.description.clone(),
            authors: p.authors.clone(),
            released: p.released.clone(),
            source_url: p.source_url.clone(),
            images: Vec::new(),
        }
        .tidy()
        .validate()
        .unwrap_or_else(|e| panic!("pack {}: {e}", p.id));
    }
    for m in &f.mods {
        Details {
            description: m.description.clone(),
            pack: m.pack.clone(),
            ..Details::default()
        }
        .tidy()
        .validate()
        .unwrap_or_else(|e| panic!("{}: {e}", m.map_id));
    }
}

#[test]
fn wanted_reads_images_and_reports_from_their_directories() {
    let id = "ab".repeat(32);
    let dir = temp("wanted");
    let images = dir.join("images");
    let reports = dir.join("contents");
    std::fs::create_dir_all(images.join("zombiebar")).unwrap();
    std::fs::create_dir_all(images.join("pack-ai-revision")).unwrap();
    std::fs::create_dir_all(&reports).unwrap();
    std::fs::write(images.join("zombiebar").join("01.png"), PNG).unwrap();
    std::fs::write(images.join("pack-ai-revision").join("menu.jpg"), JPEG).unwrap();
    let report = Analysis {
        map_id: Some(id.clone()),
        base: Some("zanzibar".into()),
        features: vec!["ai".into()],
        contents: Contents {
            lines: vec!["AI characters: 2 new".into()],
            ..Contents::default()
        },
    };
    std::fs::write(
        reports.join("zombiebar.toml"),
        toml::to_string(&report).unwrap(),
    )
    .unwrap();

    let f = DetailsFile::parse(&file(&id)).unwrap();
    let (packs, mods) = wanted(&f, Some(&images), Some(&reports)).unwrap();
    assert_eq!(packs[0].info.images[0].sha256, Sha256::of(JPEG));
    assert_eq!(packs[0].info.authors, ["Zone 117"]);
    let d = &mods[0].details;
    assert_eq!(
        d.description.as_deref(),
        Some("Zanzibar overrun.\nTwo lines.")
    );
    assert_eq!(d.images[0].sha256, Sha256::of(PNG));
    assert_eq!(d.images[0].caption.as_deref(), Some("the bar at night"));
    assert_eq!(d.contents.as_ref().unwrap().lines, ["AI characters: 2 new"]);
    assert_eq!(
        d.features.as_deref(),
        Some(&["scripts".to_string(), "ai".to_string()][..])
    );
    assert_eq!(d.pack.as_deref(), Some("ai-revision"));
    assert_eq!(mods[0].shots[0].bytes, PNG);

    // No report: none sent. No images directory: an error.
    let (_, mods) = wanted(&f, Some(&images), None).unwrap();
    assert!(mods[0].details.contents.is_none());
    assert!(wanted(&f, None, None).is_err());
    // A report for another package, or a file that is no image: errors.
    let other = Analysis {
        map_id: Some("cd".repeat(32)),
        ..report.clone()
    };
    std::fs::write(
        reports.join("zombiebar.toml"),
        toml::to_string(&other).unwrap(),
    )
    .unwrap();
    assert!(wanted(&f, Some(&images), Some(&reports)).is_err());
    std::fs::write(images.join("zombiebar").join("01.png"), b"GIF89a").unwrap();
    let err = wanted(&f, Some(&images), None).unwrap_err();
    assert!(format!("{err:#}").contains("not a PNG or JPEG"), "{err:#}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_admin_sets_details_and_packs_from_a_file() {
    let dir = temp("apply");
    let server = live_server::start_embedded(
        live_server::MEMORY_DB,
        "127.0.0.1:0".parse().unwrap(),
        &dir.join("server"),
        Default::default(),
    )
    .unwrap();
    let client = Client::at(&server.url).unwrap();
    let (created, key) = client.create_account("Importer", None).unwrap();
    let accounts = dir.join("accounts.json");
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
    let pkg = dir.join("bar");
    std::fs::create_dir_all(&pkg).unwrap();
    let patch = b"PATCH invented bar".to_vec();
    std::fs::write(pkg.join("bar.ips"), &patch).unwrap();
    let mut m = Manifest::retail("zanzibar", Sha256([0x22; 32]));
    m.package.name = "Zombiebar".into();
    m.patches = vec![Layer {
        file: "bar.ips".into(),
        sha256: Sha256::of(&patch),
    }];
    std::fs::write(pkg.join(MANIFEST_FILE), m.to_toml()).unwrap();
    let id = publish(&client, &accounts, None, &pkg, None, None)
        .unwrap()
        .map
        .summary
        .map_id;

    let images = dir.join("images");
    std::fs::create_dir_all(images.join("zombiebar")).unwrap();
    std::fs::create_dir_all(images.join("pack-ai-revision")).unwrap();
    std::fs::write(images.join("zombiebar").join("01.png"), PNG).unwrap();
    std::fs::write(images.join("pack-ai-revision").join("menu.jpg"), JPEG).unwrap();
    let f = DetailsFile::parse(&file(&id.to_string())).unwrap();
    let (packs, mods) = wanted(&f, Some(&images), None).unwrap();
    let op = Signer::Operator(
        crate::live_cmd::operator(Some(dir.join("server").display().to_string()), &client).unwrap(),
    );
    let me = Signer::Account(pick_account(&client, &accounts, None).unwrap());

    // The operator alone cannot upload the images.
    assert!(apply_details(&client, &op, None, &packs, &mods, false).is_err());
    let said = apply_details(&client, &op, Some(&me), &packs, &mods, true).unwrap();
    let all = said.join("\n");
    assert!(
        all.contains("would upload") && all.contains("pack ai-revision: would set"),
        "{all}"
    );
    assert!(
        all.contains(
            "Zombiebar: would set a description, 1 images, 0 report lines, pack ai-revision"
        ),
        "{all}"
    );
    assert!(
        list(&client, None, None).unwrap()[0].images.is_empty(),
        "a dry run sends nothing"
    );

    let said = apply_details(&client, &op, Some(&me), &packs, &mods, false).unwrap();
    let all = said.join("\n");
    assert!(
        all.contains("uploaded") && all.contains("pack ai-revision: set"),
        "{all}"
    );
    let listed = &list(&client, None, None).unwrap()[0];
    assert_eq!(
        listed.description.as_deref(),
        Some("Zanzibar overrun.\nTwo lines.")
    );
    assert_eq!(listed.images[0].sha256, Sha256::of(PNG));
    assert_eq!(listed.pack.as_deref(), Some("ai-revision"));
    assert_eq!(
        listed.pack_name.as_deref(),
        Some("Halo 2 Remapped AI Revision")
    );
    assert_eq!(listed.features, ["scripts", "ai"]);
    assert_eq!(listed.author.gamertag, "Importer", "the publisher stays");

    // Again: nothing to do.
    let said = apply_details(&client, &op, None, &packs, &mods, false).unwrap();
    assert!(said.iter().all(|l| l.ends_with("already so")), "{said:?}");

    // A plain account is no admin; a wrong name sends nothing.
    let mut other = mods.clone();
    other[0].details.description = Some("Changed.".into());
    assert!(apply_details(&client, &me, Some(&me), &[], &other, false).is_err());
    other[0].name = Some("Not Zombiebar".into());
    assert!(apply_details(&client, &op, Some(&me), &[], &other, false).is_err());
    assert_eq!(
        list(&client, None, None).unwrap()[0].description.as_deref(),
        Some("Zanzibar overrun.\nTwo lines.")
    );

    // `analyze --map-id` reads the published layer and its base.
    let (base, layer) = published_layer(&client, &id).unwrap();
    assert_eq!((base.as_str(), &layer), ("zanzibar", &patch));
    assert!(published_layer(&client, &MapId([7; 32])).is_err());
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}
