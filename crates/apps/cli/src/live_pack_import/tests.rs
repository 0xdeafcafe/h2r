use super::*;
use content::Sha256;
use live_proto::maps::ModPackSummary;
use serde::Deserialize;

fn args(s: &[&str]) -> Args {
    Args(s.iter().map(|v| v.to_string()).collect())
}

fn metadata_flags() -> Args {
    args(&[
        "--pack",
        "Invented release",
        "--author",
        "Example One",
        "--author",
        "Example Two",
        "--source-url",
        "https://example.invalid/release",
        "--year",
        "2006",
        "--version",
        "1.2",
        "--not-for-online",
    ])
}

#[test]
fn metadata_keeps_author_order_and_rejects_conflicting_advisories() {
    let mut flags = metadata_flags();
    let doc = metadata(&mut flags).unwrap();
    assert_eq!(doc.authors, ["Example One", "Example Two"]);
    assert_eq!(doc.year, Some(2006));
    assert_eq!(doc.advisory, Some(ModPackAdvisory::NotForOnline));
    assert!(flags.0.is_empty());

    let mut flags = metadata_flags();
    flags.0.push("--cheating-pack".into());
    assert!(metadata(&mut flags).is_err());
}

#[test]
fn patch_names_recognize_archive_layouts_and_require_one_level() {
    let names = ["dune", "lockout", "burial_mounds", "cyclotron"].map(str::to_owned);
    for (file, expected) in [
        ("Release/Changes - Burial Mounds.map.ppf", "burial_mounds"),
        ("Files/RELIC/Custom weapons.xdelta", "dune"),
        ("Ivory Tower - final.sppf", "cyclotron"),
        ("Lockout/final.PPF", "lockout"),
    ] {
        assert_eq!(
            patch_base(Path::new(file), &names).unwrap().as_deref(),
            Some(expected)
        );
    }
    assert!(patch_base(Path::new("lockout and relic.ppf"), &names).is_err());
    assert!(patch_base(Path::new("unknown.ppf"), &names).is_err());
    assert!(
        patch_base(Path::new("mainmenu.ppf"), &names)
            .unwrap()
            .is_none()
    );
}

fn cache(level: &str) -> Vec<u8> {
    let mut bytes = vec![0; 8192];
    bytes[..8].copy_from_slice(b"daeh\x08\0\0\0");
    bytes[0x140] = 1;
    bytes[0x198..0x198 + level.len()].copy_from_slice(level.as_bytes());
    bytes[0x7fc..0x800].copy_from_slice(b"toof");
    bytes
}

#[derive(Deserialize)]
struct Detail {
    pack: ModPackSummary,
}

#[test]
fn mixed_folder_publishes_credited_deltas_as_one_pack_and_retries() {
    let temp = Scratch::new().unwrap();
    let disc = temp.0.join("disc");
    let release = temp.0.join("release");
    fs::create_dir_all(disc.join("maps")).unwrap();
    fs::write(disc.join("default.xbe"), b"XBEH invented").unwrap();
    fs::create_dir_all(release.join("patched")).unwrap();
    fs::create_dir_all(release.join("Lockout")).unwrap();
    let mut targets = BTreeMap::new();
    for name in ["lockout", "midship", "unchanged"] {
        let base = cache(name);
        fs::write(disc.join("maps").join(format!("{name}.map")), &base).unwrap();
        let mut edited = base.clone();
        edited[0x1200] = 18;
        match name {
            "lockout" => {
                let patch = content::patch::write::ppf3(&base, &edited, "invented");
                fs::write(release.join("Lockout").join("patch.PPF"), patch).unwrap();
            }
            "midship" => fs::write(release.join("patched").join("midship.MAP"), &edited).unwrap(),
            _ => {
                fs::write(release.join("patched").join("unchanged.map"), base).unwrap();
                continue;
            }
        }
        targets.insert(name, (base, edited));
    }
    // Untrusted sidecars never choose a local file for publication.
    fs::write(
        release.join("Lockout").join("mod.toml"),
        "image = '/unrelated/file'",
    )
    .unwrap();
    let mut doc = metadata(&mut metadata_flags()).unwrap();
    let files = source::discover(&release, &temp.0).unwrap();
    let packages = prepare(files, &disc, None, &temp.0, &doc).unwrap();
    assert_eq!(packages.len(), 2);
    for pkg in &packages {
        let (base, target) = &targets[pkg.manifest.base.map.as_str()];
        assert_eq!(pkg.manifest.base.sha256, Some(Sha256::of(base)));
        let layer = fs::read(pkg.dir.join(&pkg.manifest.patches[0].file)).unwrap();
        assert!(!live_proto::maps::is_whole_map(&layer));
        assert_eq!(
            &crate::live_mod_import::apply(base, &layer).unwrap(),
            target
        );
        assert_eq!(
            pkg.manifest.package.author.as_deref(),
            Some("Example One & Example Two")
        );
    }
    doc.members = packages.iter().map(|p| p.manifest.map_id()).collect();
    doc.validate().unwrap();
    let service = live_server::start_embedded(
        live_server::MEMORY_DB,
        "127.0.0.1:0".parse().unwrap(),
        &temp.0.join("server"),
        Default::default(),
    )
    .unwrap();
    let client = Client::at(&service.url).unwrap();
    let (account, key) = client.create_account("Publisher", None).unwrap();
    let accounts = temp.0.join("accounts.json");
    crate::live_accounts::save(
        &accounts,
        &[crate::live_accounts::LocalAccount::new(
            account.xuid,
            account.gamertag.as_str(),
            &client.base,
            &client.discovery,
            &key,
        )],
    )
    .unwrap();
    let id = publish_pack(&client, &accounts, None, &doc, &packages).unwrap();
    assert_eq!(
        publish_pack(&client, &accounts, None, &doc, &packages).unwrap(),
        id
    );
    let admin = Signer::Operator(
        crate::live_cmd::operator(Some(temp.0.join("server").display().to_string()), &client)
            .unwrap(),
    );
    let moderate = |action| {
        let req = client
            .agent
            .post(&client.url(&format!("/admin/mod-packs/{id}/{action}")));
        let token = admin
            .token(&client, live_proto::lxm::CONTENT_MODERATE)
            .unwrap();
        call(client.authorized(req, token).unwrap(), None).unwrap();
    };
    moderate("block");
    let err = publish_pack(&client, &accounts, None, &doc, &packages).unwrap_err();
    assert_eq!(status_of(&err), Some(410));
    moderate("unblock");
    assert_eq!(
        publish_pack(&client, &accounts, None, &doc, &packages).unwrap(),
        id
    );
    let detail: Detail = client.get(&format!("/mod-packs/{id}"), &[]).unwrap();
    assert_eq!(detail.pack.map_count, 2);
    assert_eq!(detail.pack.info.authors, doc.authors);
    assert_eq!(detail.pack.info.source_url, doc.source_url);
    assert_eq!(detail.pack.advisory, doc.advisory);
    assert_eq!(detail.pack.uploader.xuid, account.xuid);
}
