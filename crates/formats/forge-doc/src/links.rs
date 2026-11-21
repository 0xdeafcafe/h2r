//! Links written as retail data: teleporter channels and device groups.
//!
//! A link names two entries of a scenario block by identity: an entry of
//! the base map by its index there, or the entry a `Placement::Add` op of
//! this document made. Identity survives later appends and swap-removes,
//! so the materialiser resolves every active link to final indices after
//! the placement edits. An unlinked entry keeps the bytes it would have had
//! without the link.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    ActiveLabel, BlockKind, Edit, ForgeDoc, Label, Labels, Op, OpId, Placement, Refusal, Result,
    wire,
};

/// The largest channel a teleporter link writes. The netgame flag's
/// identifier is an `i16`; retail channels are small non-negative values.
pub const MAX_CHANNEL: u16 = i16::MAX as u16;
/// The netgame flag type the teleporter readers treat as a sender
/// (docs/game/bots.md 3.2, "Traversal evidence").
pub const TELEPORTER_SENDER: u16 = 7;
/// The netgame flag type the teleporter readers treat as a receiver.
pub const TELEPORTER_RECEIVER: u16 = 8;

const MAX_ENTRY_INDEX: u32 = i16::MAX as u32;

/// One scenario entry a link names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Endpoint {
    /// The base map's entry at `index`, wherever later edits move it.
    Base {
        #[serde(with = "wire::block")]
        block: BlockKind,
        index: u32,
    },
    /// The entry the document's `Placement::Add` op with this ID made.
    Added(OpId),
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
        let pass = Pass::run(&self.ops, base_counts)?;
        pass.resolve()
    }

    /// Resolve the active labels to final indices. The map's base count is
    /// needed for each block a label names.
    pub fn labels(&self, base_counts: &BTreeMap<BlockKind, u32>) -> Result<Labels> {
        Pass::run(&self.ops, base_counts)?.resolve_labels()
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
            if sender == receiver {
                return Err(invalid(link.what(), "a flag cannot link to itself"));
            }
            if channel > MAX_CHANNEL {
                return Err(invalid(link.what(), "channel exceeds the identifier range"));
            }
        }
        Link::Device { from, to } => {
            if from >= to {
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
/// a linked entry is not removed, and each flag keeps one role and channel.
pub(crate) fn check(ops: &[Op], base_counts: &BTreeMap<BlockKind, u32>) -> Result<()> {
    Pass::run(ops, base_counts).map(|_| ())
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
}

impl<'a> Pass<'a> {
    fn run(ops: &[Op], base: &'a BTreeMap<BlockKind, u32>) -> Result<Self> {
        let mut pass = Self {
            base,
            rows: BTreeMap::new(),
            added: BTreeMap::new(),
            gone: BTreeSet::new(),
            active: BTreeMap::new(),
            order: Vec::new(),
            roles: BTreeMap::new(),
            labels: BTreeMap::new(),
        };
        for op in ops {
            match &op.edit {
                Edit::Placement(placement) => pass.placement(op.id, placement)?,
                Edit::Forge(Forge::Link(link)) => pass.link(op.id, link)?,
                Edit::Forge(Forge::Unlink { link }) => pass.unlink(*link)?,
                Edit::Forge(Forge::Label { target, label }) => {
                    pass.label(op.id, *target, label.as_ref())?
                }
                _ => {}
            }
        }
        Ok(pass)
    }

    fn rows(&mut self, block: BlockKind) -> Option<&mut Vec<Endpoint>> {
        let count = *self.base.get(&block)?;
        Some(self.rows.entry(block).or_insert_with(|| {
            (0..count)
                .map(|index| Endpoint::Base { block, index })
                .collect()
        }))
    }

    fn placement(&mut self, id: OpId, placement: &Placement) -> Result<()> {
        let block = placement.block();
        match *placement {
            Placement::Add { .. } => {
                self.added.insert(id, block);
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
                    self.gone.insert(end);
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
            Endpoint::Base { block, index } => self.base.get(&block).is_none_or(|&n| index < n),
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
