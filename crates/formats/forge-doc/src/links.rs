//! Links written as retail data: teleporter channels and device groups.
//!
//! A link names two entries of a scenario block by identity: an entry of
//! the base map by its index there, or the entry a `Placement::Add` op of
//! this document made. Identity survives later appends and swap-removes,
//! so the materialiser resolves every active link to final indices after
//! the placement edits. An unlinked entry keeps the bytes it would have had
//! without the link.
//!
//! Labels and entry ops (`crate::entry`) name entries the same way, and one
//! pass over the ops checks all three: it follows each identity through the
//! placement and entry edits, and holds each base entry to one print.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    ActiveLabel, Area, BlockKind, Edit, EntryOp, FieldWrite, ForgeDoc, Label, Labels, Limits,
    MarkerArea, Op, OpId, Placement, Print, Refusal, Result, Value, WorldBounds, builtin_label,
    entry, wire,
};

/// The largest channel a teleporter link writes. The netgame flag's
/// identifier is an `i16`; retail channels are small non-negative values.
pub const MAX_CHANNEL: u16 = i16::MAX as u16;
/// The netgame flag type the teleporter readers treat as a sender
/// (docs/game/bots.md 3.2, "Traversal evidence").
pub const TELEPORTER_SENDER: u16 = 7;
/// The netgame flag type the teleporter readers treat as a receiver.
pub const TELEPORTER_RECEIVER: u16 = 8;

const FLAGS: BlockKind = BlockKind::NetgameFlags;
const MAX_ENTRY_INDEX: u32 = i16::MAX as u32;

/// One scenario entry a link, a label or an entry op names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Endpoint {
    /// The base map's entry at `index` in the base map's block, wherever
    /// later edits move it. `print` is the entry's [`Print`]: an entry op
    /// requires one; a link or a label may go without, which keeps its
    /// spelling and identity.
    Base {
        #[serde(with = "wire::block")]
        block: BlockKind,
        index: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        print: Option<Print>,
    },
    /// The entry the document's `Placement::Add` op with this ID made.
    Added(OpId),
}

impl Endpoint {
    /// The base map's entry at `index`, named without a print.
    pub const fn base(block: BlockKind, index: u32) -> Self {
        Self::Base {
            block,
            index,
            print: None,
        }
    }

    /// The entry this names, without its print. Two endpoints name one
    /// entry when their identities are equal.
    pub const fn identity(self) -> Self {
        match self {
            Self::Base { block, index, .. } => Self::base(block, index),
            added => added,
        }
    }

    /// The print a base endpoint carries.
    pub const fn print(self) -> Option<Print> {
        match self {
            Self::Base { print, .. } => print,
            Self::Added(_) => None,
        }
    }
}

/// Forge-family edits. Links are written as retail data, so they work in
/// any session and cost nothing a tick.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Forge {
    Link(Link),
    /// End the active link the op `link` made.
    Unlink {
        link: OpId,
    },
    /// Set the label of a placement, replacing any it has, or clear it
    /// (`label` absent). Needs format 2.
    Label {
        target: Endpoint,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<Label>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Link {
    /// A teleporter sender and receiver on one channel: two netgame flags of
    /// the sender and receiver types with the same identifier. A sender
    /// may have several receivers on its channel.
    Teleporter {
        sender: Endpoint,
        receiver: Endpoint,
        channel: u16,
    },
    /// Two machines in one device group. Linked machines form one group
    /// each, which the materialiser adds to the scenario. The lower
    /// endpoint comes first, so a link has one spelling.
    Device { from: Endpoint, to: Endpoint },
}

impl Link {
    pub fn endpoints(&self) -> [Endpoint; 2] {
        match *self {
            Self::Teleporter {
                sender, receiver, ..
            } => [sender, receiver],
            Self::Device { from, to } => [from, to],
        }
    }

    /// The scenario block both endpoints belong to.
    pub fn block(&self) -> BlockKind {
        match self {
            Self::Teleporter { .. } => BlockKind::NetgameFlags,
            Self::Device { .. } => BlockKind::Machines,
        }
    }

    fn what(&self) -> &'static str {
        match self {
            Self::Teleporter { .. } => "teleporter link",
            Self::Device { .. } => "device link",
        }
    }
}

/// A teleporter flag an active link writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TeleporterFlag {
    /// The netgame flag's index after every placement edit.
    pub index: u32,
    pub sender: bool,
    pub channel: u16,
}

/// An active link with its endpoints' final indices in its block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveLink {
    pub id: OpId,
    pub link: Link,
    pub indices: [u32; 2],
}

/// The document's active links, resolved against the base map's counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Links {
    /// In op order.
    pub active: Vec<ActiveLink>,
    /// Each linked flag once, by final index.
    pub teleporters: Vec<TeleporterFlag>,
    /// Final machine indices of each group, ascending. Groups are ordered
    /// by their lowest member, which fixes the order they are added in.
    pub device_groups: Vec<Vec<u32>>,
}

impl ForgeDoc {
    /// Resolve the active links to final indices. The map's base count is
    /// needed for each block a link names.
    pub fn links(&self, base_counts: &BTreeMap<BlockKind, u32>) -> Result<Links> {
        let pass = Pass::run(&self.ops, base_counts, None, None)?;
        pass.resolve()
    }

    /// Resolve the active labels to final indices. The map's base count is
    /// needed for each block a label names.
    pub fn labels(&self, base_counts: &BTreeMap<BlockKind, u32>) -> Result<Labels> {
        Pass::run(&self.ops, base_counts, None, None)?.resolve_labels()
    }

    /// Resolve active areas to final netgame flag indices.
    pub fn areas(&self, base_counts: &BTreeMap<BlockKind, u32>) -> Result<BTreeMap<u32, MarkerArea>> {
        if !has_area_ops(&self.ops) {
            return Ok(BTreeMap::new());
        }
        Pass::run(&self.ops, base_counts, None, None)?.resolve_areas()
    }

    /// Active areas by placement identity, for a live session whose base
    /// things still use their original indices.
    pub fn area_targets(
        &self,
        base_counts: &BTreeMap<BlockKind, u32>,
    ) -> Result<BTreeMap<Endpoint, Area>> {
        if !has_area_ops(&self.ops) {
            return Ok(BTreeMap::new());
        }
        Ok(Pass::run(&self.ops, base_counts, None, None)?.areas)
    }
}

fn invalid(what: &str, reason: impl Into<String>) -> Refusal {
    Refusal::invalid(what, reason)
}

/// Checks that need only the op itself.
pub(crate) fn op(edit: &Forge) -> Result<()> {
    let link = match edit {
        Forge::Label { target, label } => return crate::labels::check_op(target, label.as_ref()),
        Forge::Unlink { link } if link.0 == 0 => {
            return Err(invalid("unlink", "want the ID of a link op"));
        }
        Forge::Unlink { .. } => return Ok(()),
        Forge::Link(link) => link,
    };
    let block = link.block();
    for end in link.endpoints() {
        match end {
            Endpoint::Base { block: b, .. } if b != block => {
                return Err(invalid(
                    link.what(),
                    format!("want {} entries", block.name()),
                ));
            }
            Endpoint::Base { index, .. } if index > MAX_ENTRY_INDEX => {
                return Err(invalid(link.what(), "entry index exceeds block limit"));
            }
            Endpoint::Added(OpId(0)) => {
                return Err(invalid(link.what(), "want the ID of an add op"));
            }
            _ => {}
        }
    }
    match *link {
        Link::Teleporter {
            sender,
            receiver,
            channel,
        } => {
            if sender.identity() == receiver.identity() {
                return Err(invalid(link.what(), "a flag cannot link to itself"));
            }
            if channel > MAX_CHANNEL {
                return Err(invalid(link.what(), "channel exceeds the identifier range"));
            }
        }
        Link::Device { from, to } => {
            if from.identity() >= to.identity() {
                return Err(invalid(
                    link.what(),
                    "want two machines, the lower endpoint first",
                ));
            }
        }
    }
    Ok(())
}

/// Checks over the active document: endpoints exist when the link is made,
/// a linked entry is not removed, each flag keeps one role and channel,
/// each base entry has one print, and the entry ops' rules
/// (`Pass::entry`).
pub(crate) fn check(ops: &[Op], limits: &Limits) -> Result<()> {
    Pass::run(
        ops,
        &limits.base_counts,
        limits.world,
        Some(&limits.base_flag_types),
    )?
    .check_hills()
}

struct Role {
    sender: bool,
    channel: u16,
    links: u32,
}

struct Pass<'a> {
    base: &'a BTreeMap<BlockKind, u32>,
    /// Entry identities in index order, for the link blocks whose base
    /// count is known.
    rows: BTreeMap<BlockKind, Vec<Endpoint>>,
    added: BTreeMap<OpId, BlockKind>,
    gone: BTreeSet<Endpoint>,
    active: BTreeMap<OpId, Link>,
    order: Vec<OpId>,
    roles: BTreeMap<Endpoint, Role>,
    labels: BTreeMap<Endpoint, (OpId, Label)>,
    areas: BTreeMap<Endpoint, Area>,
    /// Flag types by identity, tracked only when the document shapes an area.
    flag_types: Option<BTreeMap<Endpoint, u16>>,
    /// The print each base entry is named with, by identity.
    prints: BTreeMap<Endpoint, Print>,
    /// Base entries an entry op has changed since the last reset.
    edited: BTreeSet<Endpoint>,
    /// The ops a later reset covers (`entry::covered`).
    covered: BTreeSet<OpId>,
    world: Option<WorldBounds>,
}

impl<'a> Pass<'a> {
    fn run(
        ops: &[Op],
        base: &'a BTreeMap<BlockKind, u32>,
        world: Option<WorldBounds>,
        base_flag_types: Option<&[u16]>,
    ) -> Result<Self> {
        let flag_types = base_flag_types.filter(|_| has_area_ops(ops)).map(|kinds| {
            kinds
                .iter()
                .enumerate()
                .map(|(i, &kind)| (Endpoint::base(FLAGS, i as u32), kind))
                .collect()
        });
        let mut pass = Self {
            base,
            rows: BTreeMap::new(),
            added: BTreeMap::new(),
            gone: BTreeSet::new(),
            active: BTreeMap::new(),
            order: Vec::new(),
            roles: BTreeMap::new(),
            labels: BTreeMap::new(),
            areas: BTreeMap::new(),
            flag_types,
            prints: BTreeMap::new(),
            edited: BTreeSet::new(),
            covered: entry::covered(ops),
            world,
        };
        for op in ops {
            match &op.edit {
                Edit::Placement(placement) => pass.placement(op.id, placement)?,
                Edit::Forge(Forge::Link(link)) => {
                    let link = pass.bare(link)?;
                    pass.link(op.id, &link)?
                }
                Edit::Forge(Forge::Unlink { link }) => pass.unlink(*link)?,
                Edit::Forge(Forge::Label { target, label }) => {
                    let target = pass.name(*target)?;
                    pass.label(op.id, target, label.as_ref())?
                }
                Edit::Entry(edit) => pass.entry(op.id, edit)?,
                _ => {}
            }
        }
        Ok(pass)
    }

    fn rows(&mut self, block: BlockKind) -> Option<&mut Vec<Endpoint>> {
        let count = *self.base.get(&block)?;
        Some(self.rows.entry(block).or_insert_with(|| {
            (0..count)
                .map(|index| Endpoint::base(block, index))
                .collect()
        }))
    }

    /// The identity `end` names, after recording its print. One base entry
    /// has one print in a document.
    fn name(&mut self, end: Endpoint) -> Result<Endpoint> {
        let identity = end.identity();
        if let Some(print) = end.print()
            && *self.prints.entry(identity).or_insert(print) != print
        {
            return Err(invalid(
                "print",
                "the entry is named with two different prints",
            ));
        }
        Ok(identity)
    }

    /// The link with its endpoints' identities.
    fn bare(&mut self, link: &Link) -> Result<Link> {
        Ok(match *link {
            Link::Teleporter {
                sender,
                receiver,
                channel,
            } => Link::Teleporter {
                sender: self.name(sender)?,
                receiver: self.name(receiver)?,
                channel,
            },
            Link::Device { from, to } => Link::Device {
                from: self.name(from)?,
                to: self.name(to)?,
            },
        })
    }

    /// An entry op: its target exists and is not removed, an added
    /// target's write fits its block, and a reset follows an entry op on
    /// its entry. A removal requires the links, label and area to be clear.
    fn entry(&mut self, id: OpId, edit: &EntryOp) -> Result<()> {
        let what = edit.what();
        let target = self.name(edit.target())?;
        let block = match target {
            Endpoint::Base { block, index, .. } => {
                if self.base.get(&block).is_some_and(|&n| index >= n) {
                    return Err(invalid(what, format!("names no {} entry", block.name())));
                }
                block
            }
            Endpoint::Added(add) => {
                let block = *self
                    .added
                    .get(&add)
                    .ok_or_else(|| invalid(what, "names no added entry"))?;
                entry::check_added(edit, block, self.world)?;
                block
            }
        };
        if let EntryOp::Reset { .. } = edit {
            if !self.edited.remove(&target) {
                return Err(invalid(what, "the entry has no edit to reset"));
            }
            self.gone.remove(&target);
            self.areas.remove(&target);
            return Ok(());
        }
        if self.gone.contains(&target) {
            return Err(invalid(what, "names a removed entry"));
        }
        if let Endpoint::Base { .. } = target {
            self.edited.insert(target);
        }
        if let EntryOp::Area { area, .. } = edit {
            match area {
                Some(area) => {
                    self.areas.insert(target, area.clone());
                }
                None if self.areas.remove(&target).is_none() => {
                    return Err(invalid(what, "the flag has no area to clear"));
                }
                None => {}
            }
        }
        if let EntryOp::Set { write, .. } = edit
            && block == FLAGS
            && !self.covered.contains(&id)
            && let Some(types) = &mut self.flag_types
            && let Some(kind) = flag_type(write)
        {
            types.insert(target, kind);
        }
        if let EntryOp::Remove { .. } = edit {
            if self.linked(target) {
                return Err(invalid(block.name(), "remove the entry's links first"));
            }
            if self.labels.contains_key(&target) {
                return Err(invalid(block.name(), "clear the entry's label first"));
            }
            if self.areas.contains_key(&target) {
                return Err(invalid(block.name(), "clear the flag's area first"));
            }
            self.gone.insert(target);
            // A removal a later reset covers is left out by the fold, so
            // the entry keeps its row for the final indices.
            if !self.covered.contains(&id)
                && let Some(rows) = self.rows(block)
                && let Some(i) = rows.iter().position(|e| *e == target)
            {
                rows.swap_remove(i);
            }
        }
        Ok(())
    }

    fn placement(&mut self, id: OpId, placement: &Placement) -> Result<()> {
        let block = placement.block();
        match *placement {
            Placement::Add { like, ref fields, .. } => {
                self.added.insert(id, block);
                if block == FLAGS && self.flag_types.is_some() {
                    let kind = fields.iter().find_map(flag_type).or_else(|| match like {
                        Some(index) => {
                            let source = *self.rows(FLAGS)?.get(index as usize)?;
                            self.flag_types.as_ref()?.get(&source).copied()
                        }
                        None => Some(0),
                    });
                    if let (Some(types), Some(kind)) = (&mut self.flag_types, kind) {
                        types.insert(Endpoint::Added(id), kind);
                    }
                }
                if let Some(rows) = self.rows(block) {
                    rows.push(Endpoint::Added(id));
                }
            }
            Placement::Remove { index, .. } => {
                let Some(rows) = self.rows(block) else {
                    return Ok(());
                };
                // The placement counts refuse an index past the end.
                if (index as usize) < rows.len() {
                    let end = rows.swap_remove(index as usize);
                    if self.linked(end) {
                        return Err(invalid(block.name(), "remove the entry's links first"));
                    }
                    if self.labels.contains_key(&end) {
                        return Err(invalid(block.name(), "clear the entry's label first"));
                    }
                    if self.areas.contains_key(&end) {
                        return Err(invalid(block.name(), "clear the flag's area first"));
                    }
                    self.gone.insert(end);
                }
            }
            Placement::Set { index, ref write, .. }
                if block == FLAGS && self.flag_types.is_some() =>
            {
                let target = self
                    .rows(FLAGS)
                    .and_then(|rows| rows.get(index as usize))
                    .copied();
                if let (Some(types), Some(target), Some(kind)) =
                    (&mut self.flag_types, target, flag_type(write))
                {
                    types.insert(target, kind);
                }
            }
            Placement::Set { .. } => {}
        }
        Ok(())
    }

    fn linked(&self, end: Endpoint) -> bool {
        self.active.values().any(|l| l.endpoints().contains(&end))
    }

    fn exists(&self, link: &Link, end: Endpoint) -> Result<()> {
        let block = link.block();
        let present = match end {
            Endpoint::Base { index, .. } => self.base.get(&block).is_none_or(|&n| index < n),
            Endpoint::Added(id) => self.added.get(&id) == Some(&block),
        };
        if !present || self.gone.contains(&end) {
            return Err(invalid(
                link.what(),
                format!("names no {} entry", block.name()),
            ));
        }
        Ok(())
    }

    fn link(&mut self, id: OpId, link: &Link) -> Result<()> {
        for end in link.endpoints() {
            self.exists(link, end)?;
        }
        if self.active.values().any(|l| l == link) {
            return Err(invalid(link.what(), "the link already exists"));
        }
        if let Link::Teleporter {
            sender,
            receiver,
            channel,
        } = *link
        {
            for (end, is_sender) in [(sender, true), (receiver, false)] {
                if let Some(role) = self.roles.get(&end) {
                    if role.sender != is_sender {
                        return Err(invalid(
                            link.what(),
                            "a flag is either a sender or a receiver",
                        ));
                    }
                    if role.channel != channel {
                        return Err(invalid(
                            link.what(),
                            format!("the flag is already on channel {}", role.channel),
                        ));
                    }
                }
            }
            for (end, is_sender) in [(sender, true), (receiver, false)] {
                self.roles
                    .entry(end)
                    .or_insert(Role {
                        sender: is_sender,
                        channel,
                        links: 0,
                    })
                    .links += 1;
            }
        }
        self.active.insert(id, *link);
        self.order.push(id);
        Ok(())
    }

    fn unlink(&mut self, id: OpId) -> Result<()> {
        let link = self
            .active
            .remove(&id)
            .ok_or_else(|| invalid("unlink", "names no active link"))?;
        self.order.retain(|&o| o != id);
        if let Link::Teleporter { .. } = link {
            for end in link.endpoints() {
                if let Some(role) = self.roles.get_mut(&end) {
                    role.links -= 1;
                    if role.links == 0 {
                        self.roles.remove(&end);
                    }
                }
            }
        }
        Ok(())
    }

    fn label(&mut self, id: OpId, target: Endpoint, label: Option<&Label>) -> Result<()> {
        let present = match target {
            Endpoint::Base { block, index, .. } => self.base.get(&block).is_none_or(|&n| index < n),
            Endpoint::Added(add) => self.added.contains_key(&add),
        };
        if !present || self.gone.contains(&target) {
            return Err(invalid("label", "names no entry"));
        }
        match label {
            Some(label) => {
                self.labels.insert(target, (id, label.clone()));
            }
            None => {
                if self.labels.remove(&target).is_none() {
                    return Err(invalid("label", "the entry has no label to clear"));
                }
            }
        }
        Ok(())
    }

    fn check_hills(&self) -> Result<()> {
        let Some(types) = &self.flag_types else {
            return Ok(());
        };
        if self.areas.is_empty() {
            return Ok(());
        }
        let mut hills = BTreeMap::new();
        let labelled = self.labels.keys().filter(|end| !types.contains_key(*end));
        for end in types.keys().chain(labelled) {
            let block = match *end {
                Endpoint::Base { block, .. } => block,
                Endpoint::Added(id) => self.added[&id],
            };
            if block != FLAGS || self.gone.contains(end) {
                continue;
            }
            let label = self
                .labels
                .get(end)
                .map(|(_, label)| label.clone())
                .or_else(|| types.get(end).and_then(|&kind| builtin_label(kind)));
            if let Some(label) = label
                && label.text == "hill_corner"
            {
                let (count, shaped) = hills.entry(label.number).or_insert((0, false));
                *count += 1;
                *shaped |= self.areas.contains_key(end);
            }
        }
        if hills.values().any(|&(count, shaped)| count >= 3 && shaped) {
            return Err(Refusal::Area("a hill with corners takes no area".into()));
        }
        Ok(())
    }

    fn resolve_areas(mut self) -> Result<BTreeMap<u32, MarkerArea>> {
        if self.areas.is_empty() {
            return Ok(BTreeMap::new());
        }
        if self.rows(FLAGS).is_none() {
            return Err(invalid("areas", "the map's netgame_flags count is needed"));
        }
        std::mem::take(&mut self.areas)
            .into_iter()
            .map(|(target, area)| Ok((self.index(FLAGS, target)?, area.marker_area())))
            .collect()
    }

    fn resolve_labels(mut self) -> Result<Labels> {
        let labels = std::mem::take(&mut self.labels);
        let mut active = Vec::with_capacity(labels.len());
        for (target, (id, label)) in labels {
            let block = match target {
                Endpoint::Base { block, .. } => block,
                Endpoint::Added(add) => self.added[&add],
            };
            if self.rows(block).is_none() {
                return Err(invalid(
                    "labels",
                    format!("the map's {} count is needed", block.name()),
                ));
            }
            active.push(ActiveLabel {
                id,
                block,
                index: self.index(block, target)?,
                label,
            });
        }
        active.sort_by_key(|l| (l.block, l.index));
        Ok(Labels { active })
    }

    fn index(&self, block: BlockKind, end: Endpoint) -> Result<u32> {
        self.rows
            .get(&block)
            .and_then(|rows| rows.iter().position(|e| *e == end))
            .map(|i| i as u32)
            .ok_or_else(|| invalid("links", format!("names no {} entry", block.name())))
    }

    fn resolve(mut self) -> Result<Links> {
        // A linked block no placement touched still needs its base rows.
        let blocks: BTreeSet<_> = self.active.values().map(Link::block).collect();
        for block in blocks {
            if self.rows(block).is_none() {
                return Err(invalid(
                    "links",
                    format!("the map's {} count is needed", block.name()),
                ));
            }
        }
        let mut links = Links::default();
        for id in &self.order {
            let link = self.active[id];
            let [a, b] = link.endpoints();
            links.active.push(ActiveLink {
                id: *id,
                link,
                indices: [self.index(link.block(), a)?, self.index(link.block(), b)?],
            });
        }
        let mut flags = BTreeMap::new();
        let mut machines = Groups::default();
        for active in &links.active {
            match active.link {
                Link::Teleporter { channel, .. } => {
                    for (index, sender) in [(active.indices[0], true), (active.indices[1], false)] {
                        flags.insert(
                            index,
                            TeleporterFlag {
                                index,
                                sender,
                                channel,
                            },
                        );
                    }
                }
                Link::Device { .. } => machines.join(active.indices[0], active.indices[1]),
            }
        }
        links.teleporters = flags.into_values().collect();
        links.device_groups = machines.finish();
        Ok(links)
    }
}

fn has_area_ops(ops: &[Op]) -> bool {
    ops.iter()
        .any(|op| matches!(&op.edit, Edit::Entry(EntryOp::Area { .. })))
}

fn flag_type(write: &FieldWrite) -> Option<u16> {
    match (write.field.as_str(), &write.value) {
        ("type", Value::Int(n)) => u16::try_from(*n).ok(),
        _ => None,
    }
}

/// Union-find over machine indices.
#[derive(Default)]
struct Groups {
    parent: BTreeMap<u32, u32>,
}

impl Groups {
    fn root(&mut self, mut i: u32) -> u32 {
        while let Some(&p) = self.parent.get(&i) {
            if p == i {
                break;
            }
            i = p;
        }
        i
    }

    fn join(&mut self, a: u32, b: u32) {
        self.parent.entry(a).or_insert(a);
        self.parent.entry(b).or_insert(b);
        let (ra, rb) = (self.root(a), self.root(b));
        let (low, high) = (ra.min(rb), ra.max(rb));
        self.parent.insert(high, low);
    }

    fn finish(mut self) -> Vec<Vec<u32>> {
        let members: Vec<u32> = self.parent.keys().copied().collect();
        let mut groups: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for m in members {
            let root = self.root(m);
            groups.entry(root).or_default().push(m);
        }
        // Each root is its group's lowest member, and members are visited in
        // ascending order, so both levels are already sorted.
        groups.into_values().collect()
    }
}
