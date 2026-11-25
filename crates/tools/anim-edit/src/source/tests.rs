use super::*;

use tags::StringId;
use tags::anim::{Animation, DataSizes};
use tags::anim_keys::distance;
use tags::anim_sample::{AnimationClip, Codec};

fn nodes(n: usize) -> Vec<Node> {
    (0..n)
        .map(|i| Node {
            name: Some(format!("joint_{i}")),
            parent: if i == 0 { -1 } else { 0 },
        })
        .collect()
}

fn decode(doc: &AnimDoc, kind: ClipKind, r: Encoded) -> AnimationClip {
    let record = Animation {
        name: StringId(0),
        node_list_checksum: 0,
        checksums: [0; 2],
        anim_type: kind.into(),
        frame_info_type: 0,
        unknown_12: 0xff,
        node_count: doc.nodes().len() as u8,
        frame_count: doc.frames() as i16,
        flags: Animation::FLAG_IN_RESOURCE,
        unknown_18: [0; 8],
        unknown_20: 0,
        resource_index: 0,
        data_offset: 0,
        static_pose: -1,
        unknown_2e: 0,
        link_a: -1,
        link_b: -1,
        inline_size: 0,
        inline_address: 0,
        sizes: DataSizes {
            static_flags: r.flag_size,
            animated_flags: r.flag_size,
            unknown: [0; 2],
            unknown_4: 0,
            static_size: r.static_size,
            unknown_8: 0,
            animated_size: r.animated_size,
        },
        frame_events: Vec::new(),
        sound_events: Vec::new(),
        effect_events: Vec::new(),
        extra_block_count: 0,
    };
    AnimationClip::new(&record, r.bytes).unwrap()
}

#[test]
fn source_round_trip_keeps_sparse_tracks_and_quaternions_with_fresh_history() {
    let mut doc = AnimDoc::new("folded-pose".to_owned(), 8, nodes(2)).unwrap();
    let q = crate::angles_to_rotation([12.0, 18.0, 25.0]);
    doc.set_key(1, Kind::Rotation, 1, q, Some(Interp::Smooth))
        .unwrap();
    doc.set_key(1, Kind::Rotation, 7, [0.0, 0.0, 0.0, 1.0], None)
        .unwrap();
    doc.set_tangent(1, Kind::Rotation, 1, [0.0, 0.03, 0.0, 0.0], None)
        .unwrap();
    let source = Source::from_doc(
        r"invented\armature".to_owned(),
        Operation::Replace {
            animation: "resting".to_owned(),
        },
        &doc,
    );
    let yaml = source.to_yaml().unwrap();
    let read = Source::parse(yaml.as_bytes()).unwrap();
    assert_eq!(read, source);
    let clip = read.doc().unwrap();
    assert_eq!(clip.nodes(), doc.nodes());
    assert_eq!(clip.frames(), doc.frames());
    assert_eq!(clip.keys(1, Kind::Rotation), doc.keys(1, Kind::Rotation));
    assert_eq!(clip.bake(), doc.bake());
    assert!(clip.keys(0, Kind::Translation).is_empty());
    assert_eq!(clip.revision(), 0);
    assert!(!clip.can_undo());
}

#[test]
fn yaml_defaults_and_validation_keep_channels_unambiguous() {
    let yaml = br#"
graph: invented\toy
name: gesture
operation:
  op: add
  mode: curious
  class: unarmed
  action: wave
frames: 5
nodes:
  - name: shoulder
    parent: -1
channels:
  - node: 0
    channel: translation
    keys:
      - frame: 1
        value: [2.0, 0.0, -1.0]
"#;
    let source = Source::parse(yaml).unwrap();
    assert!(matches!(
        &source.operation,
        Operation::Add { weapon_type, kind: ClipKind::Base, .. } if weapon_type == "any"
    ));
    let doc = source.doc().unwrap();
    assert_eq!(
        doc.key(0, Kind::Translation, 1).unwrap().interp,
        Interp::Linear
    );

    let mut malformed = source.clone();
    malformed.channels[0].keys[0].value.pop();
    assert!(
        malformed
            .doc()
            .unwrap_err()
            .contains("value needs 3 values")
    );
    let mut malformed = source.clone();
    malformed.channels[0].keys[0].value[0] = f32::INFINITY;
    assert!(malformed.doc().unwrap_err().contains("must be finite"));
    let mut malformed = source.clone();
    malformed.channels[0].keys[0].frame = source.frames;
    assert!(malformed.doc().unwrap_err().contains("no frame 5"));
    let mut malformed = source.clone();
    malformed.channels.extend(source.channels);
    assert!(
        malformed
            .doc()
            .unwrap_err()
            .contains("channel appears twice")
    );
}

#[test]
fn bake_keeps_unkeyed_channels_and_supports_the_full_node_index() {
    let mut doc = AnimDoc::new("mount".to_owned(), 5, nodes(255)).unwrap();
    doc.set_key(239, Kind::Scale, 0, [2.0, 0.0, 0.0, 0.0], None)
        .unwrap();
    doc.set_key(254, Kind::Rotation, 0, [0.0, 0.0, 0.0, 1.0], None)
        .unwrap();
    let q = crate::angles_to_rotation([71.0, -23.0, 12.0]);
    doc.set_key(254, Kind::Rotation, 4, q, None).unwrap();
    doc.set_key(70, Kind::Translation, 4, [-1.0, 4.0, 2.0, 0.0], None)
        .unwrap();

    let encoded = encode(&doc, ClipKind::Base).unwrap();
    let clip = decode(&doc, ClipKind::Base, encoded);
    assert_eq!(
        clip.codecs(),
        [
            Some(Codec::UncompressedStatic),
            Some(Codec::QuantizedRotation8)
        ]
    );
    for frame in 0..doc.frames() {
        let baked = doc.sample(frame as f32);
        let sampled = clip.sample_frame(frame).unwrap();
        for (a, b) in baked.nodes.iter().zip(sampled.nodes) {
            assert_eq!(a.has_rotation, b.has_rotation);
            assert_eq!(a.has_translation, b.has_translation);
            assert_eq!(a.has_scale, b.has_scale);
            assert!(distance(Kind::Rotation, a.rotation, b.rotation) < 0.0001);
            assert_eq!(a.translation, b.translation);
            assert_eq!(a.scale, b.scale);
        }
    }
}

#[test]
fn constant_overlay_and_replacement_channels_use_the_animated_stream() {
    let mut doc = AnimDoc::new("static-delta".to_owned(), 4, nodes(2)).unwrap();
    doc.set_key(1, Kind::Translation, 2, [0.0, 0.0, -1.0, 0.0], None)
        .unwrap();
    for kind in [ClipKind::Overlay, ClipKind::Replacement] {
        let encoded = encode(&doc, kind).unwrap();
        let clip = decode(&doc, kind, encoded);
        assert_eq!(clip.codecs(), [None, Some(Codec::QuantizedRotation8)]);
        assert_eq!(clip.sample_frame(0).unwrap(), doc.sample(0.0));
        assert_eq!(clip.sample_frame(3).unwrap(), doc.sample(3.0));
    }
}
