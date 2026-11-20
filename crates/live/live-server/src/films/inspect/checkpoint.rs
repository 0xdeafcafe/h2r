//! Private upload checkpoint. A versioned frame keeps the partly received
//! zstd payload binary. Only small parser cursors and the import journal
//! are JSON. SHA-256 includes its pending partial block in each checkpoint.
use super::{ApiResult, Inspector, MAX_CHUNK, MAX_ENTRIES, invalid, range};
use serde::{Serialize, de::DeserializeOwned};

const MAGIC: &[u8; 8] = b"H2INGST1";
const HEADER: usize = 16;

pub(super) mod digest {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    use sha2_checkpoint::{Sha256, digest::common::hazmat::SerializableState};

    pub fn serialize<S: Serializer>(digest: &Sha256, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(digest.serialize().as_slice())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Sha256, D::Error> {
        let bytes = Vec::<u8>::deserialize(d)?;
        let state = bytes.as_slice().try_into().map_err(D::Error::custom)?;
        Sha256::deserialize(state).map_err(D::Error::custom)
    }
}

impl Inspector {
    /// Persist together with the caller's progress in one compare/exchange.
    /// Only the current compressed frame carries over between HTTP parts.
    pub(in crate::films) fn checkpoint<T: Serialize>(&self, extra: &T) -> ApiResult<Vec<u8>> {
        let mut out = Vec::from(*MAGIC);
        out.resize(HEADER, 0);
        serde_json::to_writer(&mut out, &(self, extra))
            .map_err(|_| invalid("could not encode film inspection checkpoint"))?;
        let metadata_len = u32::try_from(out.len() - HEADER)
            .map_err(|_| invalid("film inspection checkpoint metadata too large"))?;
        let scenes = self.scene_starts.len();
        out[8..12].copy_from_slice(&metadata_len.to_le_bytes());
        out[12..16].copy_from_slice(&(scenes as u32).to_le_bytes());
        out.reserve_exact(scenes.div_ceil(4) + self.collected.len());
        for group in self.scene_starts.chunks(4) {
            let mut byte = 0;
            for (i, state) in group.iter().enumerate() {
                let bits = match state {
                    None => 0,
                    Some(false) => 1,
                    Some(true) => 2,
                };
                byte |= bits << (i * 2);
            }
            out.push(byte);
        }
        out.extend_from_slice(&self.collected);
        Ok(out)
    }

    pub(in crate::films) fn restore<T: DeserializeOwned>(bytes: &[u8]) -> ApiResult<(Self, T)> {
        let bad = || invalid("invalid film inspection checkpoint");
        if bytes.len() < HEADER || &bytes[..8] != MAGIC {
            return Err(bad());
        }
        let metadata_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let scenes = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        if scenes > MAX_ENTRIES {
            return Err(bad());
        }
        let (metadata, tail) = bytes[HEADER..]
            .split_at_checked(metadata_len)
            .ok_or_else(bad)?;
        let (scene_bytes, frame) = tail.split_at_checked(scenes.div_ceil(4)).ok_or_else(bad)?;
        let (mut inspector, extra): (Self, T) =
            serde_json::from_slice(metadata).map_err(|_| bad())?;
        if inspector.pos > live_proto::film::MAX_UPLOAD_BYTES
            || inspector.prefix.len() >= 8
            || inspector.footer.len() > range::FOOTER_BYTES
            || inspector.chunks > MAX_ENTRIES
            || scenes > inspector.chunks
            || inspector.packed as usize > MAX_CHUNK
            || inspector.remaining > inspector.packed as usize
        {
            return Err(bad());
        }
        let frame_len = if inspector.reading_front {
            if inspector.pos >= range::DATA_OFFSET {
                return Err(bad());
            }
            inspector.pos as usize
        } else if inspector.remaining != 0 && inspector.collect() {
            inspector.packed as usize - inspector.remaining
        } else {
            0
        };
        if frame.len() != frame_len {
            return Err(bad());
        }
        inspector.collected.extend_from_slice(frame);
        inspector.scene_starts.reserve_exact(scenes);
        for i in 0..scenes {
            let bits = (scene_bytes[i / 4] >> ((i % 4) * 2)) & 3;
            inspector.scene_starts.push(match bits {
                0 => None,
                1 => Some(false),
                2 => Some(true),
                _ => return Err(bad()),
            });
        }
        Ok((inspector, extra))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::film::Summary;
    use ruzstd::encoding::{CompressionLevel, compress_to_vec};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    fn append(bytes: &mut Vec<u8>, tag: [u8; 4], value: &Value) -> Value {
        let json = serde_json::to_vec(value).unwrap();
        let packed = compress_to_vec(json.as_slice(), CompressionLevel::Uncompressed);
        let entry = json!({"tag": tag, "offset": bytes.len(), "size": packed.len()});
        bytes.extend_from_slice(&tag);
        bytes.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&packed);
        entry
    }

    #[test]
    fn restore_at_each_byte_reproduces_the_index_summary_and_hash() {
        let mut bytes = Vec::new();
        let header = append(&mut bytes, *b"HEAD", &json!({"version": 2}));
        let key = append(&mut bytes, *b"KEYF", &json!("retained in the film"));
        let summary = Summary {
            duration_ticks: 87,
            ..Default::default()
        };
        let menu = append(&mut bytes, *b"SUMM", &json!(summary));
        let index_offset = bytes.len() as u64;
        append(&mut bytes, *b"INDX", &json!([header, key, menu]));
        bytes.extend_from_slice(b"H2FILM2!");
        bytes.extend_from_slice(&index_offset.to_le_bytes());

        let mut parser = Inspector::default();
        for (i, b) in bytes.iter().enumerate() {
            parser.consume(std::slice::from_ref(b)).unwrap();
            let checkpoint = parser.checkpoint(&i).unwrap();
            let (resumed, page): (_, usize) = Inspector::restore(&checkpoint).unwrap();
            assert_eq!(page, i);
            assert_eq!(resumed.position(), (i + 1) as u64);
            // Compares with the straight sha2-0.10 implementation at all
            // 64 partial-block lengths, including both padding boundaries.
            assert_eq!(
                resumed.sha256(),
                format!("{:x}", Sha256::digest(&bytes[..=i]))
            );
            parser = resumed;
        }
        assert_eq!(parser.finish().unwrap().summary, Some(summary));
    }

    #[test]
    fn partial_compressed_chunk_is_stored_verbatim_and_truncation_is_rejected() {
        let mut bytes = Vec::new();
        append(&mut bytes, *b"HEAD", &json!({"version": 1}));
        bytes.extend_from_slice(b"INDX");
        bytes.extend_from_slice(&100_000u32.to_le_bytes());
        bytes.extend(std::iter::repeat_n(255, 65_535));
        let mut parser = Inspector::default();
        parser.consume(&bytes).unwrap();
        let mut saved = parser.checkpoint(&()).unwrap();
        assert!(saved.ends_with(&bytes[bytes.len() - 65_535..]));
        assert!(saved.len() < 65_535 + 8_192);
        saved.pop();
        assert!(Inspector::restore::<()>(&saved).is_err());
    }
}
