//! Admission of ranges served verbatim from private film storage.
use super::{ApiResult, Front, invalid};
use live_proto::film::{
    events::Record,
    privacy::public_summary,
    stream::{self, Chunk},
};
use serde::{Deserialize, Serialize, de::IgnoredAny};
use sha2_checkpoint::{Digest, Sha256};

/// Only fields admitted by the typed front may be present in the on-disk
/// JSON. Compare structurally so object key order and omitted defaults are
/// immaterial. The front reserve bounds both JSON trees to 128 KiB of input.
pub(super) fn public_front(raw: &[u8], front: &Front) -> bool {
    use serde_json::Value;
    fn admitted(actual: &Value, public: &Value) -> bool {
        match (actual, public) {
            (Value::Object(a), Value::Object(p)) => a
                .iter()
                .all(|(k, v)| p.get(k).is_some_and(|allowed| admitted(v, allowed))),
            (Value::Array(a), Value::Array(p)) => {
                a.len() == p.len() && a.iter().zip(p).all(|(v, allowed)| admitted(v, allowed))
            }
            _ => actual == public,
        }
    }
    let mut public = front.clone();
    public_summary(&mut public.summary);
    let Ok(public) = serde_json::to_value(public) else {
        return false;
    };
    serde_json::from_slice(raw).is_ok_and(|actual| admitted(&actual, &public))
}

/// Full Directory validation already checks the types of every known field,
/// including the numeric clock tuples. Check unknown keys without allocating
/// a JSON tree per row of a potentially 8 MiB index.
pub(super) fn public_directory(raw: &[u8]) -> bool {
    #[allow(dead_code)]
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Row {
        clock: Option<IgnoredAny>,
        scene: IgnoredAny,
        first: IgnoredAny,
        last: IgnoredAny,
        offset: IgnoredAny,
        size: IgnoredAny,
        raw_size: IgnoredAny,
    }
    #[allow(dead_code)]
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Key {
        tick: IgnoredAny,
        offset: IgnoredAny,
    }
    #[allow(dead_code)]
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Directory {
        version: IgnoredAny,
        chunks: Vec<Row>,
        keyframes: Vec<Key>,
    }
    serde_json::from_slice::<Directory>(raw).is_ok()
}

/// The private frame contains every lifecycle and canonical stats event,
/// including unchanged ones between two redactions. The public chunk can
/// additionally contain pads, poses and highlights. Bind their common
/// records to the same tick, subtype, contents, multiplicity and order.
#[derive(Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Overlap {
    first: u64,
    count: u16,
    pub(super) stats: usize,
    digest: [u8; 32],
}

pub(super) fn overlap(raw: &[u8]) -> ApiResult<Overlap> {
    let chunk = Chunk::parse(raw).map_err(invalid)?;
    let mut digest = Sha256::new();
    let mut stats = 0;
    for record in chunk.records() {
        let record = record.map_err(invalid)?;
        if record.tag != stream::EVENT {
            continue;
        }
        match record.payload.split_first() {
            Some((&stream::LIFECYCLE_EVENT | &stream::CLOSING_EVENT, _)) => {}
            Some((&stream::STATS_EVENT, body))
                if Record::decode_telemetry(body).map_err(invalid)?.is_some() =>
            {
                stats += 1;
            }
            _ => continue,
        }
        digest.update(record.tick.to_le_bytes());
        digest.update((record.payload.len() as u64).to_le_bytes());
        digest.update(record.payload);
    }
    Ok(Overlap {
        first: chunk.first_tick,
        count: chunk.tick_count,
        stats,
        digest: digest.finalize().into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::films::inspect::Inspector;
    use live_proto::film::{
        privacy::{PRIVATE_TAG, project},
        range::TICK,
    };
    use ruzstd::encoding::{CompressionLevel, compress_to_vec};
    use serde_json::{Value, json};

    fn frame(tag: [u8; 4], raw: &[u8]) -> Vec<u8> {
        let packed = compress_to_vec(raw, CompressionLevel::Uncompressed);
        let mut bytes = tag.to_vec();
        bytes.extend((packed.len() as u32).to_le_bytes());
        bytes.extend(packed);
        bytes
    }

    fn block(first: u64, film_tick: u64, match_tick: u64) -> Vec<u8> {
        let mut w = stream::ChunkWriter::new(first, 2).unwrap();
        let mut event = vec![stream::STATS_EVENT];
        let row = json!({"tick":match_tick, "kind":"PlayerJoined", "data":{"player":{
            "index":0,"name":"Red","team":1,"xuid":live_proto::Xuid(123)
        }}});
        serde_json::to_writer(&mut event, &row).unwrap();
        w.record(film_tick, stream::EVENT, &event).unwrap();
        w.finish()
    }

    #[test]
    fn paired_chunks_preserve_the_original_once_across_an_upload_checkpoint() {
        let view = project(&block(1, 1, 19)).unwrap();
        let mut reader = Inspector {
            version: Some(3),
            public_safe: true,
            ..Default::default()
        };
        reader
            .consume(&frame(PRIVATE_TAG, &view.private.unwrap()))
            .unwrap();
        let saved = reader.checkpoint(&()).unwrap();
        let restore = || Inspector::restore::<()>(&saved).unwrap().0;
        let mut resumed = restore();
        resumed.consume(&frame(TICK, &view.public)).unwrap();
        let rows = serde_json::to_value(&resumed.events).unwrap();
        assert_eq!(rows["rows"].as_array().unwrap().len(), 1);
        assert_eq!(
            rows["rows"][0]["data"]["player"]["xuid"],
            json!(live_proto::Xuid(123))
        );
        assert!(resumed.public_safe);

        // Moving either clock, dropping the event, changing the covered time
        // range or inserting a machine frame breaks the IPRV/TICK association.
        for raw in [
            project(&block(1, 2, 19)).unwrap().public,
            project(&block(1, 1, 20)).unwrap().public,
            project(&block(2, 2, 19)).unwrap().public,
            stream::ChunkWriter::new(1, 2).unwrap().finish(),
        ] {
            assert!(restore().consume(&frame(TICK, &raw)).is_err());
        }
        assert!(restore().consume(&frame(*b"HEAD", b"{}")).is_err());
    }

    #[test]
    fn legacy_ticks_still_supply_stats_and_private_bytes_disable_web_ranges() {
        let mut reader = Inspector {
            version: Some(3),
            public_safe: true,
            ..Default::default()
        };
        reader.consume(&frame(TICK, &block(1, 1, 19))).unwrap();
        let rows = serde_json::to_value(&reader.events).unwrap();
        assert_eq!(
            rows["rows"][0]["data"]["player"]["xuid"],
            json!(live_proto::Xuid(123))
        );
        assert!(!reader.public_safe);
    }

    #[test]
    fn unknown_front_or_directory_fields_cannot_become_public_ranges() {
        let mut front = json!({
            "id":"g","final_head":99,"image_stamp":"i","runtime_stamp":"r","started_ms":0,
            "map_sha256":"m","map_origin":"o","roster":[],"summary":live_proto::film::Summary::default()
        });
        let safe = |v: &Value| {
            public_front(
                &serde_json::to_vec_pretty(v).unwrap(),
                &serde_json::from_value(v.clone()).unwrap(),
            )
        };
        assert!(safe(&front));
        front["summary"]["future_user_token"] = json!("hidden");
        assert!(!safe(&front));
        let mut directory = json!({"version":1,"chunks":[],"keyframes":[{"tick":1,"offset":12}]});
        assert!(public_directory(
            &serde_json::to_vec_pretty(&directory).unwrap()
        ));
        directory["keyframes"][0]["future_user_token"] = json!("hidden");
        assert!(!public_directory(&serde_json::to_vec(&directory).unwrap()));
    }
}
