use super::*;

pub(in crate::films) fn host_size(model: &Model, id: &str, source: Source<'_>) -> ApiResult<u64> {
    // A pinned Live game holds its stats stream and its film file.
    let film = model.films.film_files.get(id).map_or(0, |f| f.size);
    if let Some((size, _)) = model.films.downloads.get(id) {
        return Ok(*size + film);
    }
    if let Some(ranges) = source.ranges() {
        return Ok(ranges.size + film);
    }
    if let Source::Game(g) = source
        && !g.has_host_film
    {
        return Ok(0);
    }
    Err(invalid("host recording has no verified size for quota"))
}

/// Reservations and pinned films cannot be discarded. Position breaks ties
/// and is the same on every machine replaying the signed journal.
fn eviction_order(
    fixed: u64,
    mut candidates: Vec<(u64, String, u64)>,
    extra: u64,
    limit: u64,
) -> ApiResult<Vec<String>> {
    if fixed.saturating_add(extra) > limit {
        return Err(ApiError::new(
            ErrorCode::TooLarge,
            "The film will not fit in your 1 GB library. Unpin a film or finish pending uploads.",
        ));
    }
    let mut used = fixed + extra + candidates.iter().map(|row| row.2).sum::<u64>();
    candidates.sort_unstable();
    let mut evict = Vec::new();
    for (_, id, size) in candidates {
        if used <= limit {
            break;
        }
        used -= size;
        evict.push(id);
    }
    Ok(evict)
}

pub(in crate::films) fn evict(
    model: &Model,
    by: Xuid,
    extra: u64,
    reserve: bool,
) -> ApiResult<Vec<String>> {
    let mut fixed = 0;
    let mut candidates = Vec::new();
    for s in model.films.shared.values().filter(|s| s.owner == by) {
        let size = s.upload.film.size + s.upload.stats.size;
        if s.publication.is_none() {
            if reserve {
                fixed += size;
            }
        } else if s.pinned {
            fixed += size;
        } else {
            candidates.push((s.position, s.id.clone(), size));
        }
    }
    for id in model
        .films
        .saved
        .get(&by)
        .into_iter()
        .flat_map(|pins| pins.values())
    {
        if let Some(source) = visible_game(model, id, Viewer::Account(by)) {
            fixed += host_size(model, id, source)?;
        }
    }
    eviction_order(fixed, candidates, extra, ACCOUNT_QUOTA)
}

pub(in crate::films) async fn commit(
    live: &Live,
    version: u64,
    evictions: Vec<String>,
    kind: &str,
    data: Value,
) -> ApiResult<()> {
    let mut events = evictions
        .into_iter()
        .map(|id| live.server_event(STREAM.into(), DELETED, json!({"film":id})))
        .collect::<Vec<_>>();
    events.push(live.server_event(STREAM.into(), kind, data));
    let rows = live
        .store
        .append(events, &[(STREAM.into(), version)])
        .await?;
    live.wait_applied(rows.last().expect("film change").position)
        .await
}

pub(in crate::films) fn library(
    model: &Model,
    viewer: Viewer,
    by: Xuid,
    cursor: u64,
    limit: usize,
    pos: u64,
) -> Page<Value> {
    enum Item<'a> {
        Host(Source<'a>),
        Upload(&'a Share),
    }
    // Keep one page and its cursor. A long career must not materialize every
    // player's full scoreboard in the Worker's memory on each page request.
    let mut rows = BTreeMap::new();
    let mut keep = |pos, row| {
        rows.insert(pos, row);
        if rows.len() > limit + 1 {
            rows.pop_first();
        }
    };
    let removed = model.films.removed.get(&by);
    for source in model
        .games_newest()
        .map(Source::Game)
        .chain(model.films.standalone.values().map(Source::Standalone))
    {
        let key = source.position();
        if !source.players().contains(&by)
            || !source.visible(model, viewer)
            || (cursor != 0 && key >= cursor)
        {
            continue;
        }
        let film = source.listing(by);
        if !film.has_host_film
            || removed.is_some_and(|ids| ids.contains(film.game_id))
            || crate::sim_runs::is_sim_id(film.game_id)
        {
            continue;
        }
        keep(key, Item::Host(source));
    }
    for s in model.films.shared.values().filter(|s| {
        s.owner == by && s.publication.is_some() && (cursor == 0 || s.position < cursor)
    }) {
        keep(s.position, Item::Upload(s));
    }
    page(
        rows.into_iter().rev().map(|(key, item)| {
            let row = match item {
                Item::Host(source) => {
                    let mut row = json!(source.listing(by));
                    row["saved"] = json!(model.films.pinned_by(source, by));
                    row
                }
                Item::Upload(s) => s.row(),
            };
            (key, row)
        }),
        limit,
        pos,
    )
}

pub(super) async fn remove_game(live: &Live, author: &Author, id: &str) -> ApiResult<StatusCode> {
    let by = account(author)?;
    let viewer = live.viewer_of(author);
    let _guard = live.film_writes.lock().await;
    let (version, pinned) = {
        let model = live.model.read();
        if !visible_game(&model, id, viewer).is_some_and(|g| g.players().contains(&by)) {
            return Err(not_found("no such library film"));
        }
        if model
            .films
            .removed
            .get(&by)
            .is_some_and(|ids| ids.contains(id))
        {
            return Ok(StatusCode::NO_CONTENT);
        }
        (
            model.films.next_version,
            model
                .films
                .saved
                .get(&by)
                .is_some_and(|pins| pins.values().any(|game| game == id)),
        )
    };
    let mut changes = Vec::new();
    if pinned {
        changes.push(live.server_event(STREAM.into(), UNSAVED, json!({"game":id,"by":by})));
    }
    changes.push(live.server_event(STREAM.into(), REMOVED, json!({"film":id,"by":by})));
    let rows = live
        .store
        .append(changes, &[(STREAM.into(), version)])
        .await?;
    live.wait_applied(rows.last().unwrap().position).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn sweep(live: &Live) {
    let next = {
        let model = live.model.read();
        let mut last = model
            .films
            .garbage_cursor
            .lock()
            .expect("share cleanup cursor");
        model
            .films
            .garbage
            .range((Excluded(last.clone()), Unbounded))
            .next()
            .or_else(|| model.films.garbage.first_key_value())
            .map(|(id, s)| {
                last.clone_from(id);
                s.clone()
            })
    };
    if let Some(s) = next
        && let Err(error) = purge(live, &s).await
    {
        eprintln!("live: file share cleanup for {}: {error:?}", s.id);
    }
}

async fn purge(live: &Live, s: &Share) -> ApiResult<()> {
    let _upload = live.film_uploads.lock(&s.id).await;
    let store = store(live)?;
    for name in ["film", "stats"] {
        let key = s.key(name);
        for (i, hash) in s.object(name)?.parts.iter().enumerate() {
            store
                .delete(&parts::part_key(&key, i as u64, hash))
                .await
                .map_err(unavailable)?;
        }
        store.delete(&key).await.map_err(unavailable)?;
    }
    store
        .delete(&verify::cursor_key(s))
        .await
        .map_err(unavailable)?;
    let _guard = live.film_writes.lock().await;
    let version = {
        let model = live.model.read();
        if !model.films.garbage.contains_key(&s.id) {
            return Ok(());
        }
        model.films.next_version
    };
    live.append_film(version, PURGED, json!({"film":s.id}))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quota_drops_oldest_unpinned_and_keeps_pinned_and_incoming_films() {
        let items = vec![
            (30, "new".into(), 20),
            (1, "old".into(), 15),
            (20, "middle".into(), 30),
        ];
        assert_eq!(
            eviction_order(30, items, 25, 100).unwrap(),
            ["old", "middle"]
        );
        assert!(eviction_order(90, vec![(1, "old".into(), 30)], 11, 100).is_err());
        assert!(eviction_order(90, vec![], 10, 100).unwrap().is_empty());
    }
}
