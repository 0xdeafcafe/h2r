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

/// A teleporter flag this session added (a kit row), which the Links tool
/// can join as it joins the map's own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AddedTeleporter {
    /// Its add op.
    pub id: OpId,
    pub position: [f32; 3],
    pub sender: bool,
    /// Its identifier: the active link's channel, else the add's.
    pub channel: i16,
}

/// One end of a link the tool can aim at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Linkable {
    /// A teleporter flag of the map, by its netgame flag index.
    Teleporter(u32),
    /// A teleporter flag this session added, by its add op.
    Flag(OpId),
    /// A machine this session placed.
    Object(OpId),
}

impl Linkable {
    pub(crate) fn endpoint(self) -> Endpoint {
        match self {
            Self::Teleporter(index) => Endpoint::base(BlockKind::NetgameFlags, index),
            Self::Flag(id) | Self::Object(id) => Endpoint::Added(id),
        }
    }
}

/// A teleporter flag the link tool can join, with the channel it has now.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Flag {
    pub thing: Linkable,
    pub sender: bool,
    pub channel: i16,
}

/// The link op joining `a` and `b`, with the channel a teleporter pair
/// takes: the channel either flag already has through an active link
/// (the sender's first), else the sender's own. A flag with no usable
/// channel takes the lowest one no teleporter uses. A flag is the map's
/// own or one the session added.
pub(crate) fn link(
    a: Linkable,
    b: Linkable,
    flags: &[Flag],
    objects: &[PlacedObject],
    links: &Links,
) -> Result<Link, String> {
    match (a, b) {
        (Linkable::Teleporter(_) | Linkable::Flag(_), Linkable::Teleporter(_) | Linkable::Flag(_)) => {
            let find = |thing: Linkable| {
                flags.iter().find(|f| f.thing == thing).ok_or_else(|| match thing {
                    Linkable::Teleporter(i) => format!("Netgame flag {i} is not a teleporter."),
                    _ => "That netgame flag is not a teleporter.".to_string(),
                })
            };
            let (x, y) = (find(a)?, find(b)?);
            if x.thing == y.thing {
                return Err("Link a teleporter sender to a receiver.".into());
            }
            let (sender, receiver) = match (x.sender, y.sender) {
                (true, false) => (x, y),
                (false, true) => (y, x),
                _ => return Err("Link a teleporter sender to a receiver.".into()),
            };
            let active = |f: &Flag| {
                let end = f.thing.endpoint().identity();
                links.active.iter().find_map(|l| match l.link {
                    Link::Teleporter {
                        sender,
                        receiver,
                        channel,
                    } if sender.identity() == end || receiver.identity() == end => Some(channel),
                    _ => None,
                })
            };
            let channel = match active(sender).or_else(|| active(receiver)) {
                Some(channel) => channel,
                None => u16::try_from(sender.channel)
                    .ok()
                    .filter(|&c| c <= MAX_CHANNEL)
                    .map_or_else(|| free_channel(flags), Ok)?,
            };
            Ok(Link::Teleporter {
                sender: sender.thing.endpoint(),
                receiver: receiver.thing.endpoint(),
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

fn free_channel(flags: &[Flag]) -> Result<u16, String> {
    (0..=MAX_CHANNEL)
        .find(|&c| !flags.iter().any(|f| f.channel == c as i16))
        .ok_or_else(|| "Every teleporter channel is in use.".into())
}

/// Each of the map's own teleporter flags' channel, by base index: its
/// link's, else the map's. An added flag's end is not here: its row
/// carries the channel (`scene::project`).
pub(crate) fn effective(teleporters: &[Teleporter], links: &Links) -> BTreeMap<u32, i16> {
    let mut channels: BTreeMap<u32, i16> =
        teleporters.iter().map(|t| (t.index, t.channel)).collect();
    for active in &links.active {
        if let Link::Teleporter {
            sender,
            receiver,
            channel,
        } = active.link
        {
            for end in [sender, receiver] {
                if let Endpoint::Base { index, .. } = end {
                    channels.insert(index, channel as i16);
                }
            }
        }
    }
    channels
}

/// Refuse a linked own flag the map does not hold as a teleporter of that
/// role.
pub(crate) fn check_roles(teleporters: &[Teleporter], links: &Links) -> Result<(), String> {
    for active in &links.active {
        let Link::Teleporter {
            sender, receiver, ..
        } = active.link
        else {
            continue;
        };
        for (end, is_sender) in [(sender, true), (receiver, false)] {
            let Endpoint::Base { index, .. } = end else {
                continue;
            };
            if !teleporters
                .iter()
                .any(|t| t.index == index && t.sender == is_sender)
            {
                let role = if is_sender { "sender" } else { "receiver" };
                return Err(format!("Netgame flag {index} is not a teleporter {role}."));
            }
        }
    }
    Ok(())
}
