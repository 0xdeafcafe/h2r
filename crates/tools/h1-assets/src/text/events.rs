//! Bind native event slots to the Halo 1 message templates and their inputs.

use h1map::{Group, H1Map, Tag, TagRef};
use map::build::{FixupKind, TagPayload, TagSet};
use map::{FourCC, TagId};

use crate::text::h1::{Hmt, HudMessage, MessagePart, Ustr};
use crate::text::silhouettes::{Atlas, Silhouette};
use crate::text::{Error, Result};

#[derive(Clone)]
pub(crate) struct Event {
    group: FourCC,
    object: String,
    field: Field,
    pub key: String,
    pub template: usize,
    pub args: [Option<usize>; 2],
}

#[derive(Clone, Default)]
pub(crate) struct Events {
    pub entries: Vec<HudMessage>,
    pub routes: Vec<Event>,
    pub control_phrases: Vec<Option<usize>>,
    pub custom_device: Option<usize>,
    pub exit_vehicle: Option<usize>,
    pub swap_powerup: Option<usize>,
    pub subjects: Vec<Subject>,
    pub silhouettes: Vec<Silhouette>,
}

#[derive(Clone)]
pub(crate) struct Subject {
    pub group: FourCC,
    pub object: String,
    pub argument: usize,
}

#[derive(Clone, Copy)]
enum Field {
    Root(u32),
    Seat(usize),
}

impl Event {
    pub fn target(&self) -> (FourCC, &str) {
        (self.group, &self.object)
    }

    pub fn is_control(&self) -> bool {
        self.group == FourCC(*b"ctrl")
    }

    pub fn write(&self, set: &mut TagSet) {
        if let Some(id) = set.find(self.group, &self.object) {
            self.write_at(set, id);
        }
    }

    pub fn write_at(&self, set: &mut TagSet, id: TagId) -> bool {
        let Some(TagPayload::Built(root)) = set.tag_mut(id).map(|tag| &mut tag.payload) else {
            return false;
        };
        let (block, offset) = match self.field {
            Field::Root(offset) => (root, offset),
            Field::Seat(i) => {
                let Some(FixupKind::Block { child, count }) = root
                    .fixups
                    .iter_mut()
                    .find(|f| f.offset == 0x1c8)
                    .map(|f| &mut f.kind)
                else {
                    return false;
                };
                if i >= *count as usize {
                    return false;
                }
                (child, 0xb0 * i as u32 + 0x84)
            }
        };
        if offset as usize + 4 > block.bytes.len() {
            return false;
        }
        block.fixups.retain(|f| f.offset != offset);
        block.string_id(offset, &self.key);
        true
    }
}

struct Builder {
    entries: Vec<HudMessage>,
    routes: Vec<Event>,
    template_count: usize,
}

impl Builder {
    fn row(&mut self, parts: Vec<MessagePart>) -> usize {
        let index = self.entries.len();
        self.entries.push(HudMessage {
            name: format!("event_value_{index}"),
            parts,
        });
        index
    }

    fn text(&mut self, text: &str) -> usize {
        self.row(vec![MessagePart::Text(text.into())])
    }

    fn route(&mut self, tag: &Tag, field: Field, template: usize, args: [Option<usize>; 2]) {
        self.routes.push(Event {
            group: FourCC(tag.group.0.to_be_bytes()),
            object: tag.name.clone(),
            field,
            key: format!("h1_hud_event_{}", self.routes.len()),
            template,
            args,
        });
    }

    fn prompt(&mut self, tag: &Tag, field: Field, name: &str, args: [Option<usize>; 2]) {
        if let Some(template) = self.template(name) {
            self.route(tag, field, template, args);
        }
    }

    fn template(&self, name: &str) -> Option<usize> {
        self.entries[..self.template_count]
            .iter()
            .position(|message| message.name == name)
    }
}

fn short(bytes: &[u8], at: usize) -> Option<usize> {
    let index = i16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?);
    usize::try_from(index).ok()
}

fn strings(map: &H1Map, tag: Option<TagRef>) -> Result<Vec<String>> {
    match tag.filter(|r| !r.id.is_none()) {
        Some(r) => Ok(Ustr::read(map, map.tag(r.id)?)?.strings),
        None => Ok(Vec::new()),
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Scope {
    Campaign(Option<TagRef>),
    Multiplayer { weapons: bool },
}

impl Scope {
    fn includes(self, group: &[u8; 4]) -> bool {
        match self {
            Self::Campaign(_) => matches!(group, b"weap" | b"eqip" | b"vehi" | b"ctrl"),
            Self::Multiplayer { weapons } => group == b"eqip" || (weapons && group == b"weap"),
        }
    }
}

fn weapon_argument(scope: Scope, pickup: &str, name: Option<&str>, sprite: Option<char>) -> String {
    match (scope, sprite) {
        (Scope::Multiplayer { .. }, Some(sprite)) => format!("{} {sprite}", item_name(pickup)),
        (Scope::Multiplayer { .. }, None) => item_name(pickup).to_owned(),
        (Scope::Campaign(_), Some(sprite)) => sprite.to_string(),
        (Scope::Campaign(_), None) => name.unwrap_or_else(|| item_name(pickup)).to_owned(),
    }
}

pub(crate) fn read(map: &H1Map, scope: Scope) -> Result<Events> {
    let Some(globals) = map
        .tags()
        .iter()
        .find(|tag| tag.group == Group::new(b"hudg"))
    else {
        return Ok(Events::default());
    };
    let globals = map.tag_data(globals)?;
    let Some(reference) = TagRef::read(globals, 0xf0).filter(|r| !r.id.is_none()) else {
        return Ok(Events::default());
    };
    let entries = Hmt::read(map, map.tag(reference.id)?)?.messages;
    let items = strings(map, TagRef::read(globals, 0x94))?;
    let icons = strings(map, TagRef::read(globals, 0xb4))?;
    let object_names = match scope {
        Scope::Campaign(names) => strings(map, names)?,
        Scope::Multiplayer { .. } => Vec::new(),
    };
    let mut atlas = if scope.includes(b"weap") {
        Atlas::read(map, globals)?
    } else {
        None
    };
    let mut silhouettes = Vec::new();
    let mut subjects = Vec::new();
    let mut b = Builder {
        template_count: entries.len(),
        entries,
        routes: Vec::new(),
    };
    if let Scope::Multiplayer { weapons } = scope {
        for template in ["swap_powerup"].into_iter().chain(
            ["pickup", "swap_weapon", "remind_to_switch_weapons"]
                .into_iter()
                .filter(|_| weapons),
        ) {
            if b.template(template).is_none() {
                return Err(Error::Invalid(format!("missing HUD template {template}")));
            }
        }
    }
    for tag in map.tags() {
        let group = tag.group.0.to_be_bytes();
        if !scope.includes(&group) {
            continue;
        }
        let bytes = map.tag_data(tag)?;
        let name = short(bytes, 0x13c)
            .filter(|&i| i != 0)
            .and_then(|i| icons.get(i).map(String::as_str));
        match &group {
            b"weap" | b"eqip" => {
                let pickup = short(bytes, 0x180)
                    .filter(|&i| i != 0)
                    .and_then(|i| items.get(i).map(|text| (i, text)));
                if matches!(scope, Scope::Multiplayer { .. })
                    && group == *b"eqip"
                    && matches!(short(bytes, 0x308), Some(2 | 3 | 5))
                    && pickup.is_none_or(|(_, text)| text.is_empty())
                {
                    return Err(Error::Invalid(format!("missing pickup text for {}", tag.name)));
                }
                if let Some((i, text)) = pickup {
                    let single = b.text(text);
                    b.route(tag, Field::Root(0xe0), single, [None; 2]);
                    if let Some(plural) = items.get(i + 1).and_then(|s| quantity(s)) {
                        let many = b.text(&plural);
                        b.route(tag, Field::Root(0xe4), single, [None; 2]);
                        b.route(tag, Field::Root(0xe8), many, [None; 2]);
                    }
                }
                if group == *b"weap"
                    && let Some((_, pickup)) = pickup
                {
                    let sprite = match &mut atlas {
                        Some(atlas) => atlas.weapon(map, bytes, &mut silhouettes)?,
                        None => None,
                    };
                    let arg = b.text(&weapon_argument(scope, pickup, name, sprite));
                    for (field, template) in [
                        (0xcc, "pickup"),
                        (0xd0, "swap_weapon"),
                        (0xd4, "pickup"),
                        (0xd8, "swap_weapon"),
                        (0xdc, "pickup"),
                        (0xec, "remind_to_switch_weapons"),
                    ] {
                        b.prompt(tag, Field::Root(field), template, [Some(arg), None]);
                    }
                }
                if group == *b"eqip" && matches!(short(bytes, 0x308), Some(1..=5)) {
                    // Stock Xbox equipment has icon row 0, the globals'
                    // placeholder. Use the equipment's own localized pickup
                    // noun when it has no authored icon label.
                    let label = name
                        .or_else(|| pickup.map(|(_, text)| item_name(text)))
                        .unwrap_or_else(|| tag.name.rsplit('\\').next().unwrap_or(&tag.name));
                    subjects.push(Subject {
                        group: FourCC(group),
                        object: tag.name.clone(),
                        argument: b.text(label),
                    });
                }
            }
            b"vehi" => {
                let Some(name) = name else { continue };
                let vehicle = b.text(name);
                subjects.push(Subject {
                    group: FourCC(group),
                    object: tag.name.clone(),
                    argument: vehicle,
                });
                b.prompt(
                    tag,
                    Field::Root(0x21c),
                    "flip_vehicle",
                    [Some(vehicle), None],
                );
                let Some(seats) = h1map::Block::read(bytes, 0x2e4) else {
                    continue;
                };
                for (i, seat) in map.block(seats, 0x11c)?.as_chunks::<0x11c>().0.iter().enumerate() {
                    let Some(label) = short(seat, 0xec).and_then(|i| icons.get(i)) else {
                        continue;
                    };
                    let seat = b.text(label);
                    b.prompt(
                        tag,
                        Field::Seat(i),
                        "enter_vehicle",
                        [Some(seat), Some(vehicle)],
                    );
                }
            }
            b"ctrl" => {
                let leaf = tag.name.rsplit('\\').next().unwrap_or(&tag.name);
                let arg = b.text(name.unwrap_or(leaf));
                b.prompt(tag, Field::Root(0x124), "touch_device", [Some(arg), None]);
            }
            _ => unreachable!(),
        }
    }
    let custom_device = b.template("custom_device").filter(|_| scope.includes(b"ctrl"));
    let exit_vehicle = b.template("exit_vehicle").filter(|_| scope.includes(b"vehi"));
    let swap_powerup = b.template("swap_powerup");
    let mut control_phrases = Vec::new();
    if custom_device.is_some() {
        let scenario = map.tag_data(map.scenario())?;
        if let Some(controls) = h1map::Block::read(scenario, 0x2ac) {
            for control in map.block(controls, 0x40)?.as_chunks::<0x40>().0 {
                // 0x118f16 reads a one-based scenario-string row
                // from the placement.
                let phrase = short(control, 0x34)
                    .and_then(|row| row.checked_sub(1))
                    .and_then(|row| object_names.get(row))
                    .filter(|s| !s.is_empty())
                    .map(|phrase| b.text(phrase));
                control_phrases.push(phrase);
            }
        }
    }
    Ok(Events {
        entries: b.entries,
        routes: b.routes,
        control_phrases,
        custom_device,
        exit_vehicle,
        swap_powerup,
        subjects,
        silhouettes,
    })
}

fn item_name(pickup: &str) -> &str {
    let name = pickup.strip_prefix("Picked up ").unwrap_or(pickup);
    ["a ", "an ", "the "]
        .into_iter()
        .find_map(|article| name.strip_prefix(article))
        .unwrap_or(name)
}

fn quantity(text: &str) -> Option<String> {
    let (before, after) = text.split_once("%d")?;
    Some(format!("{before}\u{e422}{after}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_weapon_labels_come_from_source_rows() {
        assert_eq!(
            quantity("Found %d widgets").as_deref(),
            Some("Found \u{e422} widgets")
        );
        assert_eq!(quantity("Found one widget"), None);
        assert_eq!(item_name("Picked up an example"), "example");
    }

    #[test]
    fn multiplayer_weapon_arguments_include_the_label_and_authored_icon() {
        let scope = Scope::Multiplayer { weapons: true };
        let notice = "Picked up an example";
        assert_eq!(
            weapon_argument(scope, notice, Some("unrelated icon label"), Some('\u{e500}')),
            "example \u{e500}",
        );
        assert_eq!(weapon_argument(scope, notice, None, None), "example");
        assert_eq!(
            weapon_argument(Scope::Campaign(None), notice, None, Some('\u{e500}')),
            "\u{e500}",
        );
    }
}
