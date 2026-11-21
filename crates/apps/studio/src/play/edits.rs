use super::*;
use crate::app::{TagAction, TagWrite};
use agent_proto::{RpcError, sim::SessionView};
use map::TagId;
use scenario_edit::layout::BLOCKS;
use tag_sync::{CREATION, RELOAD, TagBaseline};

#[derive(Clone, Debug, PartialEq)]
pub enum Apply {
    Send,
    Confirm(u32),
    Adopt,
    Revert,
    Keep,
    Restore,
    Take(u32),
    Overwrite(u32),
    Reload,
    Respawn,
}

pub(super) struct Batch {
    id: u64,
    tags: Vec<tag_sync::Write>,
    placements: Vec<sync::Write>,
}

#[derive(Default)]
pub(super) struct Edits {
    pub next_pair: Option<Endpoint>,
    reload_copy: Option<(u64, u32, Baseline, TagBaseline)>,
    pub own_start: bool,
    pub map_requested: bool,
    pub respawn: bool,
    pub reload: bool,
    pub(super) epoch: Option<u64>,
    loaded: Option<std::sync::Arc<HashSet<TagId>>>,
    last_diff: Option<(u64, u64, u64, bool)>,
    checksum: Option<u32>,
    name: Option<String>,
    world: Option<u64>,
    session_reason: Option<String>,
    pub peers: Option<u32>,
    confirmed: Option<u32>,
    pub(super) journal_ready: bool,
    journal_requested: bool,
    reverting: bool,
    restore_after_batch: bool,
    taking: Option<u64>,
    tags: TagBaseline,
    pub(super) sent: Option<Batch>,
    next: u64,
    conflicts: Vec<Value>,
    created: Vec<sync::Write>,
}

type Reply = Result<Value, RpcError>;

impl Play {
    pub(super) fn known_tag_epoch(&self) -> Option<u64> {
        self.edits.loaded.as_ref().and(self.edits.epoch)
    }

    pub fn can_respawn(&self) -> bool {
        self.edits.respawn && !self.edits.created.is_empty()
    }

    fn reset_baseline(&mut self, app: &App) {
        let copy = app.pairing.play_copy.is_some_and(|(rev, sum)| {
            rev == app.session.world_rev() && Some(sum) == self.edits.checksum
        });
        let launch = self.launch_baseline.as_ref().filter(|_| copy);
        self.edits.tags = launch.map(|(_, tags)| tags.clone()).unwrap_or_default();
        self.baseline = app.loaded_map().zip(app.session.scenario()).map(|(m, s)| {
            (
                m.into(),
                launch
                    .map(|(b, _)| b.clone())
                    .unwrap_or_else(|| Baseline::source(s)),
            )
        });
        self.edits.world = Some(app.session.world_rev());
        self.edits.last_diff = None;
    }

    pub(super) fn edit_map(&mut self, app: &mut App, result: Reply) {
        self.edits.map_requested = false;
        let Ok(v) = result else {
            self.session = None;
            self.edits.session_reason = Some(result.unwrap_err().to_string());
            return;
        };
        let epoch = v["epoch"].as_u64();
        let checksum = v["checksum"].as_u64().and_then(|n| u32::try_from(n).ok());
        let name = v["name"].as_str().map(str::to_owned);
        if self.edits.epoch != epoch
            && self
                .edits
                .reload_copy
                .as_ref()
                .is_some_and(|(world, sum, _, _)| {
                    *world == app.session.world_rev()
                        && Some(*sum) == checksum
                        && MapMatch::of(app.loaded_map(), name.as_deref()) == MapMatch::Same
                })
            && let Some((world, sum, base, tags)) = self.edits.reload_copy.take()
        {
            app.pairing.play_copy = Some((world, sum));
            self.launch_baseline = Some((base, tags));
            self.launched_copy = self.endpoint().cloned().map(|ep| (ep, world, sum));
        }
        let changed = self.edits.epoch != epoch
            || self.edits.checksum != checksum
            || (self.edits.name != name
                && MapMatch::of(self.edits.name.as_deref(), name.as_deref()) != MapMatch::Same)
            || self.edits.world != Some(app.session.world_rev());
        self.scenario_tag = v["scenario"].as_str().map(str::to_owned);
        let session =
            serde_json::from_value::<SessionView>(v["session"].clone()).unwrap_or_default();
        self.session = Some(session.kind);
        self.edits.session_reason = session.kind.writes().err().map(str::to_owned).or_else(|| {
            (!session.writes).then(|| session.why.unwrap_or_else(|| "game refused edits".into()))
        });
        self.edits.peers = (session.kind == SessionKind::MpHost)
            .then_some(session.peers)
            .flatten()
            .filter(|n| *n > 1);
        if self.edits.confirmed != self.edits.peers {
            self.edits.confirmed = None;
        }
        if epoch != self.edits.epoch {
            self.edits.loaded = None;
        }
        if let Some(ids) = v["loaded"].as_array() {
            self.edits.loaded = Some(std::sync::Arc::new(
                ids.iter()
                    .filter_map(|id| Some(TagId(u32::try_from(id.as_u64()?).ok()?)))
                    .collect(),
            ));
            self.edits.last_diff = None;
        }
        if changed {
            app.sync_tags();
            app.tags.live_datums = None;
            if self.edits.epoch.is_some() {
                self.live_edits = false;
            }
            self.edits.epoch = epoch;
            self.edits.checksum = checksum;
            self.edits.name = name;
            self.edits.sent = None;
            self.edits.taking = None;
            if std::mem::take(&mut self.edits.restore_after_batch) {
                app.error("the map changed while leaving; choose how to leave the current map");
            }
            self.edits.conflicts.clear();
            self.edits.created.clear();
            self.edits.confirmed = None;
            self.edits.journal_ready = false;
            self.edits.reverting = false;
            app.pairing.journal.clear();
            self.reset_baseline(app);
            self.edits.journal_requested = false;
        }
        if let Some(epoch) = epoch
            && !self.edits.journal_ready
            && !self.edits.journal_requested
        {
            self.edits.journal_requested = true;
            self.call("edit.changes", Value::Null, Purpose::Journal(epoch));
        }
        if self.map_refusal(app).is_none() {
            app.tags.live_datums = self.edits.loaded.clone();
        }
        if self.map_refusal(app).is_some() || self.edits.session_reason.is_some() {
            self.live_edits = false;
        } else if self.edits.own_start && self.edits.journal_ready {
            self.edits.own_start = false;
            self.live_edits = app.pairing.journal.is_empty();
        }
    }

    fn map_refusal(&self, app: &App) -> Option<String> {
        if !self.is_paired() {
            return Some("not paired".into());
        }
        let open = app.loaded_map();
        if open.is_none() {
            return Some("no map is open".into());
        }
        if self.map_match(app) != MapMatch::Same
            || MapMatch::of(open, self.edits.name.as_deref()) != MapMatch::Same
        {
            return Some(format!(
                "the game runs {}, the editor has {}",
                self.map().unwrap_or("?"),
                open.unwrap()
            ));
        }
        let expected = app
            .pairing
            .play_copy
            .filter(|(rev, _)| *rev == app.session.world_rev())
            .map(|(_, s)| s)
            .or_else(|| app.session.source_checksum());
        if expected.is_none() || expected != self.edits.checksum {
            return Some("map checksum differs or is unavailable".into());
        }
        if self.edits.epoch.is_none()
            || self.edits.world != Some(app.session.world_rev())
            || self.scenario_tag.is_none()
        {
            return Some("waiting for the loaded map".into());
        }
        if let Some(sample) = &app.pairing.shown
            && Some(sample.epoch) != self.edits.epoch
        {
            return Some("waiting for the current map epoch".into());
        }
        None
    }

    pub(super) fn edit_refusal(&self, app: &App) -> Option<String> {
        self.map_refusal(app)
            .or_else(|| self.edits.session_reason.clone())
            .or_else(|| (self.session.is_none()).then(|| "waiting for the session".into()))
            .or_else(|| (!self.edits.journal_ready).then(|| "checking the game journal".into()))
            .or_else(|| {
                (!app.pairing.journal.is_empty())
                    .then(|| "adopt or revert the game journal in Pending".into())
            })
            .or_else(|| {
                self.edits
                    .peers
                    .filter(|n| Some(*n) != self.edits.confirmed)
                    .map(|n| {
                        format!(
                            "confirm host-only edits in Pending ({} other machines)",
                            n - 1
                        )
                    })
            })
            .or_else(|| {
                (app.pairing.leaving || self.edits.reverting)
                    .then(|| "finishing the pairing".into())
            })
    }

    pub(super) fn edit_journal(&mut self, app: &mut App, epoch: u64, result: Reply) {
        if self.edits.epoch != Some(epoch) {
            return;
        }
        self.edits.journal_requested = false;
        match result {
            Ok(v) if v["changes"].is_array() => {
                app.pairing.journal = v["changes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|entry| {
                        entry["fields"]
                            .as_array()
                            .map(Vec::as_slice)
                            .unwrap_or_else(|| std::slice::from_ref(entry))
                            .iter()
                    })
                    .filter(|r| r["stale"] != true)
                    .cloned()
                    .collect();
                self.edits.journal_ready = true;
                if !app.pairing.journal.is_empty() {
                    self.live_edits = false;
                    self.edits.own_start = false;
                }
            }
            Ok(_) => app.error("game returned an unreadable edit journal"),
            Err(e) => app.error(format!("edit.changes: {e}")),
        }
    }

    pub(super) fn push_edits(&mut self, app: &mut App) {
        let writable = self.edits_blocked(app).is_none();
        let revisions = (
            app.session.world_rev(),
            app.session.markers_rev(),
            app.tags.rev(),
            writable,
        );
        if self.edits.last_diff.replace(revisions) == Some(revisions) {
            return;
        }
        let scenario = app.session.scenario();
        let source;
        let base = if self.edits.world == Some(app.session.world_rev()) {
            self.baseline.as_ref().map(|(_, b)| b)
        } else {
            None
        };
        let mut d = if let Some(s) = scenario {
            let b = match base {
                Some(b) => b,
                None => {
                    source = Baseline::source(s);
                    &source
                }
            };
            sync::diff(b, s)
        } else {
            sync::Diff::default()
        };
        self.reload = d.reload;
        let empty = TagBaseline::default();
        let (tags, mut pending) = tag_sync::diff(
            if base.is_some() {
                &self.edits.tags
            } else {
                &empty
            },
            &app.tags,
        );
        let tag = json!(self.scenario_tag);
        for b in &self.reload {
            pending.push(tag_sync::row(
                "scenario",
                tag.clone(),
                &format!("{b:?}"),
                RELOAD,
                "placement count changed",
            ));
        }
        d.writes.retain(|w| {
            let def = scenario_edit::layout::block(w.block).field(w.field);
            let absent = def.is_some_and(|f| f.ty == scenario_edit::FieldType::TagRef)
                && sync::unhex(&w.value)
                    .and_then(|b| Some(u32::from_le_bytes(b.get(4..8)?.try_into().ok()?)))
                    .is_some_and(|d| d != u32::MAX && !app.tags.resident(TagId(d)));
            pending.push(tag_sync::row(
                "scenario",
                tag.clone(),
                &w.path,
                if absent { RELOAD } else { CREATION },
                if absent {
                    "reference target is absent from the loaded metadata"
                } else {
                    "shows when the object is made"
                },
            ));
            !absent
        });
        for c in &self.edits.conflicts {
            let same = |r: &&mut Value| {
                if r["at"].is_number() {
                    r["at"] == c["at"]
                } else {
                    r["doc"] == c["doc"] && r["path"] == c["path"]
                }
            };
            if let Some(row) = pending.iter_mut().find(same) {
                row["at"] = c["at"].clone();
                row["conflict"] = json!(true);
                row["why"] = json!("the game changed this");
                row["holds"] = c["holds_bytes"].clone();
            } else {
                pending.push(c.clone());
            }
        }
        app.pairing.pending = pending;
        if !writable || self.edits.sent.is_some() || !self.edits.conflicts.is_empty() {
            return;
        }
        let mut writes: Vec<_> = tags.iter().map(tag_sync::Write::wire).collect();
        writes.extend(
            d.writes.iter().map(
                |w| json!({"tag":tag,"group":"scnr","path":w.path,"value":w.value,"was":w.was}),
            ),
        );
        if writes.is_empty() {
            return;
        }
        self.edits.next += 1;
        let id = self.edits.next;
        self.call(
            "edit.batch",
            json!({"epoch":self.edits.epoch,"confirm_peers":self.edits.confirmed,"writes":writes}),
            Purpose::Edit(id),
        );
        self.edits.sent = Some(Batch {
            id,
            tags,
            placements: d.writes,
        });
    }

    pub(super) fn batch_answer(&mut self, app: &mut App, id: u64, result: Reply) {
        if self.edits.sent.as_ref().is_none_or(|batch| batch.id != id) {
            return;
        }
        let batch = self.edits.sent.take().unwrap();
        if self.edits.world != Some(app.session.world_rev()) {
            self.edits.restore_after_batch = false;
            return;
        }
        match result {
            Ok(v)
                if v["applied"].as_u64()
                    == Some((batch.tags.len() + batch.placements.len()) as u64) =>
            {
                for w in batch.tags {
                    self.edits.tags.wrote(w.at, &w.value);
                }
                if let Some((_, base)) = &mut self.baseline {
                    for w in batch.placements {
                        base.wrote(&w);
                        self.edits
                            .created
                            .retain(|p| p.block != w.block || p.index != w.index);
                        self.edits.created.push(w);
                    }
                }
            }
            Err(e) if e.code == agent_proto::code::MISMATCH => {
                self.edits.conflicts = e
                    .data
                    .and_then(|d| d["fields"].as_array().cloned())
                    .unwrap_or_default();
                for c in &mut self.edits.conflicts {
                    c["conflict"] = json!(true);
                    c["why"] = json!("the game changed this");
                    if let Some(w) = batch
                        .tags
                        .iter()
                        .find(|w| Some(u64::from(w.at)) == c["at"].as_u64())
                    {
                        c["path"] = json!(w.path);
                        c["tag"] = json!(w.tag.0);
                        c["doc"] = json!("tags");
                        c["class"] = json!(w.class);
                    } else {
                        c["doc"] = json!("scenario");
                        c["class"] = json!(CREATION);
                    }
                }
                if self.edits.conflicts.is_empty() {
                    self.live_edits = false;
                }
                app.error("the game changed a field; resolve it in Pending");
            }
            other => {
                self.live_edits = false;
                app.error(format!(
                    "edit.batch: {}",
                    other
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "incomplete acknowledgement".into())
                ));
            }
        }
        self.edits.last_diff = None;
        self.next_push = Instant::now();
        if std::mem::take(&mut self.edits.restore_after_batch)
            && let Err(why) = self.apply_edits(app, Apply::Restore)
        {
            app.error(why);
        }
    }

    fn finish_leave(&mut self, app: &mut App) {
        let next = self.edits.next_pair.take();
        self.unpair(app);
        if let Some(e) = next {
            self.pair(app, e);
        }
    }

    pub(super) fn request_leave(&mut self, app: &mut App) {
        if self.is_paired() {
            self.live_edits = false;
            app.pairing.leaving = true;
        } else {
            self.unpair(app);
        }
    }

    pub(super) fn reverted(&mut self, app: &mut App, leave: bool, epoch: u64, result: Reply) {
        if Some(epoch) != self.edits.epoch {
            return;
        }
        self.edits.reverting = false;
        match result {
            Err(e) => app.error(format!("edit.revert: {e}")),
            Ok(v) => {
                if v["dropped"].as_array().is_some_and(|a| !a.is_empty()) {
                    app.error(
                        "the game changed some journalled bytes; edit.revert left those alone",
                    );
                }
                if leave {
                    self.finish_leave(app);
                    return;
                }
                if self.edits.world != Some(app.session.world_rev()) {
                    return;
                }
                for row in ["reverted", "dropped"]
                    .iter()
                    .filter_map(|key| v[*key].as_array())
                    .flatten()
                {
                    if row["holds_bytes"].is_string()
                        && let Err(why) = self.accept_game_bytes(app, row, false)
                    {
                        app.error(format!("edit.revert: {why}"));
                    }
                }
                self.edits.created.clear();
                self.edits.conflicts.clear();
                self.edits.last_diff = None;
                self.next_push = Instant::now();
                app.pairing.journal.clear();
                self.edits.journal_ready = false;
                self.edits.journal_requested = true;
                self.call("edit.changes", Value::Null, Purpose::Journal(epoch));
            }
        }
    }

    pub fn apply_edits(&mut self, app: &mut App, action: Apply) -> Result<(), String> {
        if action == Apply::Keep {
            self.finish_leave(app);
            return Ok(());
        }
        if action == Apply::Restore && self.is_paired() {
            self.request_leave(app);
            self.edits.taking = None;
            if self.edits.sent.is_some() {
                self.edits.restore_after_batch = true;
                return Ok(());
            }
        }
        if let Some(why) = self.map_refusal(app) {
            return Err(why);
        }
        if self.edits.sent.is_some() || self.edits.reverting || self.edits.taking.is_some() {
            return Err("waiting for the game to acknowledge the previous write".into());
        }
        match action {
            Apply::Take(at) | Apply::Overwrite(at) => {
                if matches!(action, Apply::Overwrite(_)) {
                    if let Some(why) = self.edit_refusal(app) {
                        return Err(why);
                    }
                    self.live_edits = true;
                }
                let i = self
                    .edits
                    .conflicts
                    .iter()
                    .position(|c| c["at"].as_u64() == Some(u64::from(at)))
                    .ok_or("no conflict at this address")?;
                let c = self.edits.conflicts[i].clone();
                if action == Apply::Take(at) {
                    let len = observed_bytes(&c)?.len();
                    let epoch = self.edits.epoch.unwrap();
                    let (method, params) = if c["doc"] == "scenario" {
                        (
                            "edit.get",
                            json!({"epoch":epoch,"tag":self.scenario_tag,"group":"scnr","path":c["path"]}),
                        )
                    } else {
                        ("edit.read", json!({"epoch":epoch,"at":at,"len":len}))
                    };
                    self.edits.next += 1;
                    let id = self.edits.next;
                    self.edits.taking = Some(id);
                    self.call(method, params, Purpose::Take { id, at });
                    return Ok(());
                }
                self.accept_game_bytes(app, &c, false)?;
                self.edits.conflicts.remove(i);
            }
            Apply::Adopt => {
                let before = (self.baseline.clone(), self.edits.tags.clone());
                for c in app.pairing.journal.clone() {
                    if let Err(e) = self.accept_game_bytes(app, &c, false) {
                        (self.baseline, self.edits.tags) = before;
                        return Err(e);
                    }
                }
                app.pairing.journal.clear();
            }
            Apply::Revert | Apply::Restore => {
                if let Some(why) = &self.edits.session_reason {
                    return Err(why.clone());
                }
                self.live_edits = false;
                self.edits.reverting = true;
                let epoch = self.edits.epoch.unwrap();
                self.call(
                    "edit.revert",
                    json!({"epoch":epoch,"confirm_peers":self.edits.confirmed}),
                    Purpose::Revert {
                        leave: action == Apply::Restore,
                        epoch,
                    },
                );
            }
            Apply::Confirm(peers) if self.edits.peers == Some(peers) => {
                self.edits.confirmed = Some(peers);
            }
            Apply::Confirm(_) => return Err("the peer count changed".into()),
            Apply::Send => {
                if let Some(why) = self.edit_refusal(app) {
                    return Err(why);
                }
                self.live_edits = true;
            }
            Apply::Reload | Apply::Respawn => {
                if let Some(why) = self.edits_blocked(app) {
                    return Err(why);
                }
                if action == Apply::Respawn {
                    if !self.can_respawn() {
                        return Err("no acknowledged placement edits to respawn".into());
                    }
                    for w in &self.edits.created {
                        self.call("world.object.respawn",json!({"placement":{"block":crate::api::block_name(w.block),"index":w.index}}),Purpose::Other);
                    }
                } else if self.child.is_some() && self.endpoint() == self.child_endpoint.as_ref() {
                    self.launch(app);
                } else {
                    let level = app.loaded_map().ok_or("open a map first")?;
                    let dir = launch::scratch().join("reload");
                    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                    let file = dir.join(format!(
                        "{}.map",
                        std::path::Path::new(level)
                            .file_stem()
                            .ok_or("missing map name")?
                            .to_string_lossy()
                    ));
                    scenario_edit::save::save_as(
                        app.session.source().ok_or("no map source")?,
                        app.session.scenario().unwrap(),
                        &file,
                    )
                    .map_err(|e| e.to_string())?;
                    let saved = scenario_edit::MapSource::file(file.clone())
                        .open()
                        .map_err(|e| e.to_string())?;
                    self.edits.reload_copy = Some((
                        app.session.world_rev(),
                        saved.header().checksum,
                        Baseline::of(app.session.scenario().unwrap()),
                        TagBaseline::of(&app.tags),
                    ));
                    app.info(format!(
                        "Play copy saved to {}. Load it in the game if map.reload is unavailable.",
                        file.display()
                    ));
                    if !self.edits.reload {
                        return Ok(());
                    }
                    self.call("map.reload", json!({"file":file}), Purpose::Other);
                }
            }
            Apply::Keep => unreachable!(),
        }
        self.edits.last_diff = None;
        self.next_push = Instant::now();
        Ok(())
    }

    pub(super) fn take_answer(&mut self, app: &mut App, id: u64, at: u32, result: Reply) {
        if self.edits.taking != Some(id) {
            return;
        }
        self.edits.taking = None;
        let outcome: Result<(), String> = (|| {
            let reply = result.map_err(|e| e.to_string())?;
            if let Some(why) = self.map_refusal(app) {
                return Err(why);
            }
            let i = self
                .edits
                .conflicts
                .iter()
                .position(|c| c["at"].as_u64() == Some(u64::from(at)))
                .ok_or("the conflict has expired")?;
            let mut field = self.edits.conflicts[i].clone();
            let len = observed_bytes(&field)?.len();
            field["holds_bytes"] = if field["doc"] == "scenario" {
                reply["value"].clone()
            } else {
                reply["holds_bytes"].clone()
            };
            let received = field["holds_bytes"].as_str().and_then(sync::unhex);
            if game_address(&reply)? != at || received.as_ref().map(Vec::len) != Some(len) {
                return Err("the game returned a different field".into());
            }
            self.accept_game_bytes(app, &field, true)?;
            self.edits.conflicts.remove(i);
            self.edits.last_diff = None;
            self.next_push = Instant::now();
            Ok(())
        })();
        if let Err(why) = outcome {
            app.error(format!("take the game's value: {why}"));
        }
    }

    fn accept_game_bytes(&mut self, app: &mut App, c: &Value, take: bool) -> Result<(), String> {
        let at = game_address(c)?;
        let bytes = observed_bytes(c)?;
        let scenario = app.session.scenario().ok_or("no open scenario")?;
        // A grown block in a Play copy can live at a different address from
        // the open document. Its scenario path still names the original slot.
        let scenario_row = c["doc"] == "scenario"
            || c["tag"]
                .as_str()
                .is_some_and(|tag| Some(tag) == self.scenario_tag.as_deref());
        if scenario_row
            && let Some(path) = c["path"].as_str()
            && let Some((_, tail)) = path.split_once('[')
            && let Some((index, _)) = tail.split_once(']')
            && let Ok(index) = index.parse::<usize>()
        {
            for b in BLOCKS {
                let Some(f) = b.fields.iter().find(|f| {
                    f.ty.size() as usize == bytes.len() && sync::path(b.kind, index, f) == path
                }) else {
                    continue;
                };
                let (_, base) = self.baseline.as_mut().ok_or("no placement baseline")?;
                if take {
                    if !base.layout_matches(scenario, b.kind) {
                        return Err("reload the changed placement block first".into());
                    }
                    let value = scenario
                        .block(b.kind)
                        .field_value(index, f, &bytes)
                        .ok_or("the placement no longer exists")?;
                    app.session
                        .apply(scenario_edit::Command::set(b.kind, index, f.name, value))
                        .map_err(|e| e.to_string())?;
                }
                base.wrote(&sync::Write {
                    block: b.kind,
                    index,
                    field: f.name,
                    path: path.to_owned(),
                    was: String::new(),
                    value: sync::hex(&bytes),
                });
                return Ok(());
            }
        }
        if c["doc"] == "scenario" {
            return Err("the game returned an unknown placement field".into());
        }
        let mut covered = Vec::new();
        if !take
            && c["doc"] != "tags"
            && let Some((_, base)) = &mut self.baseline
        {
            covered = base.observe(scenario, at, &bytes);
            if covered.first() == Some(&(0..bytes.len())) {
                return Ok(());
            }
        }
        app.sync_tags();
        let entry = app
            .tags
            .tags()
            .iter()
            .find(|t| {
                at >= t.address
                    && u64::from(at) + bytes.len() as u64
                        <= u64::from(t.address) + u64::from(t.size)
            })
            .ok_or("address outside the document's tags")?;
        if take {
            let tag: TagId = entry.id;
            let write = TagWrite {
                tag,
                address: at,
                bytes: bytes.clone(),
                label: "take the game's value".into(),
                merge: None,
            };
            app.run_map_tag(TagAction::Write(write));
            if app.tags.read(at, bytes.len() as u32).as_ref() != Some(&bytes) {
                return Err("the document refused the game value".into());
            }
        }
        let mut start = 0;
        for range in covered {
            if start < range.start {
                self.edits
                    .tags
                    .wrote(at + start as u32, &bytes[start..range.start]);
            }
            start = start.max(range.end);
        }
        if start < bytes.len() {
            self.edits.tags.wrote(at + start as u32, &bytes[start..]);
        }
        Ok(())
    }
}

fn game_address(row: &Value) -> Result<u32, String> {
    row["at"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .or_else(|| u32::from_str_radix(row["addr"].as_str()?.strip_prefix("0x")?, 16).ok())
        .ok_or_else(|| "missing game address".into())
}

fn observed_bytes(row: &Value) -> Result<Vec<u8>, String> {
    row["holds_bytes"]
        .as_str()
        .or(row["holds"].as_str())
        .and_then(sync::unhex)
        .filter(|bytes| !bytes.is_empty())
        .ok_or_else(|| "missing or invalid current game bytes".into())
}
