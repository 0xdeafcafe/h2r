use super::*;
use crate::{blobs::Meta, films::inspect::Inspector};
use live_proto::film::{Summary, range, stream};
use ruzstd::encoding::{CompressionLevel, compress_to_vec};
use serde_json::{Value, json};

fn chunk(bytes: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) -> Value {
    let offset = bytes.len();
    let packed = compress_to_vec(payload, CompressionLevel::Uncompressed);
    bytes.extend(tag);
    bytes.extend((packed.len() as u32).to_le_bytes());
    bytes.extend(&packed);
    json!({"tag":tag,"offset":offset,"size":packed.len()})
}

struct Film {
    data: Vec<u8>,
    tick: Range<u64>,
    private: Range<u64>,
    index: Range<u64>,
}

fn fixture(forge: bool) -> Film {
    let front = serde_json::to_vec(&json!({
        "id":"game", "final_head":range::DATA_OFFSET, "image_stamp":"retail", "runtime_stamp":"r", "started_ms":50,
        "map_sha256":"abc", "map_origin":"retail", "roster":[], "match_start_tick":1, "ticks_per_second":60,
        "summary":Summary::default()
    }))
    .unwrap();
    let mut data = range::Prefix {
        json_len: front.len() as u32,
    }
    .encode()
    .to_vec();
    data.extend(front);
    data.resize(range::DATA_OFFSET as usize, 0);
    let head = chunk(
        &mut data,
        b"HEAD",
        br#"{"version":3,"meta":{"started_ms":50}}"#,
    );
    let raw = stream::ChunkWriter::new(1, 3).unwrap().finish();
    let mut hidden = vec![];
    chunk(&mut hidden, b"TICK", &raw);
    let a = data.len() as u64;
    let disk = chunk(&mut data, b"DATA", &hidden);
    let b = data.len() as u64;
    // Even a well-formed, compressed public frame inside a private disk
    // payload must never be addressable. Uncompressed zstd retains its bytes.
    let inner = data
        .windows(hidden.len())
        .position(|w| w == hidden)
        .unwrap() as u64;
    let tick = chunk(&mut data, b"TICK", &raw);
    let c = data.len() as u64;
    let directory = serde_json::to_vec(&json!({
        "version":1, "chunks":[{"first":1,"last":3,"scene":1,
        "offset":if forge {inner} else {b}, "size":tick["size"],"raw_size":raw.len()}],"keyframes":[]
    })).unwrap();
    let cidx = chunk(&mut data, b"CIDX", &directory);
    let d = data.len() as u64;
    let physical = serde_json::to_vec(&[head, disk, tick, cidx]).unwrap();
    chunk(&mut data, b"INDX", &physical);
    data.extend(
        range::Footer {
            cidx_offset: c,
            indx_offset: d,
            file_len: data.len() as u64 + range::FOOTER_BYTES as u64,
            cidx_raw_size: directory.len() as u32,
        }
        .encode(),
    );
    Film {
        data,
        private: a..b,
        tick: b..c,
        index: c..d,
    }
}

fn inspect(data: &[u8]) -> ApiResult<WebRanges> {
    let mut inspector = Inspector::default();
    for part in data.chunks(29) {
        inspector.consume(part)?;
    }
    let film = inspector.finish()?;
    assert_eq!(film.events.started_ms, Some(50));
    assert_eq!(film.events.tick_rate, Some(60));
    assert_eq!(film.summary, Some(Summary::default()));
    Ok(film.web_ranges.unwrap())
}

#[test]
fn receipt_requires_the_physical_index_and_complete_footer() {
    let good = fixture(false);
    assert_eq!(inspect(&good.data).unwrap().ticks_per_second, Some(60));
    assert!(inspect(&good.data[..good.data.len() - 1]).is_err());
    assert!(inspect(&fixture(true).data).is_err());
    let mut bad = good.data;
    let end = bad.len();
    bad[end - 40 + 8] ^= 1;
    assert!(inspect(&bad).is_err());
}

#[test]
fn consumption_stops_for_stats_before_the_next_chunk() {
    let film = fixture(false);
    let mut inspector = Inspector::default();
    let mut consumed = 0;
    while (consumed as u64) < film.tick.end {
        consumed += inspector.consume_one(&film.data[consumed..]).unwrap();
    }
    assert_eq!(consumed as u64, film.tick.end);
    inspector.consume(&film.data[consumed..]).unwrap();
    assert_eq!(inspector.finish().unwrap().events.tick_rate, Some(60));
}

#[tokio::test]
async fn fetches_only_public_extents_from_the_same_blob() {
    let film = fixture(false);
    let receipt = inspect(&film.data).unwrap();
    let cache = WebIndexCache::default();
    let store = BlobStore::memory();
    let key = "films/test.h2film";
    let mut legacy = serde_json::to_value(&receipt).unwrap();
    legacy.as_object_mut().unwrap().remove("public_safe");
    let legacy: WebRanges = serde_json::from_value(legacy).unwrap();
    assert!(!legacy.public_safe);
    assert!(legacy.read(&cache, &store, key, &(0..32)).await.unwrap().is_none());
    store
        .put(key, Bytes::from(film.data.clone()), &Meta::default())
        .await
        .unwrap();
    let summary = 32..(32 + u64::from(receipt.summary_bytes));
    let footer = (receipt.size - 40)..receipt.size;
    for span in [
        0..32,
        summary.clone(),
        film.tick.clone(),
        film.tick.start + 8..film.tick.end,
        film.index.start + 8..film.index.end,
        film.index.clone(),
        footer,
    ] {
        let (bytes, complete) = receipt
            .read(&cache, &store, key, &span)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            complete,
            span == film.tick,
            "only entire time chunks can be shared"
        );
        assert_eq!(
            bytes.as_ref(),
            &film.data[span.start as usize..span.end as usize]
        );
    }
    for span in [
        film.private.clone(),
        film.private.start + 1..film.tick.end,
        film.tick.start..film.index.end,
        0..receipt.size,
        0..summary.end + 1,
        film.index.end..receipt.size - 40,
    ] {
        assert!(
            receipt
                .read(&cache, &store, key, &span)
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn only_one_explicit_range_or_the_footer_suffix() {
    let h = |v| HeaderMap::from_iter([(header::RANGE, axum::http::HeaderValue::from_static(v))]);
    assert_eq!(parse_range(&h("bytes=-40"), 100), Some(60..100));
    assert_eq!(parse_range(&h("bytes=61-99"), 100), Some(61..100));
    for value in [
        "bytes=0-1,5-6",
        "bytes=30-",
        "bytes=3-2",
        "bytes=-100",
        "bytes=1-100",
        "bytes=1-18446744073709551615",
        "bytes=+1-2",
    ] {
        assert_eq!(parse_range(&h(value), 100), None);
    }
    let mut multiple = h("bytes=0-31");
    multiple.append(header::RANGE, "bytes=-40".parse().unwrap());
    assert_eq!(parse_range(&multiple, 100), None);
    let res = range_response(
        StatusCode::PARTIAL_CONTENT,
        Some(&(0..3)),
        100,
        Bytes::from_static(b"123"),
        false,
        None,
    );
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 0-2/100");
    assert_eq!(
        res.headers()[header::CACHE_CONTROL],
        "private, no-store, no-transform"
    );
}

#[test]
fn index_cache_retains_recent_films_and_limits_heap_residency() {
    let cache = WebIndexCache::default();
    let index = |n| Extents {
        chunks: vec![Extent { offset: 1, size: 4 }; n].into_boxed_slice(),
    };
    let first = cache.insert("first", index(2));
    for n in 1..MAX_CACHED_FILMS {
        cache.insert(&n.to_string(), index(1));
    }
    assert!(Arc::ptr_eq(&first, &cache.get("first").unwrap()));
    cache.insert("another", index(1));
    assert!(cache.get("1").is_none());
    let count = MAX_CACHED_BYTES / std::mem::size_of::<Extent>();
    cache.insert("large", index(count));
    assert!(cache.get("first").is_none());
    cache.insert("too-big", index(count + 1));
    assert!(cache.get("too-big").is_none());
    assert!(cache.get("large").is_some());
    cache.insert("large", index(3));
    cache.insert("small", index(2));
    assert_eq!(cache.get("large").unwrap().chunks.len(), 3);
}
