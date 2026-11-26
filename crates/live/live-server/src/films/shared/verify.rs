use super::super::inspect::Inspector;
use super::super::upload::film_file::{Scan, Zstd};
use super::*;
use crate::blobs::Seen;

const WINDOW: u64 = 2 << 20;

#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    /// The walk over the film file (`H2FILM4!`, docs/game/film-format.md),
    /// the copy `film::file::share_copy` makes.
    scan: Scan,
    pos: u64,
    /// Set once the whole film file checked: its film ID, which its stats
    /// stream must name, its compatibility and its length.
    game_id: Option<String>,
    compat: Option<Compat>,
    ticks: u32,
}

pub(super) fn cursor_key(s: &Share) -> String {
    format!("film-staging/{}/share", s.id)
}

/// Walks the film file's block lengths and its trailer and checks its
/// digest. Only the seed is inflated, and it must name no machine or
/// account (`film_file::Scan::feed_account`).
impl Cursor {
    fn film(&mut self, bytes: &[u8], file: &Object) -> ApiResult<()> {
        self.scan.feed_account(bytes, &mut Zstd)?;
        self.pos += bytes.len() as u64;
        if self.pos == file.size {
            let checked = self.scan.finish(file.size)?;
            if checked.sha256 != file.sha256 {
                return Err(invalid("film SHA-256 mismatch"));
            }
            let (film, compat) = self
                .scan
                .account()
                .cloned()
                .ok_or_else(|| invalid("missing film seed"))?;
            self.game_id = Some(film);
            self.compat = Some(compat);
            self.ticks = checked.ticks;
        }
        Ok(())
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
                ticks: cursor.ticks,
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
