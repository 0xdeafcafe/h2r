use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::app::{TagAction, TagWrite};
use agent_proto::{RpcError, hot::method as hot, sim::SessionView};
use map::TagId;
use scenario_edit::layout::BLOCKS;
use tag_sync::{CREATION, RELOAD, TagBaseline};

#[derive(Clone, Debug, PartialEq)]
pub enum Apply {
    Send,
    Confirm(pair::PeerConfirmation),
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

const RELOAD_TIMEOUT: Duration = Duration::from_secs(30);
static NEXT_COPY: AtomicU64 = AtomicU64::new(1);

pub(super) struct ReloadCopy {
    id: u64,
    world: u64,
    before_epoch: u64,
    checksum: u32,
    file: PathBuf,
    placements: Baseline,
    tags: TagBaseline,
    pub(super) resume_edits: bool,
    pub(super) check_identity: bool,
    deadline: Instant,
}

impl ReloadCopy {
    /// A queued copy is ready when edit.map sees its checksum at a new epoch.
    fn reload_complete(
        &self,
        app: &App,
        name: Option<&str>,
        epoch: Option<u64>,
        checksum: Option<u32>,
    ) -> bool {
        self.check_identity
            && epoch.is_some_and(|epoch| epoch != self.before_epoch)
            && checksum == Some(self.checksum)
            && self.world == app.session.world_rev()
            && pair::map_match(app, name) == MapMatch::Same
    }

    fn acknowledged(&self, app: &App, reply: &Value) -> bool {
        let Some(file) = reply["file"].as_str().map(std::path::Path::new) else {
            return false;
        };
        reply["queued"] == true
            && reply["checksum"].as_u64() == Some(u64::from(self.checksum))
            && pair::map_match(app, reply["map"].as_str()) == MapMatch::Same
            && (file == self.file.as_path()
                || matches!((file.canonicalize(), self.file.canonicalize()), (Ok(a), Ok(b)) if a == b))
    }
}

#[derive(Default)]
pub(super) struct Edits {
    pub next_pair: Option<(Endpoint, pair::Source)>,
    pub own_start: bool,
    pub map_requested: bool,
    pub(super) epoch: Option<u64>,
    loaded: Option<std::sync::Arc<HashSet<TagId>>>,
    last_diff: Option<(u64, u64, u64, bool)>,
    pub(super) checksum: Option<u32>,
    name: Option<String>,
    world: Option<u64>,
    session_reason: Option<String>,
    pub peers: Option<u32>,
    pub(super) confirmed: Option<u32>,
    pub(super) journal_ready: bool,
    journal_requested: bool,
    reverting: bool,
    restore_after_batch: bool,
    taking: Option<u64>,
    tags: TagBaseline,
    pub(super) sent: Option<Batch>,
    next: u64,
    conflicts: Vec<Value>,
    refused: issues::Refusals,
    pub(super) created: Vec<sync::Write>,
}

impl Edits {
    pub(super) fn interrupted_write(&self) -> bool {
        self.sent.is_some() || self.reverting || self.restore_after_batch
    }

    pub(super) fn document_changed(&self, app: &App) -> bool {
        self.last_diff.is_none_or(|(world, placements, tags, _)| {
            world != app.session.world_rev()
                || placements != app.session.markers_rev()
                || tags != app.tags.rev()
        })
    }

    pub(super) fn transport_lost(&mut self) {
        self.map_requested = false;
        self.journal_requested = false;
        self.reverting = false;
        self.taking = None;
        self.sent = None;
        self.last_diff = None;
    }
}

type Reply = Result<Value, RpcError>;

impl Play {
    pub(super) fn known_tag_epoch(&self) -> Option<u64> {
        if matches!(self.conn, Conn::Reconnecting { .. }) {
            None
        } else {
            self.edits.loaded.as_ref().and(self.edits.epoch)
        }
    }

    pub(super) fn retry_refused_fields(&mut self) {
        self.edits.refused = issues::Refusals::default();
        self.edits.last_diff = None;
    }

    pub fn can_respawn(&self) -> bool {
        self.has_method(agent_proto::sim::method::OBJECT_RESPAWN) && self.objects.can_respawn()
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
        let v = match result {
            Ok(v) => v,
            Err(e) => {
                self.session = None;
                if e.code == agent_proto::code::UNAVAILABLE
                    && (self.reload_copy.is_some()
                        || (self.waiting == Waiting::Launch && self.pending_spawn.is_some()))
                {
                    // The tag index is unavailable while the game changes maps.
                    self.edits.session_reason = Some(e.message);
                    return;
                }
                if matches!(self.waiting, Waiting::Pair | Waiting::Reload | Waiting::Launch) {
                    self.cancel_reload();
                    app.error(format!("Play: edit.map: {e}"));
                }
                self.edits.session_reason = Some(e.to_string());
                return;
            }
        };
        let epoch = v["epoch"].as_u64();
        let checksum = v["checksum"].as_u64().and_then(|n| u32::try_from(n).ok());
        let name = v["name"].as_str().map(str::to_owned);
        if let Some(copy) = &self.reload_copy
            && !copy.reload_complete(app, name.as_deref(), epoch, checksum)
        {
            // The new epoch and its tag index can become visible on different polls.
            self.session = None;
            return;
        }
        let resume_edits = self.reload_copy.take().map(|copy| {
            app.pairing.play_copy = Some((copy.world, copy.checksum));
            self.launch_baseline = Some((copy.placements, copy.tags));
            self.launched_copy = self.endpoint().cloned().map(|ep| (ep, copy.world, copy.checksum));
            copy.resume_edits
        });
        let reconnecting = matches!(self.conn, Conn::Reconnecting { .. });
        if reconnecting {
            self.conn = Conn::Connected;
            self.retry = None;
        }
        let changed = (reconnecting && (epoch.is_none() || checksum.is_none()))
            || self.edits.epoch != epoch
            || self.edits.checksum != checksum
            || (self.edits.name != name
                && MapMatch::of(self.edits.name.as_deref(), name.as_deref()) != MapMatch::Same)
            || self.edits.world != Some(app.session.world_rev());
        if reconnecting && changed && self.waiting == Waiting::Launch && self.pending_spawn.is_some() {
            self.abandon_spawn(app, "the loaded map changed before the player could be moved");
        }
        self.scenario_tag = v["scenario"].as_str().map(str::to_owned);
        let session =
            serde_json::from_value::<SessionView>(v["session"].clone()).unwrap_or_default();
        self.session = Some(session.kind);
        self.edits.session_reason = session.kind.writes().err().map(str::to_owned).or_else(|| {
            if session.kind == SessionKind::MpHost && session.peers.is_none() {
                Some("the game cannot verify the multiplayer peer count".into())
            } else {
                (!session.writes).then(|| session.why.unwrap_or_else(|| "game refused edits".into()))
            }
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
            if resume_edits.is_none() {
                self.playtest_consent = false;
            }
            app.sync_tags();
            app.tags.live_datums = None;
            if reconnecting || self.edits.epoch.is_some() {
                self.edits.own_start = false;
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
            self.edits.refused = issues::Refusals::default();
            self.edits.created.clear();
            self.edits.confirmed = None;
            self.edits.journal_ready = false;
            self.edits.reverting = false;
            app.pairing.journal.clear();
            self.reset_baseline(app);
            self.edits.journal_requested = false;
        }
        if let Some(epoch) = epoch {
            self.request_journal(epoch);
        }
        if let Some(resume_edits) = resume_edits {
            // The poll that triggered edit.map may still describe the old level.
            self.spawn_after_tick = self.status.as_ref().map_or(0, |s| s.at.ticks + SPAWN_AFTER_TICKS);
            self.status = None;
            self.waiting = Waiting::Launch;
            self.arm_spawn_wait();
            self.edits.own_start = resume_edits;
            self.playtest_consent = true;
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

    pub(super) fn map_refusal(&self, app: &App) -> Option<String> {
        if !self.is_paired() {
            return Some("not paired".into());
        }
        if let Err(why) = self.require_method(hot::EDIT_MAP) {
            return Some(why);
        }
        let open = app.loaded_map();
        if open.is_none() {
            return Some("no map is open".into());
        }
        if self.map_match(app) != MapMatch::Same
            || pair::map_match(app, self.edits.name.as_deref()) != MapMatch::Same
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
            .or_else(|| self.session_refusal(app))
            .or_else(|| self.require_method(hot::EDIT_BATCH).err())
    }

    pub(super) fn session_refusal(&self, app: &App) -> Option<String> {
        self.require_method(hot::EDIT_MAP).err()
            .or_else(|| self.edits.session_reason.clone())
            .or_else(|| self.require_method(hot::EDIT_CHANGES).err())
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

    fn request_journal(&mut self, epoch: u64) {
        if !self.edits.journal_ready
            && !self.edits.journal_requested
            && self.has_method(hot::EDIT_CHANGES)
        {
            self.edits.journal_requested = true;
            self.call(hot::EDIT_CHANGES, Value::Null, Purpose::Journal(epoch));
        }
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
            Ok(_) => {
                self.waiting = Waiting::Idle;
                app.error("game returned an unreadable edit journal");
            }
            Err(e) => {
                self.waiting = Waiting::Idle;
                app.error(format!("edit.changes: {e}"));
            }
        }
    }

    pub(super) fn push_edits(&mut self, app: &mut App) {
        self.refresh_pending(app, self.live_edits);
    }

    pub(super) fn refresh_pending(&mut self, app: &mut App, send: bool) {
        let writable = send && self.reload_copy.is_none() && self.edit_refusal(app).is_none();
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
        if self.edits.world != Some(app.session.world_rev()) {
            self.edits.refused = issues::Refusals::default();
            self.edits.conflicts.clear();
        }
        let empty = TagBaseline::default();
        let (mut tags, mut pending) = tag_sync::diff(
            if base.is_some() {
                &self.edits.tags
            } else {
                &empty
            },
            &app.tags,
        );
        let tag = json!(self.scenario_tag);
        for b in &self.reload {
            let mut row = tag_sync::row(
                "scenario",
                tag.clone(),
                &format!("{b:?}"),
                RELOAD,
                "placement layout changed",
            );
            row["target"] = json!({"panel": "pending"});
            pending.push(row);
        }
        d.writes.retain(|w| {
            let def = scenario_edit::layout::block(w.block).field(w.field);
            let absent = def.is_some_and(|f| f.ty == scenario_edit::FieldType::TagRef)
                && sync::unhex(&w.value)
                    .and_then(|b| Some(u32::from_le_bytes(b.get(4..8)?.try_into().ok()?)))
                    .is_some_and(|d| d != u32::MAX && !app.tags.resident(TagId(d)));
            let mut row = tag_sync::row(
                "scenario",
                tag.clone(),
                &w.path,
                if absent { RELOAD } else { CREATION },
                if absent {
                    "reference target is absent from the loaded metadata"
                } else {
                    "shows when the object is made"
                },
            );
            row["target"] = json!({"block": crate::api::block_name(w.block), "index": w.index});
            row["field"] = json!(w.field);
            pending.push(row);
            !absent
        });
        self.edits.refused.mark(&mut pending, &tags, &d.writes);
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
                row["why"] = c["why"].clone();
                row["holds"] = c["holds_bytes"].clone();
            } else {
                let mut conflict = c.clone();
                let target = &c["target"];
                let placement_moved = target["block"].is_string()
                    && crate::api::P(target).block().is_ok_and(|block| {
                        !base.zip(scenario).is_some_and(|(b, s)| b.layout_matches(s, block))
                    });
                let field_moved = target["path"].as_str().is_some_and(|path| {
                    crate::app::tag_api::lookup(&app.tags, &target["tag"])
                        .and_then(|tag| crate::app::tag_api::field_focus(&app.tags, tag, path))
                        .map_or(true, |focus| c["at"] != focus.address)
                });
                if placement_moved || field_moved {
                    conflict["target"] = json!({"panel": "pending"});
                }
                pending.push(conflict);
            }
        }
        self.objects.publish(&mut pending);
        app.pairing.pending = pending;
        if !writable || self.edits.sent.is_some() || self.edits.taking.is_some()
            || !self.edits.conflicts.is_empty()
        {
            return;
        }
        self.edits.refused.exclude_unchanged(&mut tags, &mut d.writes);
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
            hot::EDIT_BATCH,
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
        self.objects.field_answer();
        let playtest = self.waiting == Waiting::Sent;
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
                        let def = scenario_edit::layout::block(w.block);
                        if def.object_type.is_none() || def.field(w.field).is_some_and(|f| {
                            matches!(f.role, scenario_edit::Role::Position | scenario_edit::Role::Rotation
                                | scenario_edit::Role::Facing)
                        }) {
                            continue;
                        }
                        self.edits
                            .created
                            .retain(|p| p.block != w.block || p.index != w.index);
                        self.edits.created.push(w);
                    }
                }
                if playtest {
                    self.playtest_consent = true;
                    self.finish_playtest();
                }
            }
            Err(e) if e.code == agent_proto::code::MISMATCH => {
                self.waiting = Waiting::Idle;
                let message = e.message;
                self.edits.conflicts = e
                    .data
                    .and_then(|d| d["fields"].as_array().cloned())
                    .unwrap_or_default();
                for c in &mut self.edits.conflicts {
                    c["conflict"] = json!(true);
                    c["why"] = json!(message);
                    if let Some(w) = batch
                        .tags
                        .iter()
                        .find(|w| Some(u64::from(w.at)) == c["at"].as_u64())
                    {
                        c["path"] = json!(w.path);
                        c["tag"] = json!(w.tag.0);
                        c["doc"] = json!("tags");
                        c["class"] = json!(w.class);
                        if let Some(row) = app.pairing.pending.iter().find(|r| r["at"] == c["at"]) {
                            c["target"] = row["target"].clone();
                        }
                    } else {
                        c["doc"] = json!("scenario");
                        c["class"] = json!(CREATION);
                        if let Some(w) = batch.placements.iter().find(|w| c["path"] == w.path.as_str()) {
                            c["target"] = json!({"block": crate::api::block_name(w.block), "index": w.index});
                            c["field"] = json!(w.field);
                        }
                    }
                }
                if !playtest && self.edits.conflicts.is_empty() {
                    self.live_edits = false;
                }
                app.error("the game changed a field; resolve it in Pending");
            }
            other => {
                self.waiting = Waiting::Idle;
                if !playtest {
                    self.live_edits = false;
                }
                let why = other.err().map_or_else(
                    || "incomplete acknowledgement".to_owned(),
                    |e| e.data.as_ref().and_then(|data| data["why"].as_str())
                        .map(str::to_owned).unwrap_or(e.message),
                );
                self.edits.refused.record(&batch.tags, &batch.placements, &why);
                app.error(format!("edit.batch: {why}"));
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
        if let Some((endpoint, source)) = next {
            self.request_pair(app, endpoint, source);
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
                self.edits.refused = issues::Refusals::default();
                self.edits.last_diff = None;
                self.next_push = Instant::now();
                app.pairing.journal.clear();
                self.edits.journal_ready = false;
                self.request_journal(epoch);
            }
        }
    }

    pub(super) fn peer_confirmation(&self) -> Option<pair::PeerConfirmation> {
        if !self.is_paired()
            || self.session != Some(SessionKind::MpHost)
            || self.edits.session_reason.is_some()
        {
            return None;
        }
        Some(pair::PeerConfirmation {
            pairing: self.pairing_id,
            epoch: self.edits.epoch?,
            peers: self.edits.peers?,
        })
    }

    pub fn apply_edits(&mut self, app: &mut App, action: Apply) -> Result<(), String> {
        if let Apply::Confirm(scope) = action {
            if self.peer_confirmation() != Some(scope) {
                return Err("the paired host, map epoch or peer count changed".into());
            }
            self.edits.confirmed = Some(scope.peers);
            self.next_push = Instant::now();
            return Ok(());
        }
        if action == Apply::Keep {
            self.finish_leave(app);
            return Ok(());
        }
        if matches!(action, Apply::Revert | Apply::Restore) {
            self.require_method(hot::EDIT_REVERT)?;
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
                            hot::EDIT_GET,
                            json!({"epoch":epoch,"tag":self.scenario_tag,"group":"scnr","path":c["path"]}),
                        )
                    } else {
                        (hot::EDIT_READ, json!({"epoch":epoch,"at":at,"len":len}))
                    };
                    self.require_method(method)?;
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
                    hot::EDIT_REVERT,
                    json!({"epoch":epoch,"confirm_peers":self.edits.confirmed}),
                    Purpose::Revert {
                        leave: action == Apply::Restore,
                        epoch,
                    },
                );
            }
            Apply::Send => {
                if let Some(why) = self.edit_refusal(app) {
                    return Err(why);
                }
                self.retry_refused_fields();
                if let Some(copy) = &mut self.reload_copy {
                    copy.resume_edits = true;
                }
                self.live_edits = true;
            }
            Apply::Reload | Apply::Respawn => {
                if self.waiting == Waiting::Reload {
                    return Err("waiting for the requested map to load".into());
                }
                if let Some(why) = self.session_refusal(app) {
                    return Err(why);
                }
                if !self.live_edits {
                    return Err("View mode".into());
                }
                if action == Apply::Respawn {
                    self.require_method(agent_proto::sim::method::OBJECT_RESPAWN)?;
                    self.respawn_objects(app)?;
                } else {
                    self.remember_place(app);
                    let route = playtest::Reload::route(
                        self.has_method(agent_proto::sim::method::MAP_RELOAD),
                        self.own_game(),
                    );
                    self.reload_for_play(app, route)?;
                }
            }
            Apply::Keep | Apply::Confirm(_) => unreachable!(),
        }
        self.edits.last_diff = None;
        self.next_push = Instant::now();
        Ok(())
    }

    pub(super) fn cancel_reload(&mut self) {
        self.reload_copy = None;
        self.pending_spawn = None;
        self.spawn_deadline = None;
        self.waiting = Waiting::Idle;
    }

    pub(super) fn expire_reload(&mut self, app: &mut App, now: Instant) {
        if self.reload_copy.as_ref().is_some_and(|copy| now >= copy.deadline) {
            self.cancel_reload();
            // Require another identity poll before a write to the current game.
            self.session = None;
            app.error("Play: timed out waiting for the game to load the requested map copy");
        }
    }

    pub(super) fn reload_answer(&mut self, app: &mut App, id: u64, result: Reply) {
        let Some(copy) = self.reload_copy.as_mut().filter(|copy| copy.id == id) else {
            return;
        };
        match result {
            Ok(v) if copy.acknowledged(app, &v) => {
                copy.check_identity = true;
            }
            Err(e) if e.code == agent_proto::sim::REFUSED => {
                let why = e.data.as_ref().and_then(|v| v["why"].as_str()).unwrap_or(&e.message);
                let destination = self.pending_spawn;
                let resume_edits = copy.resume_edits;
                self.cancel_reload();
                if self.own_game() {
                    app.info(format!("map.reload: {why}; relaunching the game"));
                    if let Err(why) = self.relaunch_copy(app, destination, resume_edits) {
                        app.error(format!("Play: {why}"));
                    }
                } else {
                    app.error(format!("map.reload: {why}. Save the map and load it in the game."));
                }
            }
            other => {
                let why = match other {
                    Ok(v) if v["queued"] == false => "the game did not queue the reload".into(),
                    Ok(_) => "the game did not acknowledge the requested map copy".into(),
                    Err(e) => e.to_string(),
                };
                self.cancel_reload();
                app.error(format!("map.reload: {why}"));
            }
        }
    }

    fn relaunch_copy(
        &mut self,
        app: &mut App,
        destination: Option<Destination>,
        resume_edits: bool,
    ) -> Result<(), String> {
        self.launch_copy(app)?;
        self.pending_spawn = destination;
        self.live_edits = resume_edits;
        self.edits.own_start = resume_edits;
        Ok(())
    }

    pub(super) fn reload_for_play(
        &mut self,
        app: &mut App,
        route: playtest::Reload,
    ) -> Result<(), String> {
        if route == playtest::Reload::Manual {
            return Err(playtest::MANUAL_RELOAD.into());
        }
        if route == playtest::Reload::Relaunch {
            let destination = self.destination(app);
            return self.relaunch_copy(app, destination, self.live_edits);
        }
        self.require_method(agent_proto::sim::method::MAP_RELOAD)?;
        let level = app.loaded_map().ok_or("open a map first")?;
        let scenario = app.session.scenario().ok_or("open a map first")?;
        let before_epoch = self.edits.epoch.ok_or("waiting for the loaded map epoch")?;
        let id = NEXT_COPY.fetch_add(1, Ordering::Relaxed);
        let dir = launch::scratch().join(format!("reload-{id}"));
        let file = launch::write_copy(&app.session, level, &dir)?;
        let saved = scenario_edit::MapSource::file(file.clone())
            .open()
            .map_err(|e| e.to_string())?;
        self.reload_copy = Some(ReloadCopy {
            id,
            world: app.session.world_rev(),
            before_epoch,
            checksum: saved.header().checksum,
            file: file.clone(),
            placements: Baseline::of(scenario),
            tags: TagBaseline::of(&app.tags),
            resume_edits: self.live_edits,
            check_identity: false,
            deadline: Instant::now() + RELOAD_TIMEOUT,
        });
        self.play_world = app.session.world_rev();
        self.pending_spawn = self.destination(app);
        self.waiting = Waiting::Reload;
        if self.paused {
            self.ensure_claim();
            self.resume();
        }
        self.call(
            agent_proto::sim::method::MAP_RELOAD,
            json!({"file": file, "confirm_peers": self.edits.confirmed}),
            Purpose::Reload(id),
        );
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
            if app.tags.read_metadata(at, bytes.len() as u32).as_ref() != Some(&bytes) {
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
