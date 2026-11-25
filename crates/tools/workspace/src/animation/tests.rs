//! Invented graphs and key values for AN4.

use std::io::Cursor;

use anim_edit::{AnimDoc, Node, Operation, Source};
use map::{Map, TagId};
use tags::anim::{AnimationGraph, DataResource, WeaponType};
use tags::anim_keys::Kind;

use super::*;
use crate::tests::{base, pkg, target};
use crate::{Loaded, build, build_map, check_package};

const GRAPH: &str = "invented\\moves";
const SOURCE: &str = "moves.anim.yaml";
const FIRST: &str = "first";
const SECOND: &str = "second";

fn clip(name: &str, end: f32) -> AnimDoc {
    let mut doc = AnimDoc::new(
        name.to_owned(),
        3,
        vec![Node {
            name: Some("joint".into()),
            parent: -1,
        }],
    )
    .unwrap();
    doc.set_key(0, Kind::Translation, 0, [0.0; 4], None)
        .unwrap();
    doc.set_key(0, Kind::Translation, 2, [end, 0.0, 0.0, 0.0], None)
        .unwrap();
    doc.set_key(0, Kind::Scale, 0, [1.25, 0.0, 0.0, 0.0], None)
        .unwrap();
    doc
}

fn item(doc: &AnimDoc, op: Operation) -> Loaded {
    let source = Source::from_doc(GRAPH.to_owned(), op, doc);
    let yaml = source.to_yaml().unwrap();
    pkg(
        "moves",
        &[],
        &[("animation", SOURCE, Some(GRAPH), "all")],
        &[(SOURCE, yaml.as_bytes())],
    )
}

fn replace(animation: &str, end: f32) -> Loaded {
    item(
        &clip(animation, end),
        Operation::Replace {
            animation: animation.into(),
        },
    )
}

fn add(name: &str, mode: &str, action: &str, kind: ClipKind) -> Loaded {
    item(
        &clip(name, 8.0),
        Operation::Add {
            mode: mode.into(),
            class: "class".into(),
            weapon_type: "type".into(),
            action: action.into(),
            kind,
        },
    )
}

fn fixture(shared_resource: bool) -> map::build::TagSet {
    let mut set = base();
    set.add_group(JMAD, None, None);
    let id = set.add_tag(JMAD, GRAPH, TagPayload::Empty).unwrap();
    let mut graph = Block::zeroed(HEADER_SIZE);
    graph.tag_ref(0, TagId::NULL);
    graph.put_u32(0x60, 0x12); // Unrelated graph flags.
    let mut node = Block::zeroed(NODE_SIZE);
    node.string_id(0, "joint")
        .put_u16(4, u16::MAX)
        .put_u16(6, u16::MAX)
        .put_u16(8, u16::MAX);
    graph.block(hdr::NODES as u32, 1, node);

    let mut records = Block::zeroed(0);
    let mut streams = Block::zeroed(0);
    let (mut animation_count, mut resource_count) = (0, 0);
    let mut shared_bytes = Vec::new();
    for (i, name) in [FIRST, SECOND].into_iter().enumerate() {
        let doc = clip(name, i as f32 + 1.0);
        let encoded = encode(&doc, ClipKind::Base).unwrap();
        let mut record = Block::zeroed(ANIMATION_SIZE);
        record
            .string_id(0, name)
            .put_u32(4, 0x10)
            .put_u32(8, i as u32 + 19)
            .put_u16(FLAGS, Animation::FLAG_IN_RESOURCE)
            .put_f32(0x1c, 1.0)
            .put_u16(0x2c, u16::MAX)
            .put_u16(0x30, u16::MAX)
            .put_u16(0x32, u16::MAX);
        record.bytes[BLEND_SCREEN] = u8::MAX;
        let mut event = Block::zeroed(4);
        event.put_u16(0, 1).put_u16(2, 1);
        record.block(0x4c, 1, event);
        let resource = if shared_resource { 0 } else { i as u32 };
        install_stream(&mut record, 0, &doc, &encoded, resource);
        if shared_resource {
            record.put_u32(DATA_OFFSET, shared_bytes.len() as u32);
            shared_bytes.extend(encoded.bytes);
        } else {
            add_resource(
                &mut set,
                id,
                &mut streams,
                &mut resource_count,
                encoded.bytes,
            );
        }
        append(&mut animation_count, &mut records, record);
    }
    if shared_resource {
        add_resource(
            &mut set,
            id,
            &mut streams,
            &mut resource_count,
            shared_bytes,
        );
    }
    graph
        .block(hdr::ANIMATIONS as u32, animation_count, records)
        .block(hdr::RESOURCES as u32, resource_count, streams);
    let action = Operation::Add {
        mode: "mode".into(),
        class: "class".into(),
        weapon_type: "type".into(),
        action: "existing_action".into(),
        kind: ClipKind::Base,
    };
    add_action(&mut graph, &set.strings, &action, 1).unwrap();
    set.tag_mut(id).unwrap().payload = TagPayload::Built(graph);
    set
}

fn add_resource(
    set: &mut map::build::TagSet,
    id: TagId,
    streams: &mut Block,
    count: &mut u32,
    bytes: Vec<u8>,
) {
    let mut res = Block::zeroed(RESOURCE_SIZE);
    res.tag_id(0, id)
        .put_u32(4, bytes.len() as u32)
        .put_u32(0xc, u32::MAX);
    res.raw(8, RawRef::Local(set.add_raw(bytes, RAW_ALIGN)));
    append(count, streams, res);
}

type FixtureMap = Map<Cursor<Vec<u8>>>;

fn read(bytes: Vec<u8>) -> (FixtureMap, AnimationGraph) {
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let id = map.tags().iter().find(|t| t.group == JMAD).unwrap().id;
    let graph = AnimationGraph::read(&mut map, id).unwrap();
    (map, graph)
}

fn raw(map: &mut FixtureMap, resource: &DataResource) -> Vec<u8> {
    let p = resource.pointer.unwrap();
    assert_eq!(p.file, ResourceFile::Local);
    map.read_file_range(u64::from(p.offset), u64::from(resource.size))
        .unwrap()
}

fn final_x(map: &mut FixtureMap, graph: &AnimationGraph, anim: usize) -> f32 {
    let data = raw(
        map,
        &graph.resources[graph.animations[anim].resource_index as usize],
    );
    assert_eq!(graph.engine_problems(anim, &data), Vec::<String>::new());
    let clip = graph.clip_from_resource(anim, &data).unwrap();
    let pose = clip.sample_frame(clip.frame_count() - 1).unwrap();
    pose.nodes[0].translation[0]
}

#[test]
fn replacing_one_clip_on_a_map_preserves_the_other_record_and_shared_raw() {
    let set = fixture(true);
    let (before, _) = set.build().unwrap();
    let package = replace(FIRST, 7.0);
    assert!(check_package(&package).is_empty());
    let (after, report) = build_map(&before, &[package], &target()).unwrap();
    assert_eq!(
        report.changed.into_iter().collect::<Vec<_>>(),
        [Cx::key(JMAD, GRAPH)]
    );

    let (mut before, old) = read(before);
    let (mut after, graph) = read(after);
    assert_eq!(graph.animations[1], old.animations[1]);
    assert_eq!(graph.animations[0].name, old.animations[0].name);
    assert_eq!(graph.animations[0].checksums, old.animations[0].checksums);
    assert_eq!(
        graph.animations[0].frame_events,
        old.animations[0].frame_events
    );
    assert_eq!(graph.nodes, old.nodes);
    assert_eq!(graph.modes, old.modes);
    assert_eq!(graph.tail_words, old.tail_words);
    assert_eq!(graph.resources.len(), 2);
    assert_eq!(old.resources[0], graph.resources[0]);
    assert_eq!(
        raw(&mut before, &old.resources[0]),
        raw(&mut after, &graph.resources[0])
    );
    assert_eq!(final_x(&mut after, &graph, 1), 2.0);
    assert_eq!(final_x(&mut after, &graph, 0), 7.0);
}

#[test]
fn replacing_a_clip_in_a_fresh_set_reuses_its_exclusive_resource_slot() {
    let mut set = fixture(false);
    let (old_map, _) = set.build().unwrap();
    build(&mut set, &[replace(FIRST, 11.0)], &target(), None).unwrap();
    let (bytes, _) = set.build().unwrap();
    let (mut old_map, old) = read(old_map);
    let (mut map, graph) = read(bytes);
    assert_eq!(graph.resources.len(), old.resources.len());
    assert_eq!(
        graph.animations[0].resource_index,
        old.animations[0].resource_index
    );
    assert_eq!(graph.animations[1], old.animations[1]);
    assert_eq!(
        raw(&mut old_map, &old.resources[1]),
        raw(&mut map, &graph.resources[1])
    );
    assert_eq!(final_x(&mut map, &graph, 0), 11.0);
}

fn packed_id(map: &FixtureMap, name: &str) -> i32 {
    let index = map
        .strings()
        .iter()
        .position(|s| s.as_deref() == Some(name))
        .unwrap();
    (index as u32 | (name.len() as u32) << 24) as i32
}

fn weapon_type<'a>(map: &FixtureMap, graph: &'a AnimationGraph, mode: &str) -> &'a WeaponType {
    // The engine binary-searches these arrays in packed string-ID order.
    let i = graph
        .modes
        .binary_search_by_key(&packed_id(map, mode), |m| m.name.0 as i32)
        .unwrap();
    let classes = &graph.modes[i].classes;
    let i = classes
        .binary_search_by_key(&packed_id(map, "class"), |c| c.name.0 as i32)
        .unwrap();
    let types = &classes[i].types;
    let i = types
        .binary_search_by_key(&packed_id(map, "type"), |t| t.name.0 as i32)
        .unwrap();
    &types[i]
}

#[test]
fn an_added_clip_is_reachable_by_its_action_and_overlay_paths() {
    let mut set = fixture(false);
    let packages = [
        add("new base", "mode", "go", ClipKind::Base),
        add("new overlay", "m", "watch", ClipKind::Overlay),
    ];
    build(&mut set, &packages, &target(), None).unwrap();
    let (bytes, _) = set.build().unwrap();
    let (mut map, graph) = read(bytes);
    assert_eq!(graph.animations.len(), 4);
    for (name, actions, index) in [
        ("go", &weapon_type(&map, &graph, "mode").actions, 2),
        (
            "existing_action",
            &weapon_type(&map, &graph, "mode").actions,
            1,
        ),
        ("watch", &weapon_type(&map, &graph, "m").overlays, 3),
    ] {
        let i = actions
            .binary_search_by_key(&packed_id(&map, name), |a| a.name.0 as i32)
            .unwrap();
        assert_eq!(actions[i].animation_index, index);
        assert_eq!(actions[i].graph_index, -1);
    }
    assert_eq!(final_x(&mut map, &graph, 2), 8.0);
    assert_eq!(graph.animations[3].anim_type, AnimationType::Overlay);
    assert_eq!(final_x(&mut map, &graph, 3), 8.0);
}

fn graph_mut(set: &mut map::build::TagSet) -> &mut Block {
    let id = set.find(JMAD, GRAPH).unwrap();
    let TagPayload::Built(graph) = &mut set.tag_mut(id).unwrap().payload else {
        panic!("built graph")
    };
    graph
}

fn refused(set: &mut map::build::TagSet, package: Loaded, reason: &str) {
    let before = set.clone();
    let error = build(set, &[package], &target(), None).unwrap_err();
    assert!(
        error.why.contains(GRAPH) && error.why.contains(reason),
        "{error}"
    );
    assert_eq!(*set, before);
}

#[test]
fn invalid_write_targets_are_refused_by_name() {
    let mut inherited = fixture(false);
    let id = inherited.find(JMAD, GRAPH).unwrap();
    graph_mut(&mut inherited).fixups.retain(|f| f.offset != 0);
    graph_mut(&mut inherited).tag_ref(0, id);
    refused(
        &mut inherited,
        replace(FIRST, 9.0),
        "inherits from a parent graph",
    );

    let mut shared = fixture(false);
    shared.cache_type = CacheType::Shared;
    refused(&mut shared, replace(FIRST, 9.0), "shared cache");

    let mut inline = fixture(false);
    rows_mut(graph_mut(&mut inline), hdr::ANIMATIONS, ANIMATION_SIZE)
        .unwrap()
        .1
        .put_u16(FLAGS, 0);
    refused(&mut inline, replace(FIRST, 9.0), "inline data");

    let mut moving = fixture(false);
    rows_mut(graph_mut(&mut moving), hdr::ANIMATIONS, ANIMATION_SIZE)
        .unwrap()
        .1
        .bytes[FRAME_INFO] = 1;
    refused(&mut moving, replace(FIRST, 9.0), "frame-info type 1");

    let mut collision = fixture(false);
    refused(
        &mut collision,
        add("different name", "mode", "existing_action", ClipKind::Base),
        "already exists",
    );
}
