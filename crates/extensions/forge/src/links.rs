//! What the Links tool joins: the loaded map's teleporter flags and the
//! session's machines (docs/game/forge-design.md, "Teleporters and retail
//! links"). Both are written as retail data, so the document's link ops
//! name them as scenario entries.
use std::collections::BTreeMap;

use forge_doc::{BlockKind, Endpoint, Link, Links, MAX_CHANNEL};

use crate::{OpId, PlacedObject};

/// A teleporter netgame flag of the loaded map: type 7 sends, type 8
/// receives, and the identifier is its channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Teleporter {
    /// Its index in the scenario's netgame flags.
    pub index: u32,
    pub position: [f32; 3],
    pub sender: bool,
    pub channel: i16,
}

/// One end of a link the tool can aim at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Linkable {
    /// A teleporter flag, by its netgame flag index.
    Teleporter(u32),
    /// A machine this session placed.
    Object(OpId),
}

impl Linkable {
    pub(crate) fn endpoint(self) -> Endpoint {
        match self {
            Self::Teleporter(index) => Endpoint::Base {
                block: BlockKind::NetgameFlags,
                index,
            },
            Self::Object(id) => Endpoint::Added(id),
        }
    }
}

/// The link op joining `a` and `b`, with the channel a teleporter pair
/// takes: the channel either flag already has through an active link
/// (the sender's first), else the sender's own. A flag with no usable
/// channel takes the lowest one no teleporter uses.
pub(crate) fn link(
    a: Linkable,
    b: Linkable,
    teleporters: &[Teleporter],
    objects: &[PlacedObject],
    links: &Links,
) -> Result<Link, String> {
    match (a, b) {
        (Linkable::Teleporter(x), Linkable::Teleporter(y)) => {
            let find = |i| {
                teleporters
                    .iter()
                    .find(|t| t.index == i)
                    .ok_or_else(|| format!("Netgame flag {i} is not a teleporter."))
            };
            let (x, y) = (find(x)?, find(y)?);
            let (sender, receiver) = match (x.sender, y.sender) {
                (true, false) => (x, y),
                (false, true) => (y, x),
                _ => return Err("Link a teleporter sender to a receiver.".into()),
            };
            let active = |t: &Teleporter| {
                links
                    .teleporters
                    .iter()
                    .find(|f| f.index == t.index)
                    .map(|f| f.channel)
            };
            let channel = match active(sender).or_else(|| active(receiver)) {
                Some(channel) => channel,
                None => u16::try_from(sender.channel)
                    .ok()
                    .filter(|&c| c <= MAX_CHANNEL)
                    .map_or_else(|| free_channel(teleporters, links), Ok)?,
            };
            Ok(Link::Teleporter {
                sender: Linkable::Teleporter(sender.index).endpoint(),
                receiver: Linkable::Teleporter(receiver.index).endpoint(),
                channel,
            })
        }
        (Linkable::Object(x), Linkable::Object(y)) => {
            for id in [x, y] {
                let object = objects
                    .iter()
                    .find(|o| o.id == id)
                    .ok_or("Aim at a Forge object.")?;
                if object.item.block != BlockKind::Machines {
                    return Err("Only machines join a device group.".into());
                }
            }
            if x == y {
                return Err("Link a machine to another machine.".into());
            }
            let (from, to) = (x.min(y), x.max(y));
            Ok(Link::Device {
                from: Endpoint::Added(from),
                to: Endpoint::Added(to),
            })
        }
        _ => Err("Link a teleporter to a teleporter, or a machine to a machine.".into()),
    }
}

fn free_channel(teleporters: &[Teleporter], links: &Links) -> Result<u16, String> {
    let used = effective(teleporters, links);
    (0..=MAX_CHANNEL)
        .find(|&c| !used.values().any(|&u| u == c as i16))
        .ok_or_else(|| "Every teleporter channel is in use.".into())
}

/// Each teleporter flag's channel: its link's, else the map's.
pub(crate) fn effective(teleporters: &[Teleporter], links: &Links) -> BTreeMap<u32, i16> {
    let mut channels: BTreeMap<u32, i16> =
        teleporters.iter().map(|t| (t.index, t.channel)).collect();
    for flag in &links.teleporters {
        channels.insert(flag.index, flag.channel as i16);
    }
    channels
}

/// Refuse a linked flag the map does not hold as a teleporter of that role.
pub(crate) fn check_roles(teleporters: &[Teleporter], links: &Links) -> Result<(), String> {
    for flag in &links.teleporters {
        let role = if flag.sender { "sender" } else { "receiver" };
        if !teleporters
            .iter()
            .any(|t| t.index == flag.index && t.sender == flag.sender)
        {
            return Err(format!(
                "Netgame flag {} is not a teleporter {role}.",
                flag.index
            ));
        }
    }
    Ok(())
}
