use super::*;

fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut json = json.as_bytes().to_vec();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    let mut bin = bin.to_vec();
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = b"glTF".to_vec();
    out.extend(2u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((json.len() as u32).to_le_bytes());
    out.extend(0x4E4F_534Au32.to_le_bytes());
    out.extend(json);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(0x004E_4942u32.to_le_bytes());
    out.extend(bin);
    out
}

/// Two nodes, "hips" over "arm", and a rotation of the arm from nothing
/// to a quarter about the file's Y axis in three frames.
fn quarter_turn(nodes: &str) -> Vec<u8> {
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let mut bin = floats(&[0.0, 0.1]);
    bin.extend(floats(&[0.0, 0.0, 0.0, 1.0, 0.0, half, 0.0, half]));
    let json = format!(
        r#"{{"asset":{{"version":"2.0"}},"nodes":{nodes},
        "buffers":[{{"byteLength":40}}],
        "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":8}},{{"buffer":0,"byteOffset":8,"byteLength":32}}],
        "accessors":[{{"bufferView":0,"componentType":5126,"count":2,"type":"SCALAR"}},
                     {{"bufferView":1,"componentType":5126,"count":2,"type":"VEC4"}}],
        "animations":[{{"name":"wave","samplers":[{{"input":0,"output":1}}],
            "channels":[{{"sampler":0,"target":{{"node":1,"path":"rotation"}}}}]}},{{"samplers":[],"channels":[]}}]}}"#
    );
    glb(&json, &bin)
}

const CHAIN: &str = r#"[{"name":"hips","children":[1]},{"name":"arm","translation":[1,0,0]}]"#;

#[test]
fn a_clip_keeps_its_times_as_frames_and_its_nodes_in_the_engines_axes() {
    let file = quarter_turn(CHAIN);
    assert_eq!(clip_names(&file).unwrap(), ["wave", "animation 1"]);
    let imported = read_gltf_bytes(&file, None, 0).unwrap();
    let clip = &imported.clip;
    assert_eq!((clip.name(), clip.frames()), ("wave", 4));
    assert_eq!(clip.nodes()[1].name.as_deref(), Some("arm"));
    assert_eq!(clip.nodes()[1].parent, 0);
    assert_eq!(imported.rest[1].translation, [0.0, 1.0, 0.0]);
    let key = clip.key(1, Kind::Rotation, 3).expect("a key at frame 3");
    let half = std::f32::consts::FRAC_1_SQRT_2;
    for (got, want) in key.value.iter().zip([0.0, 0.0, half, half]) {
        assert!((got - want).abs() < 1e-6, "{:?}", key.value);
    }
    assert!(!clip.can_undo());
}

#[test]
fn what_cannot_be_held_is_named() {
    let matrix = quarter_turn(
        r#"[{"name":"hips","children":[1]},{"matrix":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]}]"#,
    );
    assert!(read_gltf_bytes(&matrix, None, 0).unwrap_err().contains("matrix"));
    assert!(read_gltf_bytes(&quarter_turn(CHAIN), None, 1).unwrap_err().contains("no channel"));
    assert!(read_gltf_bytes(&quarter_turn(CHAIN), None, 2).is_err());
    let stretched = quarter_turn(r#"[{"children":[1]},{"scale":[1,2,1]}]"#);
    let imported = read_gltf_bytes(&stretched, None, 0).unwrap();
    assert!(imported.notes.iter().any(|n| n.contains("non-uniform")));
}

#[test]
fn an_external_buffer_must_be_a_plain_file_beside_the_gltf() {
    let json = br#"{"asset":{"version":"2.0"},"nodes":[{}],
        "buffers":[{"uri":"../times.bin","byteLength":8}],
        "bufferViews":[{"buffer":0,"byteLength":8}],
        "accessors":[{"bufferView":0,"componentType":5126,"count":2,"type":"SCALAR"}],
        "animations":[{"samplers":[{"input":0,"output":0}],
            "channels":[{"sampler":0,"target":{"node":0,"path":"scale"}}]}]}"#;
    let err = read_gltf_bytes(json, Some(Path::new("/invented")), 0).unwrap_err();
    assert!(err.contains("plain relative file"), "{err}");
}

#[test]
fn base64_decodes() {
    assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
    assert!(base64("a*").is_none());
}
