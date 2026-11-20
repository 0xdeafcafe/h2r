use super::*;

const MAC: &str = "02:73:f1:aa:19:c4";
const IP: &str = "192.0.2.123:3074";
const XUID: &str = "1234567890123456789";

pub(super) fn private_lifecycle(ticks: &mut stream::ChunkWriter) {
    let mut payload = vec![stream::LIFECYCLE_EVENT];
    payload.extend(
        serde_json::to_vec(&json!([
            {"kind":"host_migration","host_id":MAC},
            {"kind":"join","player":{"seat":2,"name":"Guest","kind":"phantom","team":1,
            "xuid":1234567890123456789u64,"bot_params":{"host_address":IP,"mac":MAC}}}
        ]))
        .unwrap(),
    );
    ticks.record(1, stream::EVENT, &payload).unwrap();
}

#[tokio::test]
async fn anonymous_http_ranges_expose_public_events_and_deny_their_originals() {
    let live = live().await;
    let lease = lease();
    let id = &lease.id;
    let token = assigned_lease(&live, &lease).await;
    live.set_blobs(crate::blobs::Blobs {
        private: crate::blobs::BlobStore::memory(),
        public: None,
    });
    let film = fixture(id, new_game(), 1);
    let mut pos = range::DATA_OFFSET as usize;
    let find = |pos: usize| {
        let size = u32::from_le_bytes(film[pos + 4..pos + 8].try_into().unwrap());
        pos..pos + 8 + size as usize
    };
    while &film[pos..pos + 4] != b"IPRV" {
        pos = find(pos).end;
    }
    let private = find(pos);
    let public = find(private.end);
    assert_eq!(&film[public.start..public.start + 4], b"TICK");
    // This fixture deliberately uses uncompressed zstd blocks, so these
    // sentinels also establish that the uploaded private film retains them.
    for secret in [MAC, IP, XUID] {
        assert!(
            film[private.clone()]
                .windows(secret.len())
                .any(|w| w == secret.as_bytes())
        );
    }
    assert_eq!(
        upload(&live, id, &token, film).await,
        StatusCode::NO_CONTENT
    );

    let get = |span: std::ops::Range<usize>| {
        crate::http::router(live.clone()).oneshot(
            Request::get(format!("/v1/games/{id}/film"))
                .header(
                    header::RANGE,
                    format!("bytes={}-{}", span.start, span.end - 1),
                )
                .body(Body::empty())
                .unwrap(),
        )
    };
    let denied = get(private.clone()).await.unwrap();
    assert_eq!(denied.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        get(private.end - 1..public.start + 1)
            .await
            .unwrap()
            .status(),
        StatusCode::RANGE_NOT_SATISFIABLE
    );
    let response = get(public).await.unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    let frame = to_bytes(response.into_body(), stream::MAX_BYTES)
        .await
        .unwrap();
    let mut decoded = Vec::new();
    std::io::Read::read_to_end(
        &mut ruzstd::decoding::StreamingDecoder::new(&frame[8..]).unwrap(),
        &mut decoded,
    )
    .unwrap();
    for secret in [
        MAC,
        IP,
        XUID,
        "host_id",
        "bot_params",
        "xuid",
        "host_address",
        "host-local-name-is-not-trusted",
    ] {
        assert!(
            !decoded
                .windows(secret.len())
                .any(|w| w == secret.as_bytes()),
            "{secret}"
        );
    }
    let records = stream::Chunk::parse(&decoded).unwrap();
    let lifecycle = records
        .records()
        .map(Result::unwrap)
        .find(|r| r.tag == stream::EVENT && r.payload.first() == Some(&stream::LIFECYCLE_EVENT))
        .unwrap();
    let events: Value = serde_json::from_slice(&lifecycle.payload[1..]).unwrap();
    assert_eq!(
        events,
        json!([
            {"kind":"host_migration"},
            {"kind":"join","player":{"seat":2,"name":"Guest","team":1,"kind":"phantom"}}
        ])
    );
}
