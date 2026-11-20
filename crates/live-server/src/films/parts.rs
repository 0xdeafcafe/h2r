//! Immutable multipart films. A selected manifest contains the SHA-256 digest
//! of each verified 16 MiB upload part in order. Its key determines the only
//! directory from which those parts can be fetched.
use std::{collections::BTreeSet, ops::Range};

use bytes::Bytes;
use futures_util::{
    StreamExt, TryStreamExt,
    stream::{self, BoxStream},
};
use live_proto::film::{MAX_UPLOAD_BYTES, PART_BYTES};
use serde::{Deserialize, Serialize};

use crate::blobs::{BlobError, BlobStore, Meta, Result, Seen};

const VERSION: u8 = 1;
// At most 128 SHA-256 hex strings for a 2 GiB recording.
const MANIFEST_LIMIT: usize = 16 << 10;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u8,
    size: u64,
    parts: Vec<String>,
}

impl Manifest {
    fn validate(&self) -> Result<()> {
        if self.version != VERSION
            || !(1..=MAX_UPLOAD_BYTES).contains(&self.size)
            || self.size.div_ceil(PART_BYTES) != self.parts.len() as u64
            || self.parts.iter().any(|h| {
                h.len() != 64 || !h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            })
        {
            return Err(BlobError::Config("invalid film part manifest".into()));
        }
        Ok(())
    }
}

pub(super) fn part_key(manifest_key: &str, number: u64, digest: &str) -> String {
    format!("{manifest_key}.data/{number}-{digest}")
}

/// The caller inspected each numbered part when it arrived and verified the
/// whole-file digest before publishing. A retry may select the same bytes.
pub(super) async fn publish(
    store: &BlobStore,
    key: &str,
    size: u64,
    hashes: &[String],
) -> Result<()> {
    if !key.ends_with(".parts") {
        return Err(BlobError::Config("invalid film manifest key".into()));
    }
    let manifest = Manifest {
        version: VERSION,
        size,
        parts: hashes.to_vec(),
    };
    manifest.validate()?;
    let bytes = Bytes::from(
        serde_json::to_vec(&manifest)
            .map_err(|e| BlobError::Config(format!("encoding film part manifest: {e}")))?,
    );
    // The directory backend has a single owner. Check its current value too
    // since some stores do not implement conditional puts.
    if let Some(prior) = store.get_prefix(key, MANIFEST_LIMIT as u64 + 1).await? {
        return if prior == bytes {
            Ok(())
        } else {
            Err(BlobError::Changed(key.into()))
        };
    }
    let meta = Meta {
        content_type: Some("application/json".into()),
        ..Meta::default()
    };
    match store
        .put_if(key, bytes.clone(), &Seen::default(), &meta)
        .await
    {
        Ok(_) => Ok(()),
        Err(BlobError::Changed(_))
            if store
                .get_prefix(key, MANIFEST_LIMIT as u64 + 1)
                .await?
                .as_ref()
                == Some(&bytes) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

async fn load_manifest(store: &BlobStore, key: &str) -> Result<Option<Manifest>> {
    let Some(json) = store.get_prefix(key, MANIFEST_LIMIT as u64 + 1).await? else {
        return Ok(None);
    };
    if json.len() > MANIFEST_LIMIT {
        return Err(BlobError::Config("oversized film part manifest".into()));
    }
    let manifest: Manifest = serde_json::from_slice(&json)
        .map_err(|e| BlobError::Config(format!("invalid film part manifest: {e}")))?;
    manifest.validate()?;
    Ok(Some(manifest))
}

/// Return precisely the immutable keys the signed receipt still references,
/// including its selected manifest. Extra parts from lost CAS attempts in the
/// same directory can then be collected without deleting a selected byte.
pub(in crate::films) async fn retained_keys(
    store: &BlobStore,
    key: &str,
) -> Result<BTreeSet<String>> {
    let mut keys = BTreeSet::from([key.to_owned()]);
    if key.ends_with(".parts") {
        let manifest = load_manifest(store, key)
            .await?
            .ok_or_else(|| BlobError::Changed(key.into()))?;
        for (number, hash) in manifest.parts.iter().enumerate() {
            keys.insert(part_key(key, number as u64, hash));
        }
    }
    Ok(keys)
}

/// Stream the original bytes in order. Each part's object stream opens
/// on demand so Worker memory and open R2 requests stay bounded.
pub(super) async fn get_stream(
    store: &BlobStore,
    key: &str,
) -> Result<Option<(u64, BoxStream<'static, Result<Bytes>>)>> {
    if !key.ends_with(".parts") {
        return store.get_stream(key).await;
    }
    let Some(manifest) = load_manifest(store, key).await? else {
        return Ok(None);
    };
    let size = manifest.size;
    let store = store.clone();
    let key = key.to_owned();
    let stream = stream::iter(manifest.parts.into_iter().enumerate())
        .then(move |(n, digest)| {
            let key = part_key(&key, n as u64, &digest);
            let store = store.clone();
            async move {
                let (stored, body) = store
                    .get_stream(&key)
                    .await?
                    .ok_or_else(|| BlobError::Changed(key.clone()))?;
                if stored != (size - n as u64 * PART_BYTES).min(PART_BYTES) {
                    return Err(BlobError::Changed(key));
                }
                Ok(body)
            }
        })
        .try_flatten()
        .boxed();
    Ok(Some((size, stream)))
}

/// Read a right-exclusive byte range and return the total recording length.
/// Only read parts intersecting the requested range. Missing selected parts
/// indicate an inconsistent recording; a missing manifest denotes no film.
pub(crate) async fn get_range(
    store: &BlobStore,
    key: &str,
    range: Range<u64>,
) -> Result<Option<(Bytes, u64)>> {
    if !key.ends_with(".parts") {
        return store.get_range(key, range).await;
    }
    let Some(manifest) = load_manifest(store, key).await? else {
        return Ok(None);
    };
    if range.is_empty() || range.end > manifest.size {
        return Err(BlobError::Config("invalid multipart film range".into()));
    }

    let first = range.start / PART_BYTES;
    let last = (range.end - 1) / PART_BYTES;
    let mut joined = Vec::new();
    for number in first..=last {
        let offset = number * PART_BYTES;
        let length = (manifest.size - offset).min(PART_BYTES);
        let span = range.start.saturating_sub(offset)..(range.end - offset).min(length);
        let part = part_key(key, number, &manifest.parts[number as usize]);
        let (bytes, total) = store
            .get_range(&part, span)
            .await?
            .ok_or_else(|| BlobError::Changed(part.clone()))?;
        if total != length {
            return Err(BlobError::Changed(part));
        }
        if first == last {
            return Ok(Some((bytes, manifest.size)));
        }
        joined.extend_from_slice(&bytes);
    }
    Ok(Some((Bytes::from(joined), manifest.size)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "films/game/upload.parts";

    #[tokio::test]
    async fn reads_only_intersecting_parts_and_preserves_byte_offsets() {
        let store = BlobStore::memory();
        let hashes = vec!["a".repeat(64), "b".repeat(64)];
        let size = PART_BYTES + 3;
        let tail = part_key(KEY, 1, &hashes[1]);
        store
            .put(&tail, Bytes::from_static(b"end"), &Meta::default())
            .await
            .unwrap();
        publish(&store, KEY, size, &hashes).await.unwrap();
        // The first part is absent, so fetching the suffix must not read it.
        let (bytes, total) = get_range(&store, KEY, PART_BYTES + 1..size)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&bytes[..], b"nd");
        assert_eq!(total, size);
        assert!(get_range(&store, KEY, PART_BYTES - 2..size).await.is_err());

        let mut first = vec![0; PART_BYTES as usize];
        first[PART_BYTES as usize - 2..].copy_from_slice(b"AB");
        store
            .put(
                &part_key(KEY, 0, &hashes[0]),
                first.into(),
                &Meta::default(),
            )
            .await
            .unwrap();
        let (bytes, _) = get_range(&store, KEY, PART_BYTES - 2..size)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&bytes[..], b"ABend");

        let (declared, mut original) = get_stream(&store, KEY).await.unwrap().unwrap();
        assert_eq!(declared, size);
        let mut position = 0u64;
        let mut last_five = Vec::new();
        while let Some(chunk) = original.try_next().await.unwrap() {
            let skip = (size - 5).saturating_sub(position).min(chunk.len() as u64) as usize;
            last_five.extend_from_slice(&chunk[skip..]);
            position += chunk.len() as u64;
        }
        assert_eq!(position, size);
        assert_eq!(last_five, b"ABend");

        publish(&store, KEY, size, &hashes).await.unwrap();
        assert!(publish(&store, KEY, size - 1, &hashes).await.is_err());
        assert!(get_range(&store, KEY, 0..size + 1).await.is_err());
        assert!(get_range(&store, KEY, size..size).await.is_err());

        store
            .put(&tail, Bytes::from_static(b"long"), &Meta::default())
            .await
            .unwrap();
        assert!(
            get_range(&store, KEY, PART_BYTES..PART_BYTES + 1)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn rejects_bad_manifests_and_supports_single_object_films() {
        let store = BlobStore::memory();
        assert!(get_range(&store, KEY, 0..1).await.unwrap().is_none());
        for bad in [
            serde_json::json!({"version":2,"size":1,"parts":["0".repeat(64)]}),
            serde_json::json!({"version":1,"size":1,"parts":["../another-film"]}),
            serde_json::json!({"version":1,"size":MAX_UPLOAD_BYTES+1,"parts":[]}),
            serde_json::json!({"version":1,"size":PART_BYTES+1,"parts":["0".repeat(64)]}),
        ] {
            store
                .put(KEY, bad.to_string().into(), &Meta::default())
                .await
                .unwrap();
            assert!(get_range(&store, KEY, 0..1).await.is_err());
        }
        store
            .put(KEY, vec![b' '; MANIFEST_LIMIT + 1].into(), &Meta::default())
            .await
            .unwrap();
        assert!(get_range(&store, KEY, 0..1).await.is_err());
        let legacy = "films/old.h2film";
        store
            .put(legacy, Bytes::from_static(b"abcd"), &Meta::default())
            .await
            .unwrap();
        let (bytes, size) = get_range(&store, legacy, 1..3).await.unwrap().unwrap();
        assert_eq!((&bytes[..], size), (&b"bc"[..], 4));
    }
}
