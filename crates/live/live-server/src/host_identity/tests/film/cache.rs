use super::*;

#[tokio::test]
async fn shared_cache_needs_the_public_film_revision_and_a_complete_time_chunk() {
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
    let (start, end) = loop {
        let packed = u32::from_le_bytes(film[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let next = pos + packed + 8;
        if &film[pos..pos + 4] == b"TICK" {
            break (pos, next);
        }
        pos = next;
    };
    assert_eq!(
        upload(&live, id, &token, film).await,
        StatusCode::NO_CONTENT
    );
    let app = crate::http::router(live.clone());
    let get = |query: &str, span: &str| {
        app.clone().oneshot(
            Request::get(format!("/v1/games/{id}/film{query}"))
                .header(header::RANGE, format!("bytes={span}"))
                .body(Body::empty())
                .unwrap(),
        )
    };
    const PRIVATE: &str = "private, no-store, no-transform";
    let probe = get("", "0-31").await.unwrap();
    assert_eq!(probe.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(probe.headers()[header::CACHE_CONTROL], PRIVATE);
    let etag = probe.headers()[header::ETAG].to_str().unwrap();
    let rev = etag
        .strip_prefix("\"film-")
        .unwrap()
        .strip_suffix('"')
        .unwrap();
    let span = format!("{start}-{}", end - 1);
    let query = format!("?rev={rev}&span={span}");
    let public = get(&query, &span).await.unwrap();
    assert_eq!(public.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        public.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable, no-transform"
    );
    assert_eq!(public.headers()[header::ETAG], etag);
    assert_eq!(public.headers()[header::VARY], "Authorization, Range");
    for (query, span) in [
        (String::new(), span.clone()),
        (format!("{query}&auth=1"), span.clone()),
        (format!("?rev={rev}&span=0-31"), "0-31".into()),
        (
            format!("?rev={rev}&span={start}-{start}"),
            format!("{start}-{start}"),
        ),
    ] {
        let response = get(&query, &span).await.unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::CACHE_CONTROL], PRIVATE);
    }
    let mismatch = get(&query, "0-31").await.unwrap();
    assert_eq!(mismatch.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(mismatch.headers()[header::CACHE_CONTROL], PRIVATE);
    assert_eq!(
        get(&format!("?rev=old&span={span}"), &span)
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}
