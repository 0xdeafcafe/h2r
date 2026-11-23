//! Collision live: the Modeling tool's level collision edits sent to a
//! paired game as `edit.collision.stage`, then `edit.collision.commit`
//! (docs/editor/realtime-edits.md, "Collision" and "The editor";
//! docs/runtime/agent-api.md, "Collision edits").
//!
//! The model is [`super::raw_sync`]'s, per structure:
//!
//! * The **baseline** ([`Held`]) is what the game holds for a structure's
//!   level collision beyond its loaded file: the hash of the blocks it was
//!   last committed and the pairs the commit's receipt named. A structure
//!   with no baseline holds the file's blocks at the file's pairs.
//! * A **want** ([`CollisionWant`]) is what the documents say: the blocks
//!   `geometry_edit::level_collision::LevelEdit::stage` makes for a solid
//!   added to the level, beside the file's blocks and pairs. A structure
//!   the baseline holds and the documents no longer change wants the
//!   file's blocks back, so an undo stages them again.
//! * A difference goes as one stage, then one commit naming the pairs the
//!   game holds as `was`. The commit is `append` when no indexed block
//!   shrinks, else `rebuild`. It asks for the BSP activation whenever the
//!   stage's reply says the game has one, so Havok bodies match the
//!   collision rays see.
//!
//! Blocks over [`INLINE_BYTES`] go ahead in `edit.stage.put` chunks, so no
//! request line passes the game's 1 MiB limit. A refusal holds the row
//! under Reload, and a `MISMATCH` holds it as a conflict, until the
//! documents change or the user sends again. Sending again after a
//! mismatch commits against the pairs the game said it holds.
//!
//! The Modeling tool makes no level collision edit yet
//! (docs/editor/modeling.md): `App::set_level_collision` is the seam, and
//! with `Modeling::level_collision` empty nothing is sent.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use agent_proto::hot::{COLLISION_BLOCKS, COLLISION_LEAF_CLUSTERS, COLLISION_MOPP, encode_base64};
use geometry_edit::level_collision::LevelEdit;
use map::TagId;

use super::tag_sync::{self, Effect};
use super::*;

/// Ids of collision requests: apart from the tag batches' and
/// [`super::raw_sync`]'s.
const ID_BASE: u64 = 2 << 48;
/// Bytes a stage sends inline; more go ahead in `edit.stage.put` chunks of
/// this size (base64 makes a third more).
pub(crate) const INLINE_BYTES: usize = 256 << 10;

/// What a Pending row says for a solid appended to the level.
pub(crate) const APPEND_NOTE: &str = "rays see the new collision next tick; bodies after the BSP activation";
/// What a Pending row says when indices change.
pub(crate) const REBUILD_NOTE: &str = "the level rebuilds for a moment";
/// What a Pending row says when the game has no BSP activation.
pub(crate) const NO_ACTIVATION_NOTE: &str =
    "rays see the new collision next tick; bodies keep the old shape until the next BSP switch";

/// Blocks whose indices something else keeps: `append` may not shrink
/// them.
const INDEXED: [&str; 6] = ["planes", "leaves", "surfaces", "edges", "vertices", COLLISION_LEAF_CLUSTERS];

/// Pairs as `was` names them: `(count, address)` by block name.
pub(crate) type Pairs = BTreeMap<&'static str, (u32, u32)>;

/// A structure's level collision as blocks.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Blocks {
    /// The eight collision blocks in [`COLLISION_BLOCKS`] order: count and
    /// bytes.
    pub bsp: Vec<(u32, Vec<u8>)>,
    /// The per-leaf block (`sbsp+0x2c`): count and bytes.
    pub leaves: (u32, Vec<u8>),
    /// The `sbsp+0x1e0` block.
    pub mopp: Vec<u8>,
}

impl Blocks {
    fn hash(&self) -> u64 {
        let mut h = DefaultHasher::new();
        Hash::hash(self, &mut h);
        h.finish()
    }

    fn len(&self) -> usize {
        self.bsp.iter().map(|(_, b)| b.len()).sum::<usize>() + self.leaves.1.len() + self.mopp.len()
    }

    /// A block's count, by the name `was` gives it.
    fn count(&self, name: &str) -> u32 {
        match name {
            COLLISION_LEAF_CLUSTERS => self.leaves.0,
            COLLISION_MOPP => self.mopp.len() as u32,
            _ => COLLISION_BLOCKS
                .iter()
                .position(|n| *n == name)
                .and_then(|i| self.bsp.get(i))
                .map_or(0, |b| b.0),
        }
    }

    /// Every byte, in the order [`stage_params`] names them.
    fn joined(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len());
        for (_, raw) in &self.bsp {
            out.extend_from_slice(raw);
        }
        out.extend_from_slice(&self.leaves.1);
        out.extend_from_slice(&self.mopp);
        out
    }
}

/// One structure's wanted level collision.
#[derive(Clone, Debug)]
pub struct CollisionWant {
    pub tag: TagId,
    /// What the save writes.
    pub blocks: Arc<Blocks>,
    /// What the file holds, staged again to undo.
    pub file: Arc<Blocks>,
    /// The file's pairs.
    pub file_pairs: Pairs,
}

impl CollisionWant {
    /// The want of a level collision edit: its staged blocks, and the
    /// file's blocks and pairs it was read from.
    pub fn of(edit: &LevelEdit) -> Self {
        let new = edit.stage();
        let s = &edit.source;
        let blocks = Blocks {
            bsp: new.blocks.iter().map(|b| (b.count, b.bytes.clone())).collect(),
            leaves: (new.leaves.count, new.leaves.bytes.clone()),
            mopp: new.mopp.clone(),
        };
        let file = Blocks {
            bsp: s
                .blocks
                .iter()
                .zip(&s.block_bytes)
                .map(|(b, bytes)| (b.count, bytes.clone()))
                .collect(),
            leaves: (s.leaves.count, s.level.leaves.clone()),
            mopp: s.level.mopp.clone(),
        };
        let mut file_pairs: Pairs = COLLISION_BLOCKS
            .iter()
            .zip(&s.blocks)
            .map(|(n, b)| (*n, (b.count, b.address)))
            .collect();
        file_pairs.insert(COLLISION_LEAF_CLUSTERS, (s.leaves.count, s.leaves.address));
        file_pairs.insert(COLLISION_MOPP, (s.mopp.count, s.mopp.address));
        CollisionWant {
            tag: s.tag,
            blocks: Arc::new(blocks),
            file: Arc::new(file),
            file_pairs,
        }
    }
}

/// What the game holds for one structure beyond its file.
#[derive(Clone, Debug, PartialEq)]
struct Held {
    /// The hash of the blocks last committed; 0 when only the pairs are
    /// known (a mismatch's, adopted by a send-again).
    hash: u64,
    /// The pairs the game holds.
    pairs: Pairs,
    /// The file's blocks, staged again to undo.
    file: Arc<Blocks>,
}

/// A stage the game holds and has not committed.
#[derive(Clone, Debug, PartialEq)]
struct Staged {
    tag: u32,
    hash: u64,
    stage: u64,
    activation: bool,
}

/// An `edit.stage.put` buffer being filled for the stage of `(tag, hash)`.
#[derive(Clone, Debug, PartialEq)]
struct Put {
    tag: u32,
    hash: u64,
    stage: u64,
    len: usize,
}

/// A request in flight.
#[derive(Debug)]
enum Sent {
    Put {
        id: u64,
        tag: u32,
        hash: u64,
    },
    Stage {
        id: u64,
        tag: u32,
        hash: u64,
    },
    Commit {
        id: u64,
        tag: u32,
        hash: u64,
        was: Pairs,
        file: Arc<Blocks>,
    },
}

impl Sent {
    fn id(&self) -> u64 {
        match self {
            Sent::Put { id, .. } | Sent::Stage { id, .. } | Sent::Commit { id, .. } => *id,
        }
    }

    fn method(&self) -> &'static str {
        match self {
            Sent::Put { .. } => agent_proto::hot::method::EDIT_STAGE_PUT,
            Sent::Stage { .. } => agent_proto::hot::method::EDIT_COLLISION_STAGE,
            Sent::Commit { .. } => agent_proto::hot::method::EDIT_COLLISION_COMMIT,
        }
    }
}

/// Why nothing goes until the documents change or the user sends again.
#[derive(Clone, Debug)]
struct Hold {
    rev: u64,
    why: String,
    conflict: bool,
    /// A mismatch's structure, the pairs the game said it holds and the
    /// file's blocks: adopted when the user sends again.
    holds: Option<(u32, Pairs, Arc<Blocks>)>,
}

/// What a failed answer leaves: why, whether it is a conflict, and a
/// mismatch's pairs.
type Problem = (String, bool, Option<(u32, Pairs, Arc<Blocks>)>);

/// The collision half of a pairing.
#[derive(Default)]
pub(crate) struct CollisionSync {
    held: BTreeMap<u32, Held>,
    staged: Option<Staged>,
    put: Option<Put>,
    sent: Option<Sent>,
    hold: Option<Hold>,
    next: u64,
    /// The wants at a level collision revision of the documents.
    wants: (Option<u64>, Arc<Vec<CollisionWant>>),
}

/// The next request the documents need.
#[derive(Debug, PartialEq)]
pub(crate) enum Step {
    /// Stage `blocks` for `tag` (after `edit.stage.put` chunks when large).
    Stage { tag: u32, hash: u64, blocks: Arc<Blocks> },
    /// Commit the staged blocks of `tag`.
    Commit {
        tag: u32,
        hash: u64,
        stage: u64,
        rebuild: bool,
        activate: bool,
        was: Pairs,
        file: Arc<Blocks>,
    },
}

/// A structure that differs: what it wants, and the pairs the game holds.
struct Diff {
    tag: u32,
    hash: u64,
    blocks: Arc<Blocks>,
    was: Pairs,
    file: Arc<Blocks>,
}

impl Diff {
    /// `append` keeps every index: no indexed block may shrink.
    fn rebuild(&self) -> bool {
        INDEXED
            .iter()
            .any(|n| self.blocks.count(n) < self.was.get(n).map_or(0, |p| p.0))
    }
}

impl CollisionSync {
    pub(super) fn busy(&self) -> bool {
        self.sent.is_some()
    }

    pub(super) fn owns(&self, id: u64) -> bool {
        self.sent.as_ref().is_some_and(|s| s.id() == id)
    }

    /// Start again: a new map, epoch or pairing holds the file's
    /// collision.
    pub(super) fn reset(&mut self) {
        *self = CollisionSync {
            next: self.next,
            wants: std::mem::take(&mut self.wants),
            ..CollisionSync::default()
        };
    }

    /// The connection dropped: what was in flight, and every stage, is
    /// gone with it (stages belong to the connection).
    pub(super) fn transport_lost(&mut self) {
        self.sent = None;
        self.staged = None;
        self.put = None;
    }

    /// The user sent again: retry what a refusal held, and commit a
    /// mismatch against the pairs the game said it holds.
    pub(super) fn retry(&mut self) {
        if let Some(Hold {
            holds: Some((tag, pairs, file)),
            ..
        }) = self.hold.take()
        {
            let held = self.held.entry(tag).or_insert(Held {
                hash: 0,
                pairs: Pairs::new(),
                file,
            });
            held.pairs.extend(pairs);
            held.hash = 0;
        }
    }

    fn id(&mut self) -> u64 {
        self.next += 1;
        ID_BASE + self.next
    }

    /// The wants of `app`'s level collision edits, converted once per
    /// revision.
    fn wants(&mut self, app: &App) -> Arc<Vec<CollisionWant>> {
        let rev = app.modeling.level_collision_rev;
        if self.wants.0 != Some(rev) {
            let list = app.modeling.level_collision.values().map(CollisionWant::of).collect();
            self.wants = (Some(rev), Arc::new(list));
        }
        self.wants.1.clone()
    }

    /// Set the wants for documents at `rev` without converting a
    /// `LevelEdit`: the tests' fakes.
    #[cfg(test)]
    pub(super) fn prime(&mut self, rev: u64, wants: Vec<CollisionWant>) {
        self.wants = (Some(rev), Arc::new(wants));
    }

    /// Every structure whose wanted collision differs from what the game
    /// holds.
    fn diffs(&self, wants: &[CollisionWant]) -> Vec<Diff> {
        let by_tag: BTreeMap<u32, &CollisionWant> = wants.iter().map(|w| (w.tag.0, w)).collect();
        let tags: BTreeSet<u32> = by_tag.keys().chain(self.held.keys()).copied().collect();
        let mut out = Vec::new();
        for tag in tags {
            let held = self.held.get(&tag);
            let (blocks, file, was) = match (by_tag.get(&tag), held) {
                (Some(w), Some(h)) => (w.blocks.clone(), w.file.clone(), h.pairs.clone()),
                (Some(w), None) => (w.blocks.clone(), w.file.clone(), w.file_pairs.clone()),
                (None, Some(h)) => (h.file.clone(), h.file.clone(), h.pairs.clone()),
                (None, None) => continue,
            };
            let hash = Blocks::hash(&blocks);
            let holds = held.map_or_else(|| Blocks::hash(&file), |h| h.hash);
            if hash == holds {
                continue;
            }
            out.push(Diff {
                tag,
                hash,
                blocks,
                was,
                file,
            });
        }
        out
    }

    /// The next request: a commit of what the game has staged, else a
    /// stage of the first difference.
    fn step(&self, diffs: &[Diff]) -> Option<Step> {
        let d = diffs.first()?;
        Some(match &self.staged {
            Some(s) if s.tag == d.tag && s.hash == d.hash => Step::Commit {
                tag: d.tag,
                hash: d.hash,
                stage: s.stage,
                rebuild: d.rebuild(),
                activate: s.activation,
                was: d.was.clone(),
                file: d.file.clone(),
            },
            _ => Step::Stage {
                tag: d.tag,
                hash: d.hash,
                blocks: d.blocks.clone(),
            },
        })
    }
}

/// The documents' revision of everything a collision want reads.
pub(crate) fn doc_rev(app: &App) -> u64 {
    app.modeling.level_collision_rev
}

fn tag_name(app: &App, tag: u32) -> String {
    app.tags
        .tag(TagId(tag))
        .map_or_else(|| format!("{tag:#010x}"), |t| t.name.clone())
}

/// The Pending rows: one per structure that differs.
fn rows(app: &App, sync: &CollisionSync, diffs: &[Diff]) -> Vec<Value> {
    let activation = sync.staged.as_ref().is_none_or(|s| s.activation);
    diffs
        .iter()
        .map(|d| {
            let why = if d.rebuild() {
                REBUILD_NOTE
            } else if activation {
                APPEND_NOTE
            } else {
                NO_ACTIVATION_NOTE
            };
            let path = format!("structure_collision:{:08x}:0", d.tag);
            let mut row = tag_sync::row(
                "collision",
                json!(format!("{:#010x}", d.tag)),
                &path,
                Effect::Now,
                why,
            );
            row["label"] = json!(format!("{} · level collision", tag_name(app, d.tag)));
            row["target"] = json!({"target": path});
            row["rebuild"] = json!(d.rebuild());
            if let Some(h) = &sync.hold {
                row["why"] = json!(h.why);
                if h.conflict {
                    row["conflict"] = json!(true);
                    row["resolvable"] = json!(false);
                } else {
                    tag_sync::set_effect(&mut row, Effect::Reload);
                }
            }
            row
        })
        .collect()
}

/// The stage request's blocks: base64 inline, or `{stage, at, len}` into
/// the `edit.stage.put` buffer `put`, which holds [`Blocks::joined`].
fn stage_params(tag: u32, b: &Blocks, put: Option<u64>) -> Value {
    let mut at = 0usize;
    let mut bytes = |raw: &[u8]| -> Value {
        let v = match put {
            Some(stage) => json!({"stage": stage, "at": at, "len": raw.len()}),
            None => json!(encode_base64(raw)),
        };
        at += raw.len();
        v
    };
    let mut blocks = serde_json::Map::new();
    for (name, (count, raw)) in COLLISION_BLOCKS.iter().zip(&b.bsp) {
        blocks.insert((*name).to_owned(), json!({"count": count, "bytes": bytes(raw)}));
    }
    let leaves = json!({"count": b.leaves.0, "bytes": bytes(&b.leaves.1)});
    let mopp = bytes(&b.mopp);
    json!({
        "target": {"structure": format!("{tag:#010x}")},
        "blocks": blocks,
        "leaves": leaves,
        "mopp": mopp,
    })
}

fn pairs_json(pairs: &Pairs) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(n, (count, address))| ((*n).to_owned(), json!({"count": count, "address": address})))
            .collect(),
    )
}

/// `{name: {count, address}}` by `was` name.
fn pairs_of(v: &Value) -> Pairs {
    let mut out = Pairs::new();
    let Some(o) = v.as_object() else {
        return out;
    };
    let names = COLLISION_BLOCKS.iter().copied().chain([COLLISION_LEAF_CLUSTERS, COLLISION_MOPP]);
    for name in names {
        let Some(p) = o.get(name) else { continue };
        let num = |k: &str| p[k].as_u64().and_then(|n| u32::try_from(n).ok());
        if let (Some(c), Some(a)) = (num("count"), num("address")) {
            out.insert(name, (c, a));
        }
    }
    out
}

/// A `MISMATCH`'s `data.pairs` (`[{name, holds: {count, address}}]`).
fn mismatched(data: Option<&Value>) -> Pairs {
    let mut by_name = serde_json::Map::new();
    for p in data.and_then(|d| d["pairs"].as_array()).into_iter().flatten() {
        if let Some(n) = p["name"].as_str() {
            by_name.insert(n.to_owned(), p["holds"].clone());
        }
    }
    pairs_of(&Value::Object(by_name))
}

impl Play {
    /// Pending rows for level collision, pushed onto `pending`, and the
    /// next request. `None` when the editor's map is not the baseline's.
    pub(super) fn collision_pending(&mut self, app: &mut App, pending: &mut Vec<Value>) -> Option<Step> {
        if self.edits.world != Some(app.session.world_rev()) {
            return None;
        }
        app.sync_modeling();
        let rev = doc_rev(app);
        let sync = &mut self.edits.collision;
        if sync.hold.as_ref().is_some_and(|h| h.rev != rev) {
            sync.hold = None;
        }
        let wants = sync.wants(app);
        let diffs = sync.diffs(&wants);
        pending.extend(rows(app, sync, &diffs));
        sync.step(&diffs)
    }

    /// Send the next request of `step`: a stage (its large bytes first, in
    /// `edit.stage.put` chunks), or a commit.
    pub(super) fn send_collision(&mut self, step: Step) {
        let (epoch, confirmed) = (self.edits.epoch, self.edits.confirmed);
        let sync = &mut self.edits.collision;
        if sync.busy() || sync.hold.is_some() {
            return;
        }
        let id = sync.id();
        let (method, params) = match step {
            Step::Stage { tag, hash, blocks } => {
                let total = blocks.len();
                let put = sync.put.clone().filter(|p| p.tag == tag && p.hash == hash);
                if total > INLINE_BYTES && put.as_ref().is_none_or(|p| p.len < total) {
                    // The next chunk: a new buffer, or more of this one.
                    let done = put.as_ref().map_or(0, |p| p.len);
                    let joined = blocks.joined();
                    let chunk = &joined[done..(done + INLINE_BYTES).min(total)];
                    let mut params = json!({"bytes": encode_base64(chunk)});
                    match &put {
                        Some(p) => params["stage"] = json!(p.stage),
                        None => sync.put = None,
                    }
                    sync.sent = Some(Sent::Put { id, tag, hash });
                    (agent_proto::hot::method::EDIT_STAGE_PUT, params)
                } else {
                    let buffer = put.filter(|_| total > INLINE_BYTES).map(|p| p.stage);
                    let mut params = stage_params(tag, &blocks, buffer);
                    params["epoch"] = json!(epoch);
                    params["confirm_peers"] = json!(confirmed);
                    sync.sent = Some(Sent::Stage { id, tag, hash });
                    (agent_proto::hot::method::EDIT_COLLISION_STAGE, params)
                }
            }
            Step::Commit {
                tag,
                hash,
                stage,
                rebuild,
                activate,
                was,
                file,
            } => {
                let params = json!({
                    "epoch": epoch,
                    "confirm_peers": confirmed,
                    "stage": stage,
                    "mode": if rebuild { "rebuild" } else { "append" },
                    "activate": activate,
                    "was": pairs_json(&was),
                });
                sync.sent = Some(Sent::Commit {
                    id,
                    tag,
                    hash,
                    was,
                    file,
                });
                (agent_proto::hot::method::EDIT_COLLISION_COMMIT, params)
            }
        };
        self.call(method, params, Purpose::Edit(id));
    }

    /// The game's answer to a collision request.
    pub(super) fn collision_answer(&mut self, app: &mut App, result: Result<Value, agent_proto::RpcError>) {
        let Some(sent) = self.edits.collision.sent.take() else {
            return;
        };
        let rev = doc_rev(app);
        let same_map = self.edits.world == Some(app.session.world_rev());
        let method = sent.method();
        let sync = &mut self.edits.collision;
        let mut problem: Option<Problem> = None;
        match (sent, result) {
            (Sent::Put { tag, hash, .. }, Ok(v)) => match (v["stage"].as_u64(), v["len"].as_u64()) {
                (Some(stage), Some(len)) => {
                    sync.put = Some(Put {
                        tag,
                        hash,
                        stage,
                        len: len as usize,
                    })
                }
                _ => problem = Some(("the game returned no stage".to_owned(), false, None)),
            },
            (Sent::Stage { tag, hash, .. }, Ok(v)) => {
                // The stage used the buffer up.
                sync.put = None;
                match v["stage"].as_u64() {
                    Some(stage) => {
                        sync.staged = Some(Staged {
                            tag,
                            hash,
                            stage,
                            activation: v["activation"].as_bool().unwrap_or(false),
                        });
                    }
                    None => problem = Some(("the game returned no stage".to_owned(), false, None)),
                }
            }
            (
                Sent::Commit {
                    tag, hash, mut was, file, ..
                },
                Ok(v),
            ) => {
                sync.staged = None;
                if same_map {
                    was.extend(pairs_of(&v["pairs"]));
                    sync.held.insert(tag, Held { hash, pairs: was, file });
                }
            }
            (sent, Err(e)) => {
                if !matches!(sent, Sent::Commit { .. }) {
                    sync.put = None;
                }
                let conflict = e.code == agent_proto::code::MISMATCH;
                let (why, holds) = match sent {
                    Sent::Commit { tag, mut was, file, .. } if conflict => {
                        was.extend(mismatched(e.data.as_ref()));
                        (
                            "the game holds other collision pairs; change the edit or send again".to_owned(),
                            Some((tag, was, file)),
                        )
                    }
                    _ => (
                        e.data
                            .as_ref()
                            .and_then(|d| d["why"].as_str())
                            .map(str::to_owned)
                            .unwrap_or(e.message),
                        None,
                    ),
                };
                // A refused commit's stage may be gone with it (a moved
                // pair, a map change): stage again.
                sync.staged = None;
                problem = Some((why, conflict, holds));
            }
        }
        if let Some((why, conflict, holds)) = problem {
            sync.hold = Some(Hold {
                rev,
                why: why.clone(),
                conflict,
                holds,
            });
            self.feedback.problem(app, format!("{method}: {why}"));
        }
        self.edits.last_diff = None;
        self.next_push = Instant::now();
        if std::mem::take(&mut self.edits.restore_after_batch)
            && let Err(why) = self.apply_edits(app, Apply::Restore)
        {
            app.error(why);
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
