//! Owned mod packs and deterministic grouping of earlier MapPublished rows.
//! The same projection is rebuilt on native and Worker log replay.
use super::*;
use maps::{ModPackDocument, ModPackPublished, ModPackSummary, mod_pack_id_for};

#[derive(Debug, Default)]
pub(super) struct PackIndex {
    published: BTreeMap<String, ModPackPublished>,
    by_member: HashMap<MapId, String>,
    members: BTreeMap<String, BTreeSet<MapId>>,
    /// The first uploader linked to a curated pack keeps its old slug.
    /// Other uploaders have independent copies and moderation state.
    curated_owners: HashMap<String, Xuid>,
    /// Survives removing a member's manifest, so an empty/taken-down
    /// release cannot be acquired by a different uploader.
    owners: HashMap<String, Xuid>,
}

impl PackIndex {
    fn assign(&mut self, map: MapId, pack: Option<String>) {
        if let Some(old) = self.by_member.remove(&map)
            && let Some(members) = self.members.get_mut(&old)
        {
            members.remove(&map);
            if members.is_empty() {
                self.members.remove(&old);
            }
        }
        if let Some(pack) = pack {
            self.members.entry(pack.clone()).or_default().insert(map);
            self.by_member.insert(map, pack);
        }
    }
}

fn prefix(package: &Package) -> Option<&str> {
    let (name, map) = package.doc.manifest.package.name.rsplit_once(':')?;
    let name = name.trim();
    let map = map.trim();
    let normalize = |s: &str| {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let suffix = normalize(map);
    let base = normalize(&package.doc.manifest.base.map);
    let label = ::content::map_pack::title(&base)
        .or_else(|| (base == "cyclotron").then_some("Ivory Tower"));
    (!name.is_empty()
        && !suffix.is_empty()
        && (suffix == base || label.is_some_and(|label| normalize(label) == suffix)))
    .then_some(name)
}

fn singleton(map: MapId) -> String {
    format!("s-{}", &map.to_string()[..48])
}

impl CatalogState {
    /// Apply the migration while consuming the original events. No SQL
    /// changes, synthetic events or changes to a signed MapId are needed.
    pub(super) fn index_mod_member(&mut self, map: MapId) {
        let Some(p) = self.maps.get(&map) else { return };
        if let Some(entry) = self
            .mod_index
            .by_member
            .get(&map)
            .and_then(|id| self.mod_index.published.get(id))
            && entry.pack.members.contains(&map)
            && entry.uploader == p.author
        {
            return;
        }
        if p.doc.staged_for.is_some() {
            self.mod_index.assign(map, None);
            return;
        }
        let mut id = if let Some(curated) = self.details.get(&map).and_then(|d| d.pack.as_ref()) {
            let owner = self
                .mod_index
                .curated_owners
                .entry(curated.clone())
                .or_insert(p.author);
            if *owner == p.author {
                curated.clone()
            } else {
                format!("c-{}", &mod_pack_id_for(p.author, curated)[2..])
            }
        } else if let Some(title) = prefix(p) {
            mod_pack_id_for(p.author, title)
        } else {
            singleton(map)
        };
        // Once a release has a published manifest, only its explicit
        // members belong to it; a later legacy row can't silently join.
        if self.mod_index.published.contains_key(&id) {
            id = singleton(map);
        }
        self.mod_index.owners.entry(id.clone()).or_insert(p.author);
        self.mod_index.assign(map, Some(id));
    }

    pub(super) fn apply_mod_pack(&mut self, release: ModPackPublished) {
        let id = release.id.clone();
        let new_members: BTreeSet<_> = release.pack.members.iter().copied().collect();
        let old = self.mod_index.members.get(&id).cloned().unwrap_or_default();
        self.mod_index.owners.insert(id.clone(), release.uploader);
        self.mod_index.published.insert(id.clone(), release);
        self.standings.remove(&(Target::Pack, id.clone()));
        for dropped in old.difference(&new_members) {
            self.mod_index.assign(*dropped, None);
            self.index_mod_member(*dropped);
        }
        for map in new_members {
            self.mod_index.assign(map, Some(id.clone()));
        }
    }

    pub fn mod_pack_id(&self, map: &MapId) -> Option<String> {
        self.mod_index.by_member.get(map).cloned()
    }

    pub fn mod_pack_exists(&self, id: &str) -> bool {
        self.mod_index.published.contains_key(id) || self.mod_index.members.contains_key(id)
    }

    pub fn mod_pack_owner(&self, id: &str) -> Option<Xuid> {
        self.mod_index.owners.get(id).copied()
    }

    fn curated_info(&self, map: &MapId) -> Option<&PackInfo> {
        self.details_of(map)?
            .pack
            .as_ref()
            .and_then(|key| self.packs.get(key))
    }

    /// Legacy credits are often member-specific. Admin corrections and
    /// named member authors survive a pack import. Fill only missing data.
    pub(super) fn release_credit(&self, map: &MapId, p: &Package) -> Option<Credit> {
        let mut c = self
            .credits
            .get(map)
            .cloned()
            .or_else(|| p.doc.credit())
            .unwrap_or_default();
        let explicit = self
            .mod_index
            .by_member
            .get(map)
            .and_then(|id| self.mod_index.published.get(id));
        let curated = self.curated_info(map);
        if c.authors.is_empty() {
            if let Some(rel) = explicit {
                c.authors.clone_from(&rel.pack.authors);
            } else if let Some(info) = curated {
                c.authors.clone_from(&info.authors);
            }
        }
        if c.source_url.is_none() {
            c.source_url = explicit
                .and_then(|rel| rel.pack.source_url.clone())
                .or_else(|| curated.and_then(|info| info.source_url.clone()));
        }
        (!c.is_empty()).then_some(c)
    }

    pub(super) fn pack_metadata_for_map(&self, map: &MapId) -> Option<PackListed> {
        let id = self.mod_index.by_member.get(map)?;
        let mut info = if let Some(rel) = self.mod_index.published.get(id) {
            rel.pack.info()
        } else if let Some(info) = self.curated_info(map) {
            info.clone()
        } else {
            let p = self.maps.get(map)?;
            PackInfo {
                name: prefix(p)?.to_owned(),
                ..PackInfo::default()
            }
        };
        info.images = self.gallery(Target::Pack, id, &info.images);
        Some(PackListed {
            id: id.clone(),
            info,
        })
    }

    pub(super) fn staged(&self, map: &MapId) -> bool {
        self.maps
            .get(map)
            .is_some_and(|p| p.doc.staged_for.is_some())
            && !self.mod_index.by_member.contains_key(map)
    }
}

impl Live {
    pub(super) fn legacy_pack(&self, s: &CatalogState, id: &str) -> Option<PackListed> {
        if !s.standing(Target::Pack, id).is_ok() {
            return None;
        }
        if s.mod_pack_exists(id) {
            let pack = self.mod_pack_in(s, id, true)?;
            return Some(PackListed {
                id: pack.id,
                info: pack.info,
            });
        }
        let mut info = s.packs.get(id)?.clone();
        info.images = s.gallery(Target::Pack, id, &info.images);
        Some(PackListed {
            id: id.to_owned(),
            info,
        })
    }

    pub(crate) fn mod_pack_in(
        &self,
        s: &CatalogState,
        id: &str,
        public_only: bool,
    ) -> Option<ModPackSummary> {
        let ids = s.mod_index.members.get(id)?;
        if public_only
            && (!s.standing(Target::Pack, id).is_ok()
                || ids.iter().any(|id| {
                    s.refusal(id).is_some()
                        || s.maps.get(id).is_none_or(|p| {
                            let m = &p.doc.manifest;
                            m.patches
                                .iter()
                                .chain(&m.packs)
                                .any(|layer| !s.servable(&layer.sha256))
                        })
                }))
        {
            return None;
        }
        let rel = s.mod_index.published.get(id);
        let mut members: Vec<_> = ids
            .iter()
            .filter_map(|id| s.maps.get(id).map(|p| self.map_summary(s, id, p)))
            .collect();
        members.sort_by(|a, b| {
            a.base
                .map
                .to_ascii_lowercase()
                .cmp(&b.base.map.to_ascii_lowercase())
                .then(a.map_id.cmp(&b.map_id))
        });
        let first = members.first();
        let uploader = self.who(s.mod_pack_owner(id)?);
        let curated = first.and_then(|m| s.curated_info(&m.map_id));
        let mut info = match (rel, curated, first) {
            (Some(rel), _, _) => rel.pack.info(),
            (_, Some(curated), _) => curated.clone(),
            (_, _, Some(first)) => {
                let package = s.maps.get(&first.map_id)?;
                let mut authors = Vec::new();
                for member in &members {
                    if let Some(c) = &member.credits {
                        for author in &c.authors {
                            if !authors.contains(author) {
                                authors.push(author.clone());
                            }
                        }
                    }
                }
                PackInfo {
                    name: prefix(package).unwrap_or(&first.name).to_owned(),
                    authors,
                    description: (members.len() == 1)
                        .then(|| first.description.clone())
                        .flatten(),
                    source_url: members.iter().find_map(|m| m.source_url.clone()),
                    released: members
                        .iter()
                        .find_map(|m| m.credits.as_ref()?.released.clone()),
                    ..PackInfo::default()
                }
            }
            _ => return None,
        };
        if rel.is_some()
            && let Some(curated) = curated
        {
            info.images.clone_from(&curated.images);
            info.released.clone_from(&curated.released);
        }
        info.images = s.gallery(Target::Pack, id, &info.images);
        let year = rel
            .and_then(|rel| rel.pack.year)
            .or_else(|| info.released.as_deref()?.get(..4)?.parse().ok());
        let version = rel
            .and_then(|rel| rel.pack.version.clone())
            .or_else(|| first?.version.clone());
        let mut seen = HashSet::new();
        let size = members
            .iter()
            .flat_map(|m| &m.layers)
            .filter(|l| seen.insert(l.sha256))
            .map(|l| l.size)
            .sum();
        Some(ModPackSummary {
            id: id.to_owned(),
            info,
            uploader,
            version,
            year,
            advisory: rel.and_then(|rel| rel.pack.advisory),
            size,
            map_count: members.len(),
            members,
        })
    }

    pub(crate) fn mod_packs_in(&self, s: &CatalogState, public_only: bool) -> Vec<ModPackSummary> {
        let mut items: Vec<_> = s
            .mod_index
            .members
            .keys()
            .filter_map(|id| self.mod_pack_in(s, id, public_only))
            .collect();
        items.sort_by_cached_key(|p| {
            std::cmp::Reverse(p.members.iter().map(|m| m.position).max().unwrap_or(0))
        });
        items
    }

    async fn publish_mod_pack(
        &self,
        bearer: Option<&str>,
        id: &str,
        mut pack: ModPackDocument,
    ) -> ApiResult<(u64, ModPackSummary)> {
        let (_, owner) = self.uploader(bearer)?;
        pack.name = pack.name.trim().to_owned();
        pack.authors = Credit::authors(pack.authors).authors;
        pack.validate().map_err(invalid)?;
        if !maps::pack_id_ok(id) {
            return Err(invalid("invalid pack ID"));
        }
        let _guard = self.content_writes.lock().await;
        {
            let s = self.catalog.read();
            let state = s.standing(Target::Pack, id);
            if matches!(state, Standing::Hidden | Standing::Blocked) {
                return Err(gone("an admin took down this pack"));
            }
            if s.mod_pack_owner(id).is_some_and(|who| who != owner) {
                return Err(forbidden("only this pack's uploader can publish it"));
            }
            if !s.mod_pack_exists(id)
                && ((id.starts_with("p-") && id != mod_pack_id_for(owner, &pack.name))
                    || id.starts_with("c-")
                    || id.starts_with("s-")
                    || s.packs.contains_key(id))
            {
                return Err(conflict("this pack ID is reserved"));
            }
            let mut bases = HashSet::new();
            for map in &pack.members {
                let p = s
                    .maps
                    .get(map)
                    .ok_or_else(|| not_found(format!("member {map} is not published")))?;
                if p.author != owner {
                    return Err(forbidden("every member must belong to the pack uploader"));
                }
                if let Some(source) = s.mod_pack_id(map)
                    && source != id
                    && (!s.standing(Target::Pack, &source).is_ok()
                        || s.mod_index.published.contains_key(&source))
                {
                    return Err(conflict(format!(
                        "member {map} belongs to another published or moderated pack"
                    )));
                }
                if p.doc.staged_for.as_deref().is_some_and(|key| key != id) {
                    return Err(conflict("member was staged for another pack"));
                }
                // Moderation may have changed since this member was staged.
                // A deleted pack's owner may restore its own members.
                let restoring_member =
                    state == Standing::Deleted && s.mod_pack_id(map).as_deref() == Some(id);
                if !restoring_member && s.uses_withheld_content(&p.doc) {
                    return Err(gone("an admin took down the pack containing this content"));
                }
                if !s.map_standing(map).is_ok()
                    || p.doc
                        .manifest
                        .patches
                        .iter()
                        .chain(&p.doc.manifest.packs)
                        .any(|l| !s.servable(&l.sha256))
                {
                    return Err(gone("a member or layer is unavailable"));
                }
                if !bases.insert(p.doc.manifest.base.map.to_ascii_lowercase()) {
                    return Err(invalid("each member must have a distinct base map"));
                }
            }
            if state.is_ok()
                && s.mod_index
                    .published
                    .get(id)
                    .is_some_and(|prev| prev.pack == pack)
                && let Some(summary) = self.mod_pack_in(&s, id, true)
            {
                return Ok((self.position(), summary));
            }
        }
        let release = ModPackPublished {
            id: id.to_owned(),
            uploader: owner,
            pack,
        };
        let event = self.server_event(
            maps::pack_stream(id),
            maps::MOD_PACK_PUBLISHED,
            serde_json::to_value(release).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        let position = recorded[0].position;
        self.wait_applied(position).await?;
        let summary = self
            .mod_pack_in(&self.catalog.read(), id, true)
            .ok_or_else(|| unavailable("pack was not applied"))?;
        Ok((position, summary))
    }
}

pub(crate) async fn list(State(live): Shared, headers: HeaderMap) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let items = live.mod_packs_in(&live.catalog.read(), true);
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"items":items,"position":live.position()}),
    ))
}

pub(crate) async fn get(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let s = live.catalog.read();
    if !s.standing(Target::Pack, &id).is_ok() {
        return Err(gone("an admin took down this pack"));
    }
    let pack = live
        .mod_pack_in(&s, &id, true)
        .ok_or_else(|| not_found("no such pack"))?;
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"pack":pack,"position":live.position()}),
    ))
}

pub(crate) async fn put(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let pack =
        serde_json::from_slice(&body_text(body)?).map_err(|e| invalid(format!("pack: {e}")))?;
    let (position, pack) = live.publish_mod_pack(bearer(&headers), &id, pack).await?;
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"pack":pack,"position":position}),
    ))
}
