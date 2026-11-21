use super::super::inspect::{Inspector, checkpoint::digest, inflate_limited};
use super::*;
use crate::blobs::Seen;
use live_proto::film::sharing::{HEADER, Layout, MAX_SEED, clean_seed};
use sha2_checkpoint::{Digest as _, Sha256 as Hash};

const WINDOW: u64 = 2 << 20;

#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    layout: Option<Layout>,
    compat: Option<Compat>,
    game_id: Option<String>,
    pos: u64,
    next_block: u64,
    blocks: u32,
    #[serde(with = "digest")]
    content: Hash,
    #[serde(with = "digest")]
    offsets: Hash,
    #[serde(with = "digest")]
    trailer: Hash,
}

pub(super) fn cursor_key(s: &Share) -> String {
    format!("film-staging/{}/share", s.id)
}

/// Scans lengths, offsets and the compressed file's digest. Only the seed
/// is inflated. Never trust an index to skip a machine envelope.
impl Cursor {
    fn film(&mut self, bytes: &[u8], file: &Object) -> ApiResult<usize> {
        if self.pos == 0 {
            let layout = Layout::parse(
                bytes
                    .get(..HEADER)
                    .ok_or_else(|| invalid("short film header"))?,
                file.size,
            )
            .map_err(invalid)?;
            if bytes[48..64] != [0; 16] {
                return Err(invalid("unsupported film compression dictionary"));
            }
            let packed = bytes
                .get(HEADER..layout.seed_end as usize)
                .ok_or_else(|| invalid("short film seed"))?;
            let raw = inflate_limited(packed, MAX_SEED)?;
            let seed = clean_seed(&raw).map_err(invalid)?;
            if serde_json::from_slice::<Value>(&raw).ok().as_ref() != Some(&seed) {
                return Err(invalid("film seed contains private fields"));
            }
            let game = seed["game_id"]
                .as_str()
                .ok_or_else(|| invalid("missing game id"))?;
            if bytes[16..32] != live_proto::film::sharing::header_id(game) {
                return Err(invalid("film header contains an unrelated recording id"));
            }
            self.game_id = Some(game.to_owned());
            self.compat = Some(
                serde_json::from_value(seed["compat"].clone())
                    .map_err(|_| invalid("invalid compatibility"))?,
            );
            self.next_block = layout.seed_end;
            self.layout = Some(layout);
        }
        let l = self
            .layout
            .as_ref()
            .ok_or_else(|| invalid("missing film header"))?;
        let mut end = self.pos + bytes.len() as u64;
        if self.pos < l.trailer {
            end = end.min(l.trailer);
            while self.next_block < end {
                let i = (self.next_block - self.pos) as usize;
                let Some(head) = bytes.get(i..i + 9) else {
                    end = self.next_block;
                    break;
                };
                let len = u32::from_le_bytes(head[1..5].try_into().unwrap());
                if head[0] != 0
                    || len < 4
                    || head[5..9] != [0x28, 0xb5, 0x2f, 0xfd]
                    || self.blocks >= l.chunks
                {
                    return Err(invalid("invalid input film block"));
                }
                self.offsets.update(self.next_block.to_le_bytes());
                self.next_block += 5 + u64::from(len);
                if self.next_block > l.trailer {
                    return Err(invalid("film block extends past trailer"));
                }
                self.blocks += 1;
            }
        } else {
            let mut directory = bytes;
            if self.pos == l.trailer {
                let (count, tail) = bytes
                    .split_at_checked(4)
                    .ok_or_else(|| invalid("short film trailer"))?;
                if count != l.chunks.to_le_bytes()
                    || self.blocks != l.chunks
                    || self.next_block != l.trailer
                {
                    return Err(invalid("invalid film chunk count"));
                }
                directory = tail;
            }
            self.trailer.update(directory);
        }
        let n = (end - self.pos) as usize;
        if n == 0 {
            return Err(invalid("incomplete film block header"));
        }
        self.content.update(&bytes[..n]);
        self.pos = end;
        if end == file.size
            && (self.offsets.clone().finalize() != self.trailer.clone().finalize()
                || live_proto::sign::hex(&self.content.clone().finalize()) != file.sha256)
        {
            return Err(invalid("film index or SHA-256 mismatch"));
        }
        Ok(n)
    }
}

pub(super) async fn advance(store: &BlobStore, share: &Share) -> ApiResult<Option<Publication>> {
    let key = cursor_key(share);
    let (mut stats, mut cursor, seen) = match store.get_seen(&key).await.map_err(unavailable)? {
        Some((saved, seen)) => {
            let (stats, cursor) = Inspector::restore::<Cursor>(&saved)?;
            (stats, cursor, seen)
        }
        None => {
            for name in ["film", "stats"] {
                let file = share.object(name)?;
                parts::publish(store, &share.key(name), file.size, &file.parts)
                    .await
                    .map_err(unavailable)?;
            }
            (Inspector::stats_only(), Cursor::default(), Seen::default())
        }
    };
    let film = &share.upload.film;
    let (name, pos, size) = if cursor.pos < film.size {
        ("film", cursor.pos, film.size)
    } else {
        ("stats", stats.position(), share.upload.stats.size)
    };
    let (bytes, _) = parts::get_range(store, &share.key(name), pos..(pos + WINDOW).min(size))
        .await
        .map_err(unavailable)?
        .ok_or_else(|| invalid("upload missing numbered parts"))?;
    if name == "film" {
        cursor.film(&bytes, film)?;
    } else {
        let work = stats.completed_work();
        let mut rest = &bytes[..];
        while !rest.is_empty() && stats.completed_work() < work + 4 {
            let n = stats.consume_one(rest)?;
            stats.events.discard_rows();
            rest = &rest[n..];
        }
        if stats.position() == size {
            if stats.sha256() != share.upload.stats.sha256 {
                return Err(invalid("stats SHA-256 mismatch"));
            }
            if stats.stats_id != cursor.game_id {
                return Err(invalid("stats and film belong to different games"));
            }
            let checked = stats.finish()?;
            let ranges = checked
                .web_ranges
                .filter(|r| r.public_safe)
                .ok_or_else(|| invalid("stats companion has unsafe public ranges"))?;
            return Ok(Some(Publication {
                compat: cursor.compat.ok_or_else(|| invalid("missing film seed"))?,
                ticks: cursor
                    .layout
                    .ok_or_else(|| invalid("missing film header"))?
                    .ticks,
                summary: checked
                    .summary
                    .ok_or_else(|| invalid("missing stats summary"))?,
                ranges,
            }));
        }
    }
    let bytes = stats.checkpoint(&cursor)?;
    store
        .put_if(&key, bytes.into(), &seen, &Meta::default())
        .await
        .map_err(unavailable)?;
    Ok(None)
}
