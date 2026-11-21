//! Both homes execute these requests against their own event store.
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use live_proto::{
    Xuid,
    film::{
        range,
        sharing::{ACCOUNT_QUOTA, Object, Upload, Visibility},
        stream,
    },
    lxm,
    sign::{AccountRequest, KeyPair, issue_token},
};
use live_server::{Live, app::now};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tower::ServiceExt;

fn pack(raw: &[u8]) -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(raw, ruzstd::encoding::CompressionLevel::Uncompressed)
}

fn film(game: &str) -> Vec<u8> {
    let hash = "1".repeat(64);
    let seed = pack(
        &serde_json::to_vec(
            &json!({"game_id":game,"map":"lockout","map_sha256":hash,"seed":1,
        "compat":{"xbe":hash,"hooks":hash,"contract":hash,"abi":"test"},"roster":[]}),
        )
        .unwrap(),
    );
    let mut data = vec![0; 64];
    data[..8].copy_from_slice(b"H2FILM4!");
    data[8..16].copy_from_slice(&[4, 0, 2, 0, 30, 16, 0, 1]);
    data[16..32].copy_from_slice(&live_proto::film::sharing::header_id(game));
    data[32..36].copy_from_slice(&1u32.to_le_bytes());
    data[36..40].copy_from_slice(&(seed.len() as u32).to_le_bytes());
    data.extend(seed);
    let offset = data.len() as u64;
    let block = pack(&[1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    data.push(0);
    data.extend((block.len() as u32).to_le_bytes());
    data.extend(block);
    let trailer = data.len() as u64;
    data[40..48].copy_from_slice(&trailer.to_le_bytes());
    data.extend(1u32.to_le_bytes());
    data.extend(offset.to_le_bytes());
    data
}

fn chunk(data: &mut Vec<u8>, index: &mut Vec<Value>, tag: &[u8; 4], raw: &[u8]) -> (u64, u32) {
    let z = pack(raw);
    let offset = data.len() as u64;
    let size = z.len() as u32;
    index.push(json!({"offset":offset,"size":size,"tag":tag}));
    data.extend(tag);
    data.extend(size.to_le_bytes());
    data.extend(z);
    (offset, size)
}

fn stats(game: &str) -> Vec<u8> {
    let front = range::Front {
        final_head: range::DATA_OFFSET,
        id: game.into(),
        image_stamp: "test".into(),
        runtime_stamp: "test".into(),
        started_ms: 0,
        map_sha256: String::new(),
        map_origin: "retail".into(),
        match_start_tick: Some(1),
        ticks_per_second: Some(30),
        roster: vec![],
        summary: Default::default(),
    };
    let raw = serde_json::to_vec(&front).unwrap();
    let mut data = range::Prefix {
        json_len: raw.len() as u32,
    }
    .encode()
    .to_vec();
    data.extend(raw);
    data.resize(range::DATA_OFFSET as usize, 0);
    let mut index = Vec::new();
    chunk(
        &mut data,
        &mut index,
        b"HEAD",
        br#"{"version":3,"meta":{"started_ms":0}}"#,
    );
    let raw = stream::ChunkWriter::new(1, 1).unwrap().finish();
    let (offset, size) = chunk(&mut data, &mut index, b"TICK", &raw);
    let dir = range::Directory {
        chunks: vec![range::Chunk {
            offset,
            size,
            scene: 1,
            first: 1,
            last: 1,
            raw_size: raw.len() as u32,
            clock: vec![],
        }],
        ..Default::default()
    };
    let raw = serde_json::to_vec(&dir).unwrap();
    let (cidx_offset, _) = chunk(&mut data, &mut index, b"CIDX", &raw);
    let (indx_offset, _) = chunk(
        &mut data,
        &mut vec![],
        b"INDX",
        &serde_json::to_vec(&index).unwrap(),
    );
    let footer = range::Footer {
        cidx_offset,
        indx_offset,
        cidx_raw_size: raw.len() as u32,
        file_len: data.len() as u64 + range::FOOTER_BYTES as u64,
    };
    data.extend(footer.encode());
    data
}

fn object(bytes: &[u8]) -> Object {
    let digest = format!("{:x}", Sha256::digest(bytes));
    Object {
        size: bytes.len() as u64,
        sha256: digest.clone(),
        parts: vec![digest],
    }
}

struct Peer {
    key: KeyPair,
    issuer: String,
    xuid: Xuid,
}
async fn call(
    live: &Arc<Live>,
    method: &str,
    url: &str,
    token: Option<(&Peer, &str)>,
    range: Option<&str>,
    body: Vec<u8>,
) -> (u16, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(url);
    if let Some((p, scope)) = token {
        let bearer = issue_token(&p.key, &p.issuer, "live.test", scope, now());
        req = req.header("authorization", format!("Bearer {bearer}"));
    }
    if let Some(range) = range {
        req = req.header("range", range);
    }
    let res = live_server::http::router(live.clone())
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    (
        res.status().as_u16(),
        to_bytes(res.into_body(), 4 << 20).await.unwrap().to_vec(),
    )
}
fn json_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}
async fn account(live: &Arc<Live>, name: &str) -> Peer {
    let key = KeyPair::generate();
    let (status, body) = call(
        live,
        "POST",
        "/v1/accounts",
        None,
        None,
        AccountRequest::sign(&key, name, None, "live.test", now()).into_bytes(),
    )
    .await;
    assert_eq!(status, 201);
    let row = json_body(&body);
    Peer {
        key,
        issuer: row["id"].as_str().unwrap().into(),
        xuid: Xuid::parse(row["xuid"].as_str().unwrap()).unwrap(),
    }
}

async fn upload(live: &Arc<Live>, owner: &Peer, game: &str) -> (String, Vec<u8>, u64) {
    let film = film(game);
    let stats = stats(game);
    let upload = Upload {
        film: object(&film),
        stats: object(&stats),
        visibility: Visibility::Private,
    };
    let body = serde_json::to_vec(&upload).unwrap();
    let write = Some((owner, lxm::FILMS_WRITE));
    let reserve = |body| call(live, "POST", "/v1/films", write, None, body);
    assert_ne!(
        call(
            live,
            "POST",
            "/v1/films",
            Some((owner, lxm::READ)),
            None,
            body.clone()
        )
        .await
        .0,
        202
    );
    let (code, receipt) = reserve(body.clone()).await;
    assert_eq!(code, 202, "{}", String::from_utf8_lossy(&receipt));
    let id = json_body(&receipt)["film"].as_str().unwrap().to_string();
    let route = format!("/v1/films/{id}");
    let put = format!("{route}?object=film&part=0");
    assert_eq!(
        call(live, "PUT", &put, write, None, film.clone()).await.0,
        204
    );
    // Reconnect with the same descriptor and replay an unacknowledged part.
    assert_eq!(reserve(body).await.1, receipt);
    assert_eq!(
        call(live, "GET", &route, Some((owner, lxm::READ)), None, vec![])
            .await
            .0,
        404
    );
    assert_eq!(
        call(live, "PUT", &put, write, None, film.clone()).await.0,
        204
    );
    assert_eq!(
        call(
            live,
            "PUT",
            &format!("{route}?object=stats&part=0"),
            write,
            None,
            stats
        )
        .await
        .0,
        204
    );
    for _ in 0..12 {
        let (code, reply) = call(
            live,
            "POST",
            &format!("{route}?continue=1"),
            write,
            None,
            vec![],
        )
        .await;
        if code == 201 {
            return (id, film, upload.stats.size + upload.film.size);
        }
        assert_eq!(code, 202, "{}", String::from_utf8_lossy(&reply));
    }
    panic!("film publication did not finish");
}

pub async fn exercise(live: Arc<Live>) {
    let owner = account(&live, "Recorder").await;
    let guest = account(&live, "Viewer").await;
    let start = json!({"console":"test","map":"lockout","mode":"multiplayer","variant":"Slayer","team_play":false,
        "players":[{"index":0,"name":"Recorder","xuid":owner.xuid,"team":0,"local":true}]});
    let game = || {
        call(
            &live,
            "POST",
            "/v1/games",
            Some((&owner, lxm::GAMES_WRITE)),
            None,
            start.to_string().into_bytes(),
        )
    };
    let (a, first) = game().await;
    let (b, second) = game().await;
    assert_eq!((a, b), (201, 201));
    let first = json_body(&first);
    let second = json_body(&second);
    for field in ["seats", "machines"] {
        let ids = first["identifiers"][field].as_array().unwrap();
        let other = second["identifiers"][field].as_array().unwrap();
        assert_eq!(ids.len(), 16);
        assert!(ids.iter().all(|id| !other.contains(id)));
    }
    let read = Some((&owner, lxm::READ));
    let write = Some((&owner, lxm::FILMS_WRITE));
    let (old, bytes, _) = upload(&live, &owner, "old-game").await;
    let (pinned, _, pin_size) = upload(&live, &owner, "pin-game").await;
    let (new, _, new_size) = upload(&live, &owner, "new-game").await;
    let host = "host-game";
    live.fixture_host_film(owner.xuid, host, ACCOUNT_QUOTA - pin_size - new_size)
        .await;
    let library = format!("/v1/films?kind=library&player={}", owner.xuid);
    let list = || async {
        let (code, body) = call(&live, "GET", &library, read, None, vec![]).await;
        assert_eq!(code, 200);
        json_body(&body)["items"].as_array().unwrap().clone()
    };
    let rows = list().await;
    assert_eq!(rows.len(), 4);
    for id in [old.as_str(), pinned.as_str(), new.as_str(), host] {
        assert!(rows.iter().any(|r| r["game_id"] == id));
    }
    let route = format!("/v1/films/{old}");
    let get = format!("{route}/film?object=film");
    assert_eq!(
        call(
            &live,
            "GET",
            &get,
            Some((&guest, lxm::READ)),
            Some("bytes=0-7"),
            vec![]
        )
        .await
        .0,
        404
    );
    let change = |v: &str| serde_json::to_vec(&json!({"visibility":v})).unwrap();
    for visibility in ["link", "public"] {
        assert_eq!(
            call(
                &live,
                "PUT",
                &format!("{route}/visibility"),
                write,
                None,
                change(visibility)
            )
            .await
            .0,
            204
        );
        assert_eq!(
            call(&live, "GET", &get, None, Some("bytes=0-7"), vec![])
                .await
                .0,
            if visibility == "public" { 401 } else { 404 }
        );
        let mut second_machine = Vec::new();
        for (start, end) in [(0, 7), (8, bytes.len() - 1)] {
            let (status, part) = call(
                &live,
                "GET",
                &get,
                Some((&guest, lxm::READ)),
                Some(&format!("bytes={start}-{end}")),
                vec![],
            )
            .await;
            assert_eq!(status, 206);
            second_machine.extend(part);
        }
        assert_eq!(second_machine, bytes);
    }
    assert_eq!(
        call(
            &live,
            "GET",
            &format!("{route}/film"),
            None,
            Some("bytes=0-31"),
            vec![]
        )
        .await
        .0,
        206
    );
    assert_eq!(
        call(
            &live,
            "PUT",
            &format!("{route}/visibility"),
            Some((&guest, lxm::FILMS_WRITE)),
            None,
            change("private")
        )
        .await
        .0,
        403
    );
    assert_eq!(
        call(
            &live,
            "PUT",
            &format!("{route}/visibility"),
            write,
            None,
            change("private")
        )
        .await
        .0,
        204
    );
    for path in [route.clone(), format!("{route}/film"), get.clone()] {
        assert_eq!(
            call(
                &live,
                "GET",
                &path,
                Some((&guest, lxm::READ)),
                Some("bytes=0-7"),
                vec![]
            )
            .await
            .0,
            404
        );
    }
    for id in [pinned.as_str(), host] {
        assert_eq!(
            call(
                &live,
                "POST",
                &format!("/v1/films/{id}/save"),
                write,
                None,
                vec![]
            )
            .await
            .0,
            204
        );
    }
    let rows = list().await;
    assert!(
        rows.iter()
            .any(|r| r["game_id"] == pinned && r["saved"] == true)
    );
    assert!(rows.iter().any(|r| r["game_id"] == new));
    assert!(!rows.iter().any(|r| r["game_id"] == old));
    let desc = Upload {
        film: object(&vec![0; new_size as usize + 1]),
        stats: object(b"new stats"),
        visibility: Visibility::Private,
    };
    let (status, reason) = call(
        &live,
        "POST",
        "/v1/films",
        write,
        None,
        serde_json::to_vec(&desc).unwrap(),
    )
    .await;
    assert_eq!(status, 413, "{}", String::from_utf8_lossy(&reason));
    assert!(String::from_utf8_lossy(&reason).contains("Unpin"));
    let route = format!("/v1/films/{pinned}");
    assert_eq!(
        call(
            &live,
            "DELETE",
            &format!("{route}/save"),
            write,
            None,
            vec![]
        )
        .await
        .0,
        204
    );
    assert!(
        list()
            .await
            .iter()
            .any(|r| r["game_id"] == pinned && r["saved"] == false)
    );
    assert_eq!(
        call(&live, "DELETE", &route, write, None, vec![]).await.0,
        204
    );
    assert!(!list().await.iter().any(|r| r["game_id"] == pinned));
    assert_eq!(
        call(
            &live,
            "DELETE",
            &format!("/v1/films/{host}"),
            write,
            None,
            vec![]
        )
        .await
        .0,
        204
    );
    assert!(list().await.iter().all(|r| r["game_id"] != host));
}
