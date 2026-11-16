//! Synthetic tests for the player and weapon models. Every byte is invented.
use super::*;
use std::f32::consts::FRAC_1_SQRT_2;
use std::fs;

fn close(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
}

/// Quaternion for `deg` degrees about game +z.
fn yaw(deg: f32) -> [f32; 4] {
    let h = deg.to_radians() / 2.0;
    [0.0, 0.0, h.sin(), h.cos()]
}

/// A root at the origin and an arm one unit along +x, plus a forearm one
/// more unit along it; markers on the arm.
fn skeleton() -> Vec<Bone> {
    let bone = |name: &str, parent, x| Bone {
        name: name.into(),
        parent,
        translation: [x, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
    };
    vec![
        bone("pelvis", None, 0.0),
        bone("arm", Some(0), 1.0),
        bone("forearm", Some(1), 1.0),
    ]
}

fn pose_of(graph: &[(&str, Option<[f32; 4]>)]) -> (Vec<String>, Pose) {
    let names = graph.iter().map(|(n, _)| n.to_string()).collect();
    let nodes = graph
        .iter()
        .map(|(_, r)| NodeTransform {
            rotation: r.unwrap_or([0.0, 0.0, 0.0, 1.0]),
            has_rotation: r.is_some(),
            ..NodeTransform::default()
        })
        .collect();
    (names, Pose { nodes })
}

#[test]
fn rest_pose_composes_up_the_tree() {
    let w = rest_world(&skeleton());
    assert!(close(&w[2][3][..3], &[2.0, 0.0, 0.0]));
    // Parents listed after their children still compose.
    let mut b = skeleton();
    b.reverse();
    b[0].parent = Some(1);
    b[1].parent = Some(2);
    b[2].parent = None;
    let w = rest_world(&b);
    assert!(close(&w[0][3][..3], &[2.0, 0.0, 0.0]));
}

#[test]
fn pose_baking_moves_vertices_as_the_frame_says() {
    let mut src = Source {
        bones: skeleton(),
        ..Source::default()
    };
    // A vertex at the forearm's tip (x = 3), one on the arm (x = 1.5), one
    // split half and half, one on the pelvis.
    src.pieces.push(Piece {
        material: 0,
        positions: vec![
            [3.0, 0.0, 0.0],
            [1.5, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
        ],
        normals: vec![[1.0, 0.0, 0.0]; 4],
        uvs: vec![[0.0; 2]; 4],
        nodes: vec![[2, 0, 0, 0], [1, 0, 0, 0], [1, 2, 0, 0], [0, 0, 0, 0]],
        weights: vec![
            [1.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.5, 0.5, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
        ],
        triangles: vec![[0, 1, 2], [1, 2, 3]],
    });
    // The graph lists nodes in its own order and leaves the pelvis alone;
    // the frame turns the arm 90 degrees about z.
    let (names, pose) = pose_of(&[
        ("forearm", None),
        ("arm", Some(yaw(90.0))),
        ("pelvis", None),
    ]);
    let posed = posed_world(&src.bones, &names, &pose);
    let prims = pose_mesh(&src, &posed);
    assert_eq!(prims.len(), 1);
    let p = &prims[0].positions;
    assert!(close(&p[0], &[1.0, 2.0, 0.0]), "{:?}", p[0]);
    assert!(close(&p[1], &[1.0, 0.5, 0.0]), "{:?}", p[1]);
    assert!(close(&p[2], &[1.0, 1.0, 0.0]), "{:?}", p[2]);
    assert!(close(&p[3], &[0.0, 0.0, 1.0]));
    // Normals turn with their node.
    assert!(close(&prims[0].normals[0], &[0.0, 1.0, 0.0]));
    assert_eq!(prims[0].indices, vec![0, 1, 2, 1, 2, 3]);
    // No change-colour map anywhere: no COLOR_0.
    assert!(prims[0].colours.is_empty());

    // The rest pose leaves everything where it was.
    let rest = pose_mesh(&src, &rest_world(&src.bones));
    assert_eq!(rest[0].positions, src.pieces[0].positions);
}

#[test]
fn a_frame_without_a_channel_keeps_the_rest_value() {
    let mut bones = skeleton();
    bones[1].rotation = yaw(90.0);
    // The arm's rotation is not in the frame (has_rotation false): rest wins.
    let (names, pose) = pose_of(&[("arm", None)]);
    let posed = posed_world(&bones, &names, &pose);
    assert!(
        close(&posed[2][3][..3], &[1.0, 1.0, 0.0]),
        "{:?}",
        posed[2][3]
    );
    // A translation the frame supplies replaces the rest one.
    let (names, mut pose) = pose_of(&[("forearm", None)]);
    pose.nodes[0].translation = [0.0, 0.0, 2.0];
    pose.nodes[0].has_translation = true;
    let posed = posed_world(&skeleton(), &names, &pose);
    assert!(close(&posed[2][3][..3], &[1.0, 0.0, 2.0]));
}

fn quad_piece(material: usize, z: f32) -> Piece {
    Piece {
        material,
        positions: vec![
            [0.0, 0.0, z],
            [1.0, 0.0, z],
            [1.0, 1.0, z + 2.0],
            [0.0, 1.0, z + 2.0],
        ],
        normals: vec![[0.0, -1.0, 0.0]; 4],
        uvs: vec![[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]],
        nodes: vec![[2, 0, 0, 0]; 4],
        weights: vec![[1.0, 0.0, 0.0, 0.0]; 4],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
    }
}

/// 2 x 2 change-colour map: left column primary (red), right column
/// secondary (green), top row full, bottom row half.
fn cc_map() -> Image {
    Image {
        width: 2,
        height: 2,
        rgba: vec![
            255, 0, 0, 255, 0, 255, 0, 255, //
            128, 0, 0, 255, 0, 128, 0, 255,
        ],
    }
}

#[test]
fn change_colour_weights_come_from_the_map_at_each_vertex() {
    // Texel centres sample exactly.
    assert!(close(
        &change_colour(&cc_map(), [0.25, 0.25]),
        &[1.0, 0.0, 0.0, 1.0]
    ));
    assert!(close(
        &change_colour(&cc_map(), [0.75, 0.25]),
        &[0.0, 1.0, 0.0, 1.0]
    ));
    let h = 128.0 / 255.0;
    assert!(close(
        &change_colour(&cc_map(), [0.75, 0.75]),
        &[0.0, h, 0.0, 1.0]
    ));
    // Halfway between two texels blends; coordinates wrap.
    assert!(close(
        &change_colour(&cc_map(), [0.5, 0.25]),
        &[0.5, 0.5, 0.0, 1.0]
    ));
    assert!(close(
        &change_colour(&cc_map(), [1.25, -0.75]),
        &[1.0, 0.0, 0.0, 1.0]
    ));

    // In the mesh: material 0 has the map, material 1 has none (zero weights).
    let src = Source {
        bones: skeleton(),
        pieces: vec![quad_piece(0, 0.0), quad_piece(1, 0.0)],
        materials: vec![
            Material {
                name: "armour".into(),
                texture: None,
            },
            Material {
                name: "visor".into(),
                texture: None,
            },
        ],
        change_colour: vec![Some(cc_map()), None],
        ..Source::default()
    };
    let prims = pose_mesh(&src, &rest_world(&src.bones));
    assert_eq!(prims.len(), 2);
    let c = &prims[0].colours;
    assert!(close(&c[0], &[1.0, 0.0, 0.0, 1.0]));
    assert!(close(&c[1], &[0.0, 1.0, 0.0, 1.0]));
    assert!(close(&c[3], &[h, 0.0, 0.0, 1.0]));
    assert!(
        prims[1]
            .colours
            .iter()
            .all(|c| close(c, &[0.0, 0.0, 0.0, 1.0]))
    );
}

fn biped_source() -> Source {
    let mut src = Source {
        bones: skeleton(),
        pieces: vec![quad_piece(0, -0.5)],
        materials: vec![Material {
            name: "armour".into(),
            texture: Some(Image {
                width: 2,
                height: 2,
                rgba: vec![200; 16],
            }),
        }],
        change_colour: vec![Some(cc_map())],
        ..Source::default()
    };
    src.markers.push(Marker {
        name: "right_hand".into(),
        node: 2,
        translation: [0.5, 0.0, 0.0],
        rotation: yaw(90.0),
    });
    src.markers.push(Marker {
        name: "head".into(),
        node: 0,
        translation: [0.0, 0.0, 1.5],
        rotation: [0.0, 0.0, 0.0, 1.0],
    });
    src
}

#[test]
fn biped_nodes_follow_the_pose_and_the_feet_go_to_the_origin() {
    let src = biped_source();
    let (names, pose) = pose_of(&[("arm", Some(yaw(90.0)))]);
    let model = biped(&src, &names, &pose).unwrap();
    let (lo, hi) = model.bounds().unwrap();
    assert!(lo[2].abs() < 1e-6, "feet at z = 0");
    assert!((model.extras["height"].as_f64().unwrap() as f32 - (hi[2] - lo[2])).abs() < 1e-6);
    let node = |n: &str| model.nodes.iter().find(|(k, _)| k == n).unwrap().1;
    // Forearm at (1, 1) after the pose, the marker half a unit further along
    // the turned arm, then everything lifted by the 0.5 the mesh hung below.
    let w = node("weapon");
    assert!(close(&w[3][..3], &[1.0, 1.5, 0.5]), "{:?}", w[3]);
    // Arm turn (90) plus the marker's own (90): the weapon points along -x.
    assert!(close(&w[0][..3], &[-1.0, 0.0, 0.0]), "{:?}", w[0]);
    assert!(close(&node("head")[3][..3], &[0.0, 0.0, 2.0]));

    // Without the marker the right-hand bone is used; without either it fails.
    let mut no_marker = src.clone();
    no_marker.markers.retain(|m| m.name != "right_hand");
    no_marker.bones[2].name = "r_hand".into();
    let m = biped(&no_marker, &names, &pose).unwrap();
    let w = m.nodes.iter().find(|(k, _)| k == "weapon").unwrap().1;
    assert!(close(&w[3][..3], &[1.0, 1.0, 0.5]), "{:?}", w[3]);
    no_marker.bones[2].name = "something".into();
    assert!(biped(&no_marker, &names, &pose).is_err());
    assert!(
        biped(
            &Source {
                pieces: vec![],
                ..src
            },
            &names,
            &pose
        )
        .is_err()
    );
}

#[test]
fn weapon_muzzle_comes_from_its_marker_or_the_front() {
    let mut src = biped_source();
    src.markers.push(Marker {
        name: "primary_trigger".into(),
        node: 1,
        translation: [0.0, 0.0, 0.25],
        rotation: [0.0, 0.0, 0.0, 1.0],
    });
    let w = weapon(&src).unwrap();
    assert_eq!(w.mesh_name, "weapon");
    assert!(w.primitives.iter().all(|p| p.colours.is_empty()));
    let muzzle = w.nodes.iter().find(|(k, _)| k == "muzzle").unwrap().1;
    assert!(close(&muzzle[3][..3], &[1.0, 0.0, 0.25]));
    src.markers.clear();
    let w = weapon(&src).unwrap();
    let muzzle = w.nodes.iter().find(|(k, _)| k == "muzzle").unwrap().1;
    assert!(close(&muzzle[3][..3], &[1.0, 0.5, 0.5]), "{:?}", muzzle[3]);
}

/// The JSON chunk and the binary chunk of a glb.
fn parse_glb(glb: &[u8]) -> (Value, Vec<u8>) {
    let u32_at = |o: usize| u32::from_le_bytes(glb[o..o + 4].try_into().unwrap()) as usize;
    assert_eq!(&glb[..4], b"glTF");
    assert_eq!(u32_at(8), glb.len());
    let n = u32_at(12);
    let doc: Value = serde_json::from_slice(&glb[20..20 + n]).unwrap();
    let rest = 20 + n;
    if rest == glb.len() {
        return (doc, Vec::new());
    }
    (doc, glb[rest + 8..rest + 8 + u32_at(rest)].to_vec())
}

fn floats(doc: &Value, bin: &[u8], accessor: &Value) -> Vec<f32> {
    let a = &doc["accessors"][accessor.as_u64().unwrap() as usize];
    let v = &doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
    let off = v["byteOffset"].as_u64().unwrap() as usize;
    let len = v["byteLength"].as_u64().unwrap() as usize;
    bin[off..off + len]
        .chunks(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

#[test]
fn biped_glb_reads_back_with_nodes_colours_and_texture() {
    let src = biped_source();
    let (names, pose) = pose_of(&[("arm", Some(yaw(90.0)))]);
    let model = biped(&src, &names, &pose).unwrap();
    let glb = write_glb(&model);

    let parsed = gltf::Gltf::from_slice(&glb).expect("valid glTF");
    let node_names: Vec<_> = parsed
        .nodes()
        .filter_map(|n| n.name().map(str::to_owned))
        .collect();
    for n in ["root", "body", "weapon", "head"] {
        assert!(node_names.iter().any(|k| k == n), "{n} in {node_names:?}");
    }
    let prim = parsed.meshes().next().unwrap().primitives().next().unwrap();
    let semantics: Vec<_> = prim.attributes().map(|(s, _)| s).collect();
    assert!(semantics.contains(&gltf::Semantic::Colors(0)));
    assert!(semantics.contains(&gltf::Semantic::TexCoords(0)));
    let material = prim.material();
    assert!(
        material
            .pbr_metallic_roughness()
            .base_color_texture()
            .is_some()
    );
    assert_eq!(parsed.images().count(), 1);
    // A second material with the same texture shares the image.
    let mut two = model.clone();
    two.materials.push(two.materials[0].clone());
    let mut p2 = two.primitives[0].clone();
    p2.material = 1;
    two.primitives.push(p2);
    let shared = gltf::Gltf::from_slice(&write_glb(&two)).unwrap();
    assert_eq!(shared.images().count(), 1);
    assert_eq!(shared.materials().count(), 2);

    let (doc, bin) = parse_glb(&glb);
    let extras = &doc["asset"]["extras"];
    assert_eq!(extras["models_version"], MODELS_VERSION);
    assert!(extras["height"].as_f64().unwrap() > 0.0);
    // Positions are converted: game (x, y, z) -> (x, z, -y), feet at y = 0.
    let attrs = &doc["meshes"][0]["primitives"][0]["attributes"];
    let pos = floats(&doc, &bin, &attrs["POSITION"]);
    let game = &model.primitives[0].positions;
    for (i, g) in game.iter().enumerate() {
        assert!(close(&pos[i * 3..i * 3 + 3], &to_gltf(*g)));
    }
    let min_y = pos.chunks(3).map(|p| p[1]).fold(f32::INFINITY, f32::min);
    assert!(min_y.abs() < 1e-6);
    let colours = floats(&doc, &bin, &attrs["COLOR_0"]);
    assert!(close(&colours[..4], &[1.0, 0.0, 0.0, 1.0]));
    // The weapon node: game (1, 1.5, 0.5) is glTF (1, 0.5, -1.5), and its
    // 180-degree yaw is a half turn about glTF +y.
    let weapon = doc["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "weapon")
        .unwrap();
    let t: Vec<f32> = serde_json::from_value(weapon["translation"].clone()).unwrap();
    assert!(close(&t, &[1.0, 0.5, -1.5]), "{t:?}");
    let r: Vec<f32> = serde_json::from_value(weapon["rotation"].clone()).unwrap();
    assert!(
        close(&r, &[0.0, 1.0, 0.0, 0.0]) || close(&r, &[0.0, -1.0, 0.0, 0.0]),
        "{r:?}"
    );
}

#[test]
fn a_quarter_yaw_is_a_quarter_turn_about_gltf_up() {
    let mut m = trs([1.0, 2.0, 3.0], yaw(90.0), 1.0);
    let (t, r) = gltf_trs(&m);
    assert!(close(&t, &[1.0, 3.0, -2.0]));
    assert!(
        close(&r, &[0.0, FRAC_1_SQRT_2, 0.0, FRAC_1_SQRT_2]),
        "{r:?}"
    );
    m = IDENTITY;
    assert!(close(&gltf_trs(&m).1, &[0.0, 0.0, 0.0, 1.0]));
}

#[test]
fn grey_without_a_texture_and_no_colour_on_weapons() {
    let mut src = biped_source();
    src.materials[0].texture = None;
    let glb = write_glb(&weapon(&src).unwrap());
    gltf::Gltf::from_slice(&glb).expect("valid glTF");
    let (doc, _) = parse_glb(&glb);
    assert_eq!(
        doc["materials"][0]["pbrMetallicRoughness"]["baseColorFactor"],
        json!(GREY)
    );
    assert!(doc.get("images").is_none());
    assert!(
        doc["meshes"][0]["primitives"][0]["attributes"]
            .get("COLOR_0")
            .is_none()
    );
    assert!(
        doc["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["name"] == "muzzle")
    );
}

#[test]
fn big_textures_are_halved_to_fit() {
    let img = Image {
        width: 8,
        height: 4,
        rgba: (0..8 * 4)
            .flat_map(|i| [(i % 8 * 30) as u8, 0, 0, 255])
            .collect(),
    };
    let small = img.fit(2);
    assert_eq!((small.width, small.height), (2, 1));
    // Each output texel averages a 4 x 4 block: red 0,30,60,90 -> 45.
    assert_eq!(small.rgba[0], 45);
}

#[test]
fn manifest_models_object_matches_the_contract() {
    assert_eq!(ModelsEntry::new(&[], &[]), None);
    let m = ModelsEntry::new(
        &["elite".into(), "spartan".into()],
        &["battle_rifle".into()],
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&m).unwrap(),
        json!({
            "version": 1,
            "bipeds": {
                "elite": {"glb": "models/biped/elite.glb"},
                "spartan": {"glb": "models/biped/spartan.glb"}
            },
            "weapons": {"battle_rifle": {"glb": "models/weapon/battle_rifle.glb"}}
        })
    );
}

#[test]
fn write_models_skips_failures_and_lists_what_it_wrote() {
    let dir = std::env::temp_dir().join(format!("h2-models-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = biped_source();
    let (names, pose) = pose_of(&[]);
    let built = vec![
        (
            Kind::Biped,
            "spartan".to_string(),
            biped(&src, &names, &pose),
        ),
        (
            Kind::Biped,
            "elite".to_string(),
            Err(anyhow::anyhow!("broken")),
        ),
        (Kind::Weapon, "smg".to_string(), weapon(&src)),
    ];
    let w = write_models(built, &dir);
    let entry = w.entry.unwrap();
    assert_eq!(entry.bipeds.keys().collect::<Vec<_>>(), ["spartan"]);
    assert_eq!(entry.weapons.keys().collect::<Vec<_>>(), ["smg"]);
    assert!(w.skipped["models/biped/elite"].contains("broken"));
    assert!(dir.join("models/biped/spartan.glb").is_file());
    assert!(dir.join("models/weapon/smg.glb").is_file());
    assert!(!dir.join("models/biped/elite.glb").exists());
    assert_eq!(w.stats.len(), 2);
    std::fs::remove_dir_all(&dir).unwrap();

    // Nothing built: no models object at all.
    let none = write_models(vec![], &std::env::temp_dir());
    assert!(none.entry.is_none());
}

// ---- the owner's disc (ignored) --------------------------------------------

/// One glb read back for the check pictures: triangles in glTF space with
/// per-vertex normal, uv, COLOR_0 and the material's texture.
/// Position, normal, uv, COLOR_0.
type Vertex = ([f32; 3], [f32; 3], [f32; 2], [f32; 4]);
/// A drawn glb and where it is placed (translation, rotation), if anywhere.
type Placed<'a> = (&'a Drawn, Option<([f32; 3], [f32; 4])>);

struct Drawn {
    tris: Vec<[Vertex; 3]>,
    tex: Vec<Option<Image>>,
    tri_material: Vec<Option<usize>>,
    nodes: Vec<(String, [f32; 3], [f32; 4])>,
}

fn decode_png(bytes: &[u8]) -> Option<Image> {
    let mut d = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .ok()?;
    let mut buf = vec![0; d.output_buffer_size()?];
    let info = d.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba {
        return None;
    }
    buf.truncate(info.buffer_size());
    Some(Image {
        width: info.width,
        height: info.height,
        rgba: buf,
    })
}

fn read_drawn(glb: &[u8]) -> Drawn {
    let (doc, bin) = parse_glb(glb);
    let view_bytes = |v: u64| {
        let v = &doc["bufferViews"][v as usize];
        let o = v["byteOffset"].as_u64().unwrap() as usize;
        &bin[o..o + v["byteLength"].as_u64().unwrap() as usize]
    };
    let images: Vec<Option<Image>> = doc["images"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|i| decode_png(view_bytes(i["bufferView"].as_u64().unwrap())))
                .collect()
        })
        .unwrap_or_default();
    let tex = doc["materials"]
        .as_array()
        .map(|ms| {
            ms.iter()
                .map(|m| {
                    let t = m["pbrMetallicRoughness"]["baseColorTexture"]["index"].as_u64()?;
                    let src = doc["textures"][t as usize]["source"].as_u64()?;
                    images.get(src as usize)?.clone()
                })
                .collect()
        })
        .unwrap_or_default();
    let mut d = Drawn {
        tris: vec![],
        tex,
        tri_material: vec![],
        nodes: vec![],
    };
    for p in doc["meshes"][0]["primitives"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let a = &p["attributes"];
        let get = |k: &str| (!a[k].is_null()).then(|| floats(&doc, &bin, &a[k]));
        let pos = get("POSITION").unwrap();
        let nrm = get("NORMAL").unwrap();
        let uv = get("TEXCOORD_0").unwrap_or_default();
        let col = get("COLOR_0").unwrap_or_default();
        let ia = &doc["accessors"][p["indices"].as_u64().unwrap() as usize];
        let v = &doc["bufferViews"][ia["bufferView"].as_u64().unwrap() as usize];
        let o = v["byteOffset"].as_u64().unwrap() as usize;
        let idx: Vec<usize> = bin[o..o + v["byteLength"].as_u64().unwrap() as usize]
            .chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()) as usize)
            .collect();
        let vert = |i: usize| {
            (
                [pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]],
                [nrm[i * 3], nrm[i * 3 + 1], nrm[i * 3 + 2]],
                if uv.is_empty() {
                    [0.0; 2]
                } else {
                    [uv[i * 2], uv[i * 2 + 1]]
                },
                if col.is_empty() {
                    [0.0, 0.0, 0.0, 1.0]
                } else {
                    [col[i * 4], col[i * 4 + 1], col[i * 4 + 2], col[i * 4 + 3]]
                },
            )
        };
        for t in idx.chunks(3) {
            d.tris.push([vert(t[0]), vert(t[1]), vert(t[2])]);
            d.tri_material
                .push(p["material"].as_u64().map(|m| m as usize));
        }
    }
    for n in doc["nodes"].as_array().unwrap() {
        if let (Some(t), Some(r)) = (n.get("translation"), n.get("rotation")) {
            d.nodes.push((
                n["name"].as_str().unwrap().to_owned(),
                serde_json::from_value(t.clone()).unwrap(),
                serde_json::from_value(r.clone()).unwrap(),
            ));
        }
    }
    d
}

fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let u = [q[0], q[1], q[2]];
    let t = cross(u, v).map(|c| 2.0 * c);
    let c = cross(u, t);
    std::array::from_fn(|k| v[k] + q[3] * t[k] + c[k])
}

#[derive(Clone, Copy, PartialEq)]
enum Paint {
    Texture,
    ChangeColour,
}

/// Orthographic picture of glTF-space triangles. `view` 0: from +x (the
/// model's front), 1: from +z (its right side), 2: three-quarter.
fn picture(
    items: &[Placed<'_>],
    view: usize,
    paint: Paint,
    w: usize,
    h: usize,
    height: f32,
) -> Vec<u8> {
    let az: f32 = [0.0f32, 90.0, 40.0][view].to_radians();
    // Camera direction from the origin (glTF): (cos az, 0, sin az).
    let fwd = [-az.cos(), 0.0, -az.sin()];
    let right = cross(fwd, [0.0, 1.0, 0.0]);
    let scale = h as f32 * 0.85 / height;
    let (cx, cy) = (w as f32 / 2.0, h as f32 * 0.92);
    let mut depth = vec![f32::INFINITY; w * h];
    let mut img = vec![0u8; w * h * 4];
    for px in img.chunks_mut(4) {
        px.copy_from_slice(&[40, 44, 52, 255]);
    }
    let light = normalize([0.4, 0.8, 0.5]);
    for (d, place) in items {
        for (ti, t) in d.tris.iter().enumerate() {
            let tex = d.tri_material[ti]
                .and_then(|m| d.tex.get(m))
                .and_then(Option::as_ref);
            let world = |p: [f32; 3]| match place {
                Some((tr, q)) => {
                    let r = quat_rotate(*q, p);
                    std::array::from_fn(|k| r[k] + tr[k])
                }
                None => p,
            };
            let v: Vec<[f32; 3]> = t
                .iter()
                .map(|(p, ..)| {
                    let p = world(*p);
                    [cx + dot(p, right) * scale, cy - p[1] * scale, dot(p, fwd)]
                })
                .collect();
            let area = (v[1][0] - v[0][0]) * (v[2][1] - v[0][1])
                - (v[2][0] - v[0][0]) * (v[1][1] - v[0][1]);
            if area.abs() < 1e-9 {
                continue;
            }
            let minx = v
                .iter()
                .map(|p| p[0])
                .fold(f32::INFINITY, f32::min)
                .max(0.0) as usize;
            let maxx = (v
                .iter()
                .map(|p| p[0])
                .fold(f32::NEG_INFINITY, f32::max)
                .min(w as f32 - 1.0))
            .max(0.0) as usize;
            let miny = v
                .iter()
                .map(|p| p[1])
                .fold(f32::INFINITY, f32::min)
                .max(0.0) as usize;
            let maxy = (v
                .iter()
                .map(|p| p[1])
                .fold(f32::NEG_INFINITY, f32::max)
                .min(h as f32 - 1.0))
            .max(0.0) as usize;
            for y in miny..=maxy {
                for x in minx..=maxx {
                    let (sx, sy) = (x as f32 + 0.5, y as f32 + 0.5);
                    let e = |a: &[f32; 3], b: &[f32; 3]| {
                        (b[0] - a[0]) * (sy - a[1]) - (b[1] - a[1]) * (sx - a[0])
                    };
                    let b0 = e(&v[1], &v[2]) / area;
                    let b1 = e(&v[2], &v[0]) / area;
                    let b2 = 1.0 - b0 - b1;
                    if b0 < 0.0 || b1 < 0.0 || b2 < 0.0 {
                        continue;
                    }
                    let z = b0 * v[0][2] + b1 * v[1][2] + b2 * v[2][2];
                    let i = y * w + x;
                    if z >= depth[i] {
                        continue;
                    }
                    depth[i] = z;
                    let mix = |f: &dyn Fn(usize) -> f32| b0 * f(0) + b1 * f(1) + b2 * f(2);
                    let mut n: [f32; 3] = std::array::from_fn(|k| mix(&|j| t[j].1[k]));
                    if let Some((_, q)) = place {
                        n = quat_rotate(*q, n);
                    }
                    let n = normalize(n);
                    let lit = 0.35 + 0.65 * dot(n, light).abs();
                    let base: [f32; 3] = if place.is_some() {
                        [0.35, 0.37, 0.33]
                    } else if paint == Paint::Texture {
                        let uv = [mix(&|j| t[j].2[0]), mix(&|j| t[j].2[1])];
                        tex.map_or([0.6; 3], |im| {
                            let s = im.sample(uv);
                            [s[0], s[1], s[2]]
                        })
                    } else {
                        let (r, g) = (mix(&|j| t[j].3[0]), mix(&|j| t[j].3[1]));
                        let grey = 0.55 * (1.0 - r - g).max(0.0);
                        [
                            grey + 0.95 * r + 0.15 * g,
                            grey + 0.15 * r + 0.45 * g,
                            grey + 0.15 * r + 0.95 * g,
                        ]
                    };
                    for k in 0..3 {
                        img[i * 4 + k] = ((base[k] * lit).clamp(0.0, 1.0) * 255.0) as u8;
                    }
                }
            }
        }
    }
    img
}

/// Writes every model from the owner's disc, a report and check pictures to
/// `$RUSH_KEEP/players-models` (outside the repository):
/// `cargo test --release -p cli --bin h2 models_from_the_disc -- --ignored --nocapture`
#[test]
#[ignore]
fn models_from_the_disc() {
    use crate::live_art::Disc;
    let disc_path = std::env::var("H2_DISC").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../../disc/halo2.iso").into()
    });
    let keep = std::env::var("RUSH_KEEP")
        .unwrap_or_else(|_| "/Users/afr/.config/rush/keep/h2recomp-b5afe71a/bed6c8dc".into());
    let check = Path::new(&keep).join("players-models");
    let out = check.join("art");
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).unwrap();
    let disc = Disc::scan(Path::new(&disc_path)).unwrap();
    let started = std::time::Instant::now();
    let (w, infos) = extract(&disc, &out);
    let mut report = format!("models in {:?}\n", started.elapsed());
    for i in &infos {
        report += &format!(
            "{}: {} on {} posed by {}\n",
            i.species, i.tag, i.map, i.animation
        );
    }
    for s in &w.stats {
        report += &format!(
            "{:<36} {:>6} triangles {:>5} KiB\n",
            s.key,
            s.triangles,
            s.glb_bytes / 1024
        );
    }
    for (k, why) in &w.skipped {
        report += &format!("skipped {k}: {why}\n");
    }
    print!("{report}");
    fs::write(check.join("report.txt"), &report).unwrap();
    let entry = w.entry.expect("models were extracted");
    fs::write(
        check.join("models.json"),
        serde_json::to_string_pretty(&entry).unwrap(),
    )
    .unwrap();
    assert!(entry.bipeds.contains_key("spartan") && entry.bipeds.contains_key("elite"));
    assert!(entry.weapons.contains_key("battle_rifle"));

    // The rest pose agrees with the inverse rest frames the render model
    // stores (node +0x2c: three axes and a position): the quaternion
    // convention used for the pose is the engine's.
    let m = disc.maps_of(map::CacheType::Multiplayer).next().unwrap();
    let (mut map, mut files) = disc.open(m).unwrap();
    let id = map
        .tags()
        .iter()
        .find(|t| t.group == BIPD && t.location.is_some())
        .unwrap()
        .id;
    let (mode, _) = object_model(&mut map, id).unwrap();
    let geo = RenderModelGeometry::read(&mut map, mode, &mut Res(&mut files)).unwrap();
    let src = read_source(&mut map, &mut files, mode, MAX_BIPED_TRIANGLES).unwrap();
    let rest = rest_world(&src.bones);
    let mut worst = 0.0f32;
    for (n, r) in geo.nodes.iter().zip(&rest) {
        let u = n.wire.unknown_028;
        let inv: Mat4 = [
            [u[1], u[2], u[3], 0.0],
            [u[4], u[5], u[6], 0.0],
            [u[7], u[8], u[9], 0.0],
            [u[10], u[11], u[12], 1.0],
        ];
        let id = mat_mul(&inv, r);
        for c in 0..4 {
            for k in 0..4 {
                worst = worst.max((id[c][k] - IDENTITY[c][k]).abs());
            }
        }
    }
    println!("rest frames vs stored inverses: worst {worst}");
    assert!(
        worst < 1e-2,
        "rest pose disagrees with the stored inverse frames: {worst}"
    );

    // Check pictures: each biped from the front, the side and three-quarter,
    // textured, then tinted by its change-colour weights (red primary, blue
    // secondary), holding the battle rifle at its `weapon` node.
    let rifle = read_drawn(&fs::read(out.join(&entry.weapons["battle_rifle"].glb)).unwrap());
    let (pw, ph) = (360usize, 520usize);
    for (species, r) in &entry.bipeds {
        let glb = fs::read(out.join(&r.glb)).unwrap();
        let height = parse_glb(&glb).0["asset"]["extras"]["height"]
            .as_f64()
            .unwrap() as f32;
        println!("{species} height {height}");
        let body = read_drawn(&glb);
        let (_, t, q) = body
            .nodes
            .iter()
            .find(|(n, ..)| n == "weapon")
            .unwrap()
            .clone();
        let items = [(&body, None), (&rifle, Some((t, q)))];
        let panels: Vec<Vec<u8>> = [
            (0, Paint::Texture),
            (1, Paint::Texture),
            (2, Paint::Texture),
            (0, Paint::ChangeColour),
            (2, Paint::ChangeColour),
        ]
        .iter()
        .map(|&(v, p)| picture(&items, v, p, pw, ph, height))
        .collect();
        let total_w = pw * panels.len();
        let mut sheet = vec![0u8; total_w * ph * 4];
        for (k, p) in panels.iter().enumerate() {
            for y in 0..ph {
                let dst = (y * total_w + k * pw) * 4;
                sheet[dst..dst + pw * 4].copy_from_slice(&p[y * pw * 4..(y + 1) * pw * 4]);
            }
        }
        let mut png = Vec::new();
        assets::png::write_rgba(&mut png, total_w as u32, ph as u32, &sheet).unwrap();
        let path = check.join(format!("check-{species}.png"));
        fs::write(&path, png).unwrap();
        println!("wrote {}", path.display());
    }
}
