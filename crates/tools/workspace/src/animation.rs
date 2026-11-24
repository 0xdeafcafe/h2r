//! The package animation builder (docs/editor/studio-redesign.md,
//! "The animation editor"). It changes the clip's record and its raw
//! resource, keeping every unrelated field and fixup in the graph.

use anim_edit::{AnimDoc, ClipKind, Encoded, Operation, Source, encode};
use map::build::{Block, FIXED_STRING_SLOT, FixupKind, RawRef, TagPayload};
use map::{CacheType, FourCC, RawPointer, ResourceFile, TagId};
use tags::StringId;
use tags::anim::{
    ACTION_SIZE, ANIMATION_SIZE, Animation, AnimationType, CLASS_SIZE, HEADER_SIZE, MODE_SIZE,
    NODE_SIZE, RESOURCE_SIZE, TYPE_SIZE, hdr,
};

use crate::build::{Cx, Failure};
use crate::builders::Job;

const JMAD: FourCC = FourCC(*b"jmad");
/// Resources are read as whole sectors.
const RAW_ALIGN: u32 = 0x200;

// Animation record fields. The rest of a replaced record stays as it was.
const ANIM_TYPE: usize = 0x10;
const FRAME_INFO: usize = 0x11;
const BLEND_SCREEN: usize = 0x12;
const NODE_COUNT: usize = 0x13;
const FRAME_COUNT: usize = 0x14;
const FLAGS: usize = 0x16;
const RESOURCE_INDEX: usize = 0x24;
const DATA_OFFSET: usize = 0x28;
const LOOP_FRAME: usize = 0x2e;
const STATIC_FLAGS: usize = 0x3c;
const ANIMATED_FLAGS: usize = 0x3d;
const STATIC_SIZE: usize = 0x42;
const ANIMATED_SIZE: usize = 0x48;

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn half(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().expect("two bytes"))
}

fn fixup(block: &Block, at: usize) -> Option<&FixupKind> {
    block
        .fixups
        .iter()
        .find(|f| f.offset as usize == at)
        .map(|f| &f.kind)
}

/// The mutable children are separated from neighboring fields by the
/// graph's schema (including existing opaque record data).
fn rows(block: &Block, at: usize, stride: usize) -> Result<Option<&Block>, String> {
    match fixup(block, at) {
        Some(FixupKind::Block { count, child })
            if (*count as usize).checked_mul(stride) == Some(child.bytes.len()) =>
        {
            Ok(Some(child))
        }
        None if word(&block.bytes, at) == 0 => Ok(None),
        _ => Err(format!(
            "the graph's {at:#x} block is not available with {stride:#x}-byte records"
        )),
    }
}

fn rows_mut(block: &mut Block, at: usize, stride: usize) -> Result<(&mut u32, &mut Block), String> {
    if rows(block, at, stride)?.is_none() {
        block.block(at as u32, 0, Block::zeroed(0));
    }
    let field = block
        .fixups
        .iter_mut()
        .find(|f| f.offset as usize == at)
        .expect("block field");
    let FixupKind::Block { count, child } = &mut field.kind else {
        unreachable!("checked block")
    };
    // A builder places all blocks with one share key once. A changed array
    // must leave the other pointers to its old bytes alone.
    block.share = None;
    child.share = None;
    Ok((count, child))
}

fn append(count: &mut u32, array: &mut Block, mut row: Block) -> usize {
    let start = array.bytes.len();
    for fixup in &mut row.fixups {
        fixup.offset += start as u32;
    }
    array.bytes.append(&mut row.bytes);
    array.fixups.append(&mut row.fixups);
    *count += 1;
    start
}

fn row_name<'a>(
    array: &'a Block,
    at: usize,
    strings: &'a [Option<String>],
) -> Result<Option<&'a str>, String> {
    match fixup(array, at) {
        Some(FixupKind::StringId(name)) => Ok(Some(name)),
        None => StringId(word(&array.bytes, at))
            .resolve(strings)
            .map_err(|e| format!("unreadable graph name at {at:#x}: {e}")),
        _ => Err(format!("invalid graph name at {at:#x}")),
    }
}

fn named(
    array: &Block,
    stride: usize,
    name: &str,
    strings: &[Option<String>],
) -> Result<Option<usize>, String> {
    let mut found = None;
    for at in (0..array.bytes.len()).step_by(stride) {
        if row_name(array, at, strings)? == Some(name) && found.replace(at).is_some() {
            return Err(format!("graph name {name:?} is ambiguous"));
        }
    }
    Ok(found)
}

fn find_or_add(
    count: &mut u32,
    array: &mut Block,
    stride: usize,
    name: &str,
    strings: &[Option<String>],
) -> Result<usize, String> {
    array.sort_by_string_id = Some(stride as u32);
    if let Some(at) = named(array, stride, name, strings)? {
        return Ok(at);
    }
    let mut row = Block::zeroed(stride);
    row.string_id(0, name);
    Ok(append(count, array, row))
}

fn add_action(
    graph: &mut Block,
    strings: &[Option<String>],
    operation: &Operation,
    animation: i16,
) -> Result<(), String> {
    let Operation::Add {
        mode,
        class,
        weapon_type,
        action,
        kind,
    } = operation
    else {
        unreachable!("add operation")
    };
    let (count, modes) = rows_mut(graph, hdr::MODES, MODE_SIZE)?;
    let mode_at = find_or_add(count, modes, MODE_SIZE, mode, strings)?;
    let (count, classes) = rows_mut(modes, mode_at + 4, CLASS_SIZE)?;
    let class_at = find_or_add(count, classes, CLASS_SIZE, class, strings)?;
    let (count, types) = rows_mut(classes, class_at + 4, TYPE_SIZE)?;
    let type_at = find_or_add(count, types, TYPE_SIZE, weapon_type, strings)?;
    let list = if *kind == ClipKind::Overlay { 0xc } else { 4 };
    let (count, actions) = rows_mut(types, type_at + list, ACTION_SIZE)?;
    if named(actions, ACTION_SIZE, action, strings)?.is_some() {
        return Err(format!(
            "action {mode}:{class}:{weapon_type}:{action} already exists"
        ));
    }
    let mut row = Block::zeroed(ACTION_SIZE);
    row.string_id(0, action)
        .put_u16(4, u16::MAX) // This graph.
        .put_u16(6, animation as u16);
    actions.sort_by_string_id = Some(ACTION_SIZE as u32);
    append(count, actions, row);
    Ok(())
}

fn source(job: &Job<'_>) -> Result<(Source, AnimDoc), Failure> {
    let source = Source::parse(job.source)?;
    if job
        .item
        .tag
        .as_ref()
        .is_some_and(|tag| Cx::key(JMAD, tag) != Cx::key(JMAD, &source.graph))
    {
        return Err("the animation item's tag differs from the source graph".into());
    }
    if let Operation::Add {
        mode,
        class,
        weapon_type,
        action,
        ..
    } = &source.operation
    {
        for (field, value) in [
            ("name", &source.name),
            ("mode", mode),
            ("class", class),
            ("weapon_type", weapon_type),
            ("action", action),
        ] {
            if value.trim().is_empty() || value.len() >= FIXED_STRING_SLOT || value.contains('\0') {
                return Err(format!("{field} is not a valid map string ID name").into());
            }
        }
    }
    let doc = source.doc()?;
    Ok((source, doc))
}

pub fn check(job: &Job<'_>) -> Result<(), Failure> {
    source(job).map(|_| ())
}

fn check_header(
    bytes: &[u8],
    parent: Option<&FixupKind>,
    inheritance: Option<&FixupKind>,
) -> Result<(), String> {
    if bytes.len() < HEADER_SIZE {
        return Err("the graph header is truncated".into());
    }
    let parent = match parent {
        Some(FixupKind::TagRef(id)) => *id,
        None => TagId(word(bytes, 4)),
        _ => return Err("the graph's parent reference is invalid".into()),
    };
    let inherited = match inheritance {
        Some(FixupKind::Block { count, .. }) => *count,
        _ => word(bytes, hdr::INHERITANCE),
    };
    if parent != TagId::NULL || inherited != 0 {
        return Err("the graph inherits from a parent graph".into());
    }
    Ok(())
}

fn check_skeleton(
    graph: &Block,
    source: &Source,
    strings: &[Option<String>],
) -> Result<(), String> {
    let nodes = rows(graph, hdr::NODES, NODE_SIZE)?.ok_or("the graph has no skeleton")?;
    if nodes.bytes.len() / NODE_SIZE != source.nodes.len() {
        return Err(format!(
            "the graph has {} nodes, the source has {}",
            nodes.bytes.len() / NODE_SIZE,
            source.nodes.len()
        ));
    }
    for (i, want) in source.nodes.iter().enumerate() {
        let at = i * NODE_SIZE;
        let name = row_name(nodes, at, strings)?;
        let parent = half(&nodes.bytes, at + 8) as i16;
        if name != want.name.as_deref() || parent != want.parent {
            return Err(format!(
                "source node {i} ({:?}, parent {}) does not match the graph ({name:?}, parent {parent})",
                want.name, want.parent
            ));
        }
    }
    Ok(())
}

fn resource_is_shared(resources: &Block, at: usize) -> bool {
    let file = match fixup(resources, at + 8) {
        Some(FixupKind::Raw(RawRef::External { file, .. })) => Some(*file),
        None => RawPointer::decode(word(&resources.bytes, at + 8)).map(|p| p.file),
        _ => None,
    };
    matches!(
        file,
        Some(ResourceFile::Shared | ResourceFile::SinglePlayerShared)
    )
}

fn check_events(records: &Block, at: usize, frames: usize) -> Result<(), String> {
    for (offset, stride, label) in [(0x4c, 4, "frame"), (0x54, 8, "sound"), (0x5c, 4, "effect")] {
        if let Some(events) = rows(records, at + offset, stride)? {
            for event in events.bytes.chunks_exact(stride) {
                let frame = half(event, 2) as i16;
                if frame >= frames as i16 {
                    return Err(format!(
                        "{label} event at frame {frame} is outside the new clip"
                    ));
                }
            }
        }
    }
    let loop_frame = half(&records.bytes, at + LOOP_FRAME) as i16;
    if loop_frame >= frames as i16 {
        return Err(format!(
            "loop begins at frame {loop_frame} outside the new clip"
        ));
    }
    Ok(())
}

fn replacement_kind(record: &[u8]) -> Result<ClipKind, String> {
    if half(record, FLAGS) & Animation::FLAG_IN_RESOURCE == 0 {
        return Err("the animation has inline data".into());
    }
    if record[FRAME_INFO] != 0 {
        return Err(format!(
            "the animation has frame-info type {} (root movement)",
            record[FRAME_INFO]
        ));
    }
    ClipKind::try_from(AnimationType::from_u8(record[ANIM_TYPE]))
}

/// In a local pinned graph, diagnose the target before trying the schema
/// relocation. The relocator can refuse an inline clip's data as uncovered.
fn check_pinned_clip(
    bytes: &[u8],
    address: u32,
    operation: &Operation,
    strings: &[Option<String>],
) -> Result<(), String> {
    let Operation::Replace { animation } = operation else {
        return Ok(());
    };
    let span = || {
        let at = word(bytes, hdr::ANIMATIONS + 4).checked_sub(address)? as usize;
        let len = (word(bytes, hdr::ANIMATIONS) as usize).checked_mul(ANIMATION_SIZE)?;
        bytes.get(at..at.checked_add(len)?)
    };
    if let Some(array) = span() {
        for record in array.chunks_exact(ANIMATION_SIZE) {
            if StringId(word(record, 0)).resolve(strings).ok().flatten() == Some(animation.as_str())
            {
                replacement_kind(record).map_err(|why| format!("{animation:?}: {why}"))?;
            }
        }
    }
    Ok(())
}

/// What a replacement can inherit from the old record. A resource used
/// by another clip keeps its entry, its offsets and its contents.
fn replacement(
    graph: &Block,
    records: &Block,
    at: usize,
    doc: &AnimDoc,
) -> Result<(ClipKind, Option<usize>), String> {
    let old = &records.bytes[at..at + ANIMATION_SIZE];
    let kind = replacement_kind(old)?;
    if old[BLEND_SCREEN] != u8::MAX
        && rows(graph, hdr::BLEND_SCREENS, tags::anim::BLEND_SCREEN_SIZE)?
            .is_some_and(|b| !b.bytes.is_empty())
    {
        return Err("the animation is a blend screen".into());
    }
    check_events(records, at, doc.frames())?;
    let index = word(old, RESOURCE_INDEX) as usize;
    let resources = rows(graph, hdr::RESOURCES, RESOURCE_SIZE)?
        .ok_or("the animation's resource table is missing")?;
    if index >= resources.bytes.len() / RESOURCE_SIZE {
        return Err(format!("animation resource {index} is missing"));
    }
    if resource_is_shared(resources, index * RESOURCE_SIZE) {
        return Err("the animation resource is in a shared cache".into());
    }
    let users = records
        .bytes
        .chunks_exact(ANIMATION_SIZE)
        .filter(|r| {
            half(r, FLAGS) & Animation::FLAG_IN_RESOURCE != 0
                && word(r, RESOURCE_INDEX) as usize == index
        })
        .count();
    Ok((kind, (users == 1).then_some(index)))
}

fn install_resource(
    graph: &mut Block,
    owner: TagId,
    encoded: &Encoded,
    raw: RawRef,
    reuse: Option<usize>,
) -> Result<u32, String> {
    let (count, resources) = rows_mut(graph, hdr::RESOURCES, RESOURCE_SIZE)?;
    let size = u32::try_from(encoded.bytes.len()).map_err(|_| "animation resource exceeds u32")?;
    if let Some(index) = reuse {
        let at = index * RESOURCE_SIZE;
        resources.fixups.retain(|f| f.offset as usize != at + 8);
        resources
            .put_u32(at + 4, size)
            .raw((at + 8) as u32, raw)
            .put_u32(at + 0xc, u32::MAX)
            .put_u32(at + 0x10, 0);
        return Ok(index as u32);
    }
    let index = *count;
    let mut resource = Block::zeroed(RESOURCE_SIZE);
    resource
        .tag_id(0, owner)
        .put_u32(4, size)
        .raw(8, raw)
        .put_u32(0xc, u32::MAX);
    append(count, resources, resource);
    Ok(index)
}

fn install_stream(records: &mut Block, at: usize, doc: &AnimDoc, encoded: &Encoded, resource: u32) {
    records.bytes[at + NODE_COUNT] = doc.nodes().len() as u8;
    records.bytes[at + STATIC_FLAGS] = encoded.flag_size;
    records.bytes[at + ANIMATED_FLAGS] = encoded.flag_size;
    records
        .put_u16(at + FRAME_COUNT, doc.frames() as u16)
        .put_u32(at + RESOURCE_INDEX, resource)
        .put_u32(at + DATA_OFFSET, 0)
        .put_u16(at + STATIC_SIZE, encoded.static_size)
        .put_u32(at + ANIMATED_SIZE, encoded.animated_size);
}

fn write(
    cx: &mut Cx<'_>,
    key: &tags::repo::TagKey,
    source: Source,
    doc: &AnimDoc,
) -> Result<(), String> {
    if matches!(
        cx.set.cache_type,
        CacheType::Shared | CacheType::SinglePlayerShared
    ) {
        return Err("the graph is in a shared cache".into());
    }
    let id = cx.find(key).ok_or("the graph is absent from this map")?;
    match &cx.set.tag(id).expect("found").payload {
        TagPayload::Pinned { address, bytes } => {
            check_header(bytes, None, None)?;
            check_pinned_clip(bytes, *address, &source.operation, &cx.set.strings)?;
        }
        TagPayload::Empty if cx.set.pinned.is_some() => {
            return Err("the graph has no local metadata in this cache".into());
        }
        _ => {}
    }
    let mut graph = cx.read_owned_block(id)?;
    check_header(
        &graph.bytes,
        fixup(&graph, 0),
        fixup(&graph, hdr::INHERITANCE),
    )?;
    check_skeleton(&graph, &source, &cx.set.strings)?;

    let records = rows(&graph, hdr::ANIMATIONS, ANIMATION_SIZE)?;
    let (at, kind, reuse, edited) = match &source.operation {
        Operation::Replace { animation } => {
            let records = records.ok_or("the graph has no animations")?;
            let at = named(records, ANIMATION_SIZE, animation, &cx.set.strings)?
                .ok_or_else(|| format!("animation {animation:?} was not found"))?;
            let (kind, reuse) = replacement(&graph, records, at, doc)
                .map_err(|why| format!("{animation:?}: {why}"))?;
            (at, kind, reuse, animation.as_str())
        }
        Operation::Add { kind, .. } => {
            let at = records.map_or(0, |r| r.bytes.len());
            let animation = i16::try_from(at / ANIMATION_SIZE)
                .map_err(|_| "the graph has no free i16 animation index")?;
            if let Some(records) = records
                && named(records, ANIMATION_SIZE, &source.name, &cx.set.strings)?.is_some()
            {
                return Err(format!("animation {:?} already exists", source.name));
            }
            // The name, type, no-screen sentinel and linkage of an added
            // record use the same semantics as the Halo 1 converter.
            let mut row = Block::zeroed(ANIMATION_SIZE);
            row.string_id(0, &source.name)
                .put_u32(
                    4,
                    records
                        .filter(|r| !r.bytes.is_empty())
                        .map_or(0, |r| word(&r.bytes, 4)),
                )
                .put_f32(0x1c, 1.0)
                .put_u16(FLAGS, Animation::FLAG_IN_RESOURCE)
                .put_u16(0x2c, u16::MAX)
                .put_u16(0x30, u16::MAX)
                .put_u16(0x32, u16::MAX)
                .put_u32(0x20, u32::MAX);
            let animation_type: AnimationType = (*kind).into();
            row.bytes[ANIM_TYPE] = animation_type.as_u8();
            row.bytes[BLEND_SCREEN] = u8::MAX;
            add_action(&mut graph, &cx.set.strings, &source.operation, animation)?;
            let (count, records) = rows_mut(&mut graph, hdr::ANIMATIONS, ANIMATION_SIZE)?;
            append(count, records, row);
            (at, *kind, None, source.name.as_str())
        }
    };

    let encoded = encode(doc, kind).map_err(|why| format!("{edited:?}: {why}"))?;
    let raw = RawRef::Local(map::build::RawId(cx.set.raw.len()));
    let resource = install_resource(&mut graph, id, &encoded, raw, reuse)?;
    let (_, records) = rows_mut(&mut graph, hdr::ANIMATIONS, ANIMATION_SIZE)?;
    install_stream(records, at, doc, &encoded, resource);
    cx.touch(id)?;
    cx.write_block(id, key, graph)?;
    let actual = cx.set.add_raw(encoded.bytes, RAW_ALIGN);
    debug_assert_eq!(raw, RawRef::Local(actual));
    cx.note_set(key, &format!("animation {edited:?}"));
    Ok(())
}

pub fn apply(cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    let (source, doc) = source(job)?;
    let key = Cx::key(JMAD, &source.graph);
    write(cx, &key, source, &doc).map_err(|why| format!("{key}: {why}").into())
}

#[cfg(test)]
mod tests;
