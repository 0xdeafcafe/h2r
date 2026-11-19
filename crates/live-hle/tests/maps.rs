//! Joining a custom map end to end: a package published on a real server
//! (its HTTP API, in memory) is downloaded, checked and written by the
//! client's `MapGate` (docs/game/maps.md §5). Every byte is invented.
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use content::manifest::Layer;
use content::{Library, Manifest, MapId, Sha256};
use live_hle::maps::{Bases, HttpFetch, MapGate, Refusal};
use live_proto::maps::{MAP_PUBLISHED, MapDocument, package_stream};
use live_proto::session::NewSession;
use live_proto::sign::{AccountRequest, EventBody, KeyPair, issue_token};
use live_proto::lxm;

struct Disc(HashMap<String, Sha256>);
impl Bases for Disc {
    fn disc_hash(&self, map: &str) -> Option<Sha256> {
        self.0.get(map).copied()
    }
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[test]
fn a_published_package_is_fetched_over_http_and_installed() {
    let data = std::env::temp_dir().join(format!(
        "h2-hle-maps-e2e-{}-{}",
        std::process::id(),
        live_proto::sign::random_id()
    ));
    let server = live_server::start_embedded(
        live_server::MEMORY_DB,
        "127.0.0.1:0".parse().unwrap(),
        &data.join("server"),
        Default::default(),
    )
    .unwrap();
    let api = format!("{}/v1", server.url);
    let agent = ureq::AgentBuilder::new().build();
    let id_of_server = server.live.id().to_string();

    // An account publishes a Lockout mod: one patch, then the manifest.
    let key = KeyPair::generate();
    let created: serde_json::Value = serde_json::from_str(
        &agent
            .post(&format!("{api}/accounts"))
            .send_string(&AccountRequest::sign(&key, "Modder", None, &id_of_server, now()))
            .unwrap()
            .into_string()
            .unwrap(),
    )
    .unwrap();
    let author = created["id"].as_str().unwrap().to_string();
    let token = |m: &str| format!("Bearer {}", issue_token(&key, &author, &id_of_server, m, now()));
    let patch = b"PATCH invented for the e2e test".to_vec();
    let sha = Sha256::of(&patch);
    agent
        .put(&format!("{api}/content/{sha}"))
        .set("authorization", &token(lxm::CONTENT_WRITE))
        .send_bytes(&patch)
        .unwrap();
    let base = Sha256([0x42; 32]);
    let mut m = Manifest::retail("lockout", base);
    m.package.name = "Lockout Snipers".into();
    m.patches = vec![Layer {
        file: "snipers.ips".into(),
        sha256: sha,
    }];
    let id = MapId::of(&m);
    let doc = MapDocument::new(m, None);
    let jws = EventBody::new(
        &key,
        &author,
        &package_stream(&id),
        MAP_PUBLISHED,
        serde_json::to_value(&doc).unwrap(),
        time::OffsetDateTime::now_utc(),
    )
    .sign(&key);
    agent
        .put(&format!("{api}/maps"))
        .set("authorization", &token(lxm::CONTENT_WRITE))
        .send_string(&jws)
        .unwrap();

    let session = NewSession {
        addr: None,
        port: 3074,
        host_address: vec![0; 36],
        slots: Default::default(),
        attributes: BTreeMap::new(),
        flags: "custom".into(),
        hopper_id: None,
        map: Some("lockout".into()),
        name: None,
        language: None,
        visibility: Default::default(),
        skulls: None,
        campaign: None,
        ping_ms: None,
        variant_ext: None,
        map_id: Some(id),
    };
    let maps = data.join("maps");
    let fetch = HttpFetch {
        agent: &agent,
        api: &api,
        reader: None,
    };
    // Without the disc's Lockout the join is refused, with a reason.
    let none = MapGate::new(&maps, Arc::new(Disc(HashMap::new())));
    assert!(matches!(
        none.prepare(&fetch, &session),
        Err(Refusal::NoBase { .. })
    ));
    let gate = MapGate::new(
        &maps,
        Arc::new(Disc(HashMap::from([("lockout".to_string(), base)]))),
    );
    let dir = gate.prepare(&fetch, &session).unwrap().unwrap();
    assert_eq!(std::fs::read(dir.join("snipers.ips")).unwrap(), patch);
    assert_eq!(Library::open(&maps).packages()[0].id, id);

    // An admin blocks the patch: the next join is refused.
    let operator = live_server::ServerIdentity::load(&data.join("server")).unwrap().key;
    let op = issue_token(
        &operator,
        &format!("server:{id_of_server}"),
        &id_of_server,
        lxm::CONTENT_MODERATE,
        now(),
    );
    agent
        .post(&format!("{api}/admin/content/{sha}/block"))
        .set("authorization", &format!("Bearer {op}"))
        .call()
        .unwrap();
    let err = gate.prepare(&fetch, &session).unwrap_err();
    assert!(matches!(err, Refusal::TakenDown(_)), "{err}");
    drop(server);
    let _ = std::fs::remove_dir_all(&data);
}

/// The Mods screen's install counts a download (docs/live/live-api.md "The
/// mod repository"): the manifest asked for with a fresh `read` token once
/// the map is written. The server counts an account once per package, so a
/// second install leaves the count at 1; a GET with no token counts nothing.
#[test]
fn installing_a_mod_counts_one_download_per_account() {
    let data = std::env::temp_dir().join(format!(
        "h2-hle-maps-count-{}-{}",
        std::process::id(),
        live_proto::sign::random_id()
    ));
    let server = live_server::start_embedded(
        live_server::MEMORY_DB,
        "127.0.0.1:0".parse().unwrap(),
        &data.join("server"),
        Default::default(),
    )
    .unwrap();
    let api = format!("{}/v1", server.url);
    let agent = ureq::AgentBuilder::new().build();
    let server_id = server.live.id().to_string();
    let account = |tag: &str| -> (Arc<KeyPair>, String) {
        let key = Arc::new(KeyPair::generate());
        let created: serde_json::Value = serde_json::from_str(
            &agent
                .post(&format!("{api}/accounts"))
                .send_string(&AccountRequest::sign(&key, tag, None, &server_id, now()))
                .unwrap()
                .into_string()
                .unwrap(),
        )
        .unwrap();
        (key, created["id"].as_str().unwrap().to_string())
    };
    let (modder, modder_id) = account("Modder");
    let (player, player_id) = account("Player");

    // The modder publishes a Lockout mod: one patch, then the manifest.
    let write = format!(
        "Bearer {}",
        issue_token(&modder, &modder_id, &server_id, lxm::CONTENT_WRITE, now())
    );
    let patch = b"PATCH invented for the download count".to_vec();
    let sha = Sha256::of(&patch);
    agent
        .put(&format!("{api}/content/{sha}"))
        .set("authorization", &write)
        .send_bytes(&patch)
        .unwrap();
    let base = Sha256([0x42; 32]);
    let mut m = Manifest::retail("lockout", base);
    m.package.name = "Lockout Snipers".into();
    m.patches = vec![Layer {
        file: "snipers.ips".into(),
        sha256: sha,
    }];
    let id = MapId::of(&m);
    let jws = EventBody::new(
        &modder,
        &modder_id,
        &package_stream(&id),
        MAP_PUBLISHED,
        serde_json::to_value(&MapDocument {
            manifest: m,
            image: None,
            features: vec![],
            gametypes: vec![],
            version: None,
            category: None,
            source_url: None,
        })
        .unwrap(),
        time::OffsetDateTime::now_utc(),
    )
    .sign(&modder);
    // Tokens are single use: a fresh one for the manifest.
    let write = format!(
        "Bearer {}",
        issue_token(&modder, &modder_id, &server_id, lxm::CONTENT_WRITE, now())
    );
    agent
        .put(&format!("{api}/maps"))
        .set("authorization", &write)
        .send_string(&jws)
        .unwrap();
    let downloads = || -> u64 {
        let a: live_proto::maps::MapAnswer = serde_json::from_str(
            &agent
                .get(&format!("{api}/maps/{id}"))
                .call()
                .unwrap()
                .into_string()
                .unwrap(),
        )
        .unwrap();
        a.map.summary.downloads
    };
    assert_eq!(downloads(), 0, "published, and read with no token");

    let reader: live_hle::maps::Reader = {
        let (player, player_id, server_id) = (player.clone(), player_id.clone(), server_id.clone());
        Arc::new(move || Some(issue_token(&player, &player_id, &server_id, lxm::READ, now())))
    };
    let fetch = HttpFetch {
        agent: &agent,
        api: &api,
        reader: Some(reader),
    };
    let gate = MapGate::new(
        data.join("maps"),
        Arc::new(Disc(HashMap::from([("lockout".to_string(), base)]))),
    );
    let dir = gate.install(&fetch, id, &mut |_, _| {}).unwrap();
    assert_eq!(std::fs::read(dir.join("snipers.ips")).unwrap(), patch);
    assert_eq!(downloads(), 1, "installed: counted");
    assert_eq!(gate.install(&fetch, id, &mut |_, _| {}).unwrap(), dir);
    assert_eq!(downloads(), 1, "the same account again: still 1");
    drop(server);
    let _ = std::fs::remove_dir_all(&data);
}
