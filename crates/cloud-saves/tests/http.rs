//! HttpRemote and the engine against the real server, embedded on
//! loopback with an in-memory log. Every byte is invented.

use cloud_saves::engine::Engine;
use cloud_saves::local::Local;
use cloud_saves::remote::{HttpRemote, Remote, RemoteError};
use cloud_saves::state::Places;
use cloud_saves::{Key, Sha256};
use live_proto::sign::{AccountRequest, KeyPair};
use live_proto::{CreatedAccount, Discovery};

struct Server {
    /// Kept for its lifetime: dropping it stops the server.
    _embedded: live_server::Embedded,
    api: String,
    id: String,
    dir: std::path::PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "h2-cloud-http-{name}-{}-{}",
        std::process::id(),
        live_proto::sign::random_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn server() -> Server {
    let dir = temp("server");
    let embedded = live_server::start_embedded(
        live_server::MEMORY_DB,
        "127.0.0.1:0".parse().unwrap(),
        &dir,
        Default::default(),
    )
    .unwrap();
    let d: Discovery = serde_json::from_str(
        &ureq::get(&format!("{}/.well-known/h2live.json", embedded.url))
            .call()
            .unwrap()
            .into_string()
            .unwrap(),
    )
    .unwrap();
    let api = if d.api.starts_with('/') {
        format!("{}{}", embedded.url, d.api)
    } else {
        d.api.clone()
    };
    Server {
        _embedded: embedded,
        api,
        id: d.id,
        dir,
    }
}

impl Server {
    fn account(&self, tag: &str) -> (live_proto::Xuid, KeyPair) {
        let key = KeyPair::generate();
        let body = AccountRequest::sign(&key, tag, None, &self.id, now());
        let created: CreatedAccount = serde_json::from_str(
            &ureq::post(&format!("{}/accounts", self.api))
                .set("content-type", "application/jose")
                .send_string(&body)
                .unwrap()
                .into_string()
                .unwrap(),
        )
        .unwrap();
        (created.xuid, key)
    }

    fn remote(&self, xuid: live_proto::Xuid, key: KeyPair) -> HttpRemote {
        HttpRemote::new(&self.api, &self.id, xuid, key).unwrap()
    }
}

#[test]
fn http_remote_round_trip_versions_and_owner_only() {
    let s = server();
    let (chief, key) = s.account("Chief");
    let remote = s.remote(chief, key);
    assert_eq!(remote.list().unwrap(), []);
    let k = Key::save("0123456789AB/SaveMeta.xbx");
    let item = remote.put(&k, b"Name=Chief", 0, 1234).unwrap();
    assert_eq!((item.version, item.modified), (1, 1234));
    assert_eq!(item.sha256, Some(Sha256::of(b"Name=Chief")));
    assert_eq!(remote.get(&k).unwrap(), b"Name=Chief");
    assert_eq!(remote.list().unwrap(), [item.clone()]);
    // A name that needs escaping.
    let odd = Key::save("TitleMeta (copy).xbx");
    remote.put(&odd, b"t", 0, 1).unwrap();
    assert_eq!(remote.get(&odd).unwrap(), b"t");
    match remote.put(&k, b"stale", 0, 1) {
        Err(RemoteError::Conflict(Some(current))) => assert_eq!(*current, item),
        other => panic!("{other:?}"),
    }
    let tomb = remote.delete(&k, 1).unwrap();
    assert!(tomb.deleted);
    assert!(matches!(remote.get(&k), Err(RemoteError::Refused(404, _))));
    // Someone else, with their own valid key, naming Chief's xuid.
    let (_, other_key) = s.account("Arbiter");
    let intruder = s.remote(chief, other_key);
    assert!(matches!(intruder.list(), Err(RemoteError::Refused(401 | 403, _))));
    assert!(matches!(intruder.get(&odd), Err(RemoteError::Refused(401 | 403, _))));
}

#[test]
fn two_machines_sync_through_the_server() {
    let s = server();
    let (chief, key) = s.account("Chief");
    let seed = key.secret_b64();
    let a_data = temp("a");
    let b_data = temp("b");
    let places = |d: &std::path::Path| Places::new(d, &s.id, &chief.to_string());
    let mut a = Engine::new(
        Local::new(&a_data, None),
        s.remote(chief, key),
        places(&a_data),
    )
    .unwrap();
    a.local
        .write(&Key::save("0123456789AB/profile"), b"campaign progress", 5000)
        .unwrap();
    let r = a.sync();
    assert_eq!(r.pushed, 1, "{r:?}");
    let mut b = Engine::new(
        Local::new(&b_data, None),
        s.remote(chief, KeyPair::from_secret_b64(&seed).unwrap()),
        places(&b_data),
    )
    .unwrap();
    let r = b.sync();
    assert_eq!(r.pulled, 1, "{r:?}");
    assert_eq!(
        std::fs::read(cloud_saves::local::udata_dir(&b_data).join("0123456789AB/profile")).unwrap(),
        b"campaign progress"
    );
    // The server going away mid-game queues; it is the embedded one, so
    // stop it by dropping it.
    drop(s);
    b.local
        .write(&Key::save("0123456789AB/profile"), b"more progress", 6000)
        .unwrap();
    let r = b.push(&[Key::save("0123456789AB")]);
    assert!(r.offline, "{r:?}");
    assert_eq!(r.queued, 1);
    let _ = std::fs::remove_dir_all(&a_data);
    let _ = std::fs::remove_dir_all(&b_data);
}
