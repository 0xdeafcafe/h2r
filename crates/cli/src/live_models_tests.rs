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
        // Each face four times over: enough triangles to count as a vehicle.
        triangles: [[0, 1, 2], [0, 2, 3]].repeat(4),
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
                ..Material::default()
            },
            Material {
                name: "visor".into(),
                ..Material::default()
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
            ..Material::default()
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
    let model = biped(&src, &names, &pose, &[]).unwrap();
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
    let m = biped(&no_marker, &names, &pose, &[]).unwrap();
    let w = m.nodes.iter().find(|(k, _)| k == "weapon").unwrap().1;
    assert!(close(&w[3][..3], &[1.0, 1.0, 0.5]), "{:?}", w[3]);
    // A creature with no hand at all holds its weapon at the root; with no
    // head marker or node the head is the top of the mesh.
    no_marker.bones[2].name = "something".into();
    no_marker.markers.clear();
    let m = biped(&no_marker, &names, &pose, &[]).unwrap();
    let node = |n: &str| m.nodes.iter().find(|(k, _)| k == n).unwrap().1;
    assert!(
        close(&node("weapon")[3][..3], &[0.0, 0.0, 0.5]),
        "{:?}",
        node("weapon")[3]
    );
    let (lo, hi) = m.bounds().unwrap();
    assert!(close(
        &node("head")[3][..3],
        &[(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, hi[2]]
    ));
    // A bone with `hand` in its name is the next best.
    no_marker.bones[1].name = "l_hand".into();
    let m = biped(&no_marker, &names, &pose, &[]).unwrap();
    let w = m.nodes.iter().find(|(k, _)| k == "weapon").unwrap().1;
    assert!(close(&w[3][..3], &[1.0, 0.0, 0.5]), "{:?}", w[3]);
    assert!(
        biped(
            &Source {
                pieces: vec![],
                ..src
            },
            &names,
            &pose,
            &[]
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
    let model = biped(&src, &names, &pose, &[]).unwrap();
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
    // The weapon node rides on the forearm (its marker's node); in the
    // scene it is where it was: game (1, 1.5, 0.5) is glTF (1, 0.5, -1.5),
    // and its 180-degree yaw is a half turn about glTF +y.
    let nodes = doc["nodes"].as_array().unwrap();
    let at = |name: &str| nodes.iter().position(|n| n["name"] == name).unwrap();
    let forearm = &nodes[at("joint_forearm")];
    assert!(
        forearm["children"]
            .as_array()
            .unwrap()
            .contains(&json!(at("weapon")))
    );
    let drawn = read_drawn(&glb);
    let (_, t, r) = drawn.nodes.iter().find(|n| n.0 == "weapon").unwrap();
    assert!(close(t, &[1.0, 0.5, -1.5]), "{t:?}");
    assert!(
        close(r, &[0.0, 1.0, 0.0, 0.0]) || close(r, &[0.0, -1.0, 0.0, 0.0]),
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

/// A flat picture: every pixel `rgba`.
fn flat(w: u32, h: u32, rgba: [u8; 4]) -> Image {
    Image {
        width: w,
        height: h,
        rgba: rgba.repeat((w * h) as usize),
    }
}

#[test]
fn bitmap_roles_come_from_their_names() {
    let role = |n: &str| bitmap_role(n);
    assert_eq!(
        role(r"objects\characters\x\bitmaps\x_cc"),
        BitmapRole::ChangeColour
    );
    assert_eq!(
        role(r"objects\v\bitmaps\plate_change_color"),
        BitmapRole::ChangeColour
    );
    // Spelt the other way round on some armour.
    assert_eq!(
        role(r"objects\c\bitmaps\arms_color_change"),
        BitmapRole::ChangeColour
    );
    assert_eq!(
        role(r"objects\c\bitmaps\arms_self_illum"),
        BitmapRole::SelfIllum
    );
    assert_eq!(
        role(r"objects\c\bitmaps\torso_illumination"),
        BitmapRole::SelfIllum
    );
    assert_eq!(role(r"objects\c\bitmaps\torso"), BitmapRole::Diffuse);
    assert_eq!(role(r"objects\c\bitmaps\torso_bump"), BitmapRole::Other);
    assert_eq!(
        role(r"objects\bitmaps\detail_maps\metal"),
        BitmapRole::Other
    );
    assert_eq!(
        role(r"objects\bitmaps\reflection_maps\armor"),
        BitmapRole::Other
    );
    // Engine defaults are never the surface, whatever they are called.
    assert_eq!(
        role(r"shaders\default_bitmaps\bitmaps\default_additive"),
        BitmapRole::Other
    );
    assert_eq!(
        role(r"shaders\default_bitmaps\bitmaps\color_white"),
        BitmapRole::Other
    );
}

#[test]
fn a_bump_env_change_colour_shader_gets_its_maps_and_shine() {
    // Base alpha is the specular mask: full on the left column, empty on the right.
    let mut base = flat(2, 2, [200, 100, 50, 255]);
    base.rgba[4..8].copy_from_slice(&[200, 100, 50, 0]);
    base.rgba[12..16].copy_from_slice(&[200, 100, 50, 0]);
    let cc = flat(2, 2, [255, 0, 0, 0]);
    let glow = flat(2, 2, [0, 80, 255, 255]);
    let (m, vertex_cc) = material_look(
        "armour",
        r"shaders\shader_templates\opaque\tex_bump_env_illum_two_change_color",
        vec![
            (
                r"o\bitmaps\armour_bump".into(),
                Some(flat(2, 2, [1, 2, 3, 4])),
            ),
            (r"o\bitmaps\armour".into(), Some(base)),
            (r"o\bitmaps\armour_color_change".into(), Some(cc.clone())),
            (r"o\bitmaps\armour_self_illum".into(), Some(glow.clone())),
            (r"objects\bitmaps\reflection_maps\shell".into(), None),
        ],
    );
    assert_eq!(m.template, "tex_bump_env_illum_two_change_color");
    assert_eq!(m.blend, Blend::Opaque);
    assert_eq!(vertex_cc.as_ref(), Some(&cc));
    assert_eq!(m.change_colour.as_ref(), Some(&cc));
    assert_eq!(m.emissive.as_ref(), Some(&glow));
    // The base map goes opaque; its alpha became the specular texture.
    let t = m.texture.as_ref().unwrap();
    assert!(t.rgba.chunks(4).all(|p| p[3] == 255));
    assert_eq!(&t.rgba[..3], &[200, 100, 50]);
    let s = m.specular.as_ref().unwrap();
    // Masked: smooth and metallic (an env shader's 0.6). Unmasked: matte, not metal.
    assert_eq!(&s.rgba[0..3], &[255, 77, 153]);
    assert_eq!(&s.rgba[4..7], &[255, 217, 0]);
}

#[test]
fn transparent_alpha_tested_and_lit_looks() {
    let base = || Some(flat(2, 2, [10, 20, 30, 128]));
    let look = |t: &str| {
        material_look(
            "m",
            &format!(r"shaders\shader_templates\{t}"),
            vec![(r"o\bitmaps\skin".into(), base())],
        )
        .0
    };
    // Glass: no picture, blended.
    let glass = look(r"transparent\two_alpha_env_multichannel");
    assert_eq!((glass.blend, glass.texture.is_none()), (Blend::Alpha, true));
    assert!(glass.specular.is_none());
    // Additive: only the glow is left.
    let light = look(r"transparent\two_add_env_illum");
    assert_eq!(light.blend, Blend::Additive);
    assert!(light.texture.is_none());
    assert_eq!(light.emissive, base());
    // Cut-outs keep their alpha.
    let torn = look(r"opaque\tex_bump_env_alpha_test");
    assert_eq!(torn.blend, Blend::Mask);
    assert_eq!(torn.texture.as_ref().unwrap().rgba[3], 128);
    assert!(
        torn.specular.is_none(),
        "a cut-out's alpha is its shape, not shine"
    );
    // A decal overlay blends with its alpha.
    assert_eq!(look(r"opaque\overlay").blend, Blend::Alpha);
    // A lit display glows with its own picture.
    assert_eq!(
        look(r"opaque\tex_bump_meter_illum")
            .emissive
            .map(|e| e.rgba[0]),
        Some(10)
    );
    // A plain bump shader: opaque, no shine, no glow.
    let plain = look(r"opaque\tex_bump");
    assert_eq!(plain.blend, Blend::Opaque);
    assert!(plain.specular.is_none() && plain.emissive.is_none() && plain.change_colour.is_none());
}

#[test]
fn the_glb_carries_each_look() {
    let mut src = biped_source();
    let (armour, _) = material_look(
        "armour",
        r"shaders\shader_templates\opaque\tex_bump_env_illum_four_change_color",
        vec![
            (
                r"o\bitmaps\armour".into(),
                Some(flat(2, 2, [200, 200, 200, 255])),
            ),
            (
                r"o\bitmaps\armour_cc".into(),
                Some(flat(2, 2, [255, 0, 0, 255])),
            ),
            (
                r"o\bitmaps\armour_illum".into(),
                Some(flat(2, 2, [0, 0, 255, 255])),
            ),
        ],
    );
    let (light, _) = material_look(
        "light",
        r"shaders\shader_templates\transparent\one_add_illum",
        vec![(
            r"o\bitmaps\armour".into(),
            Some(flat(2, 2, [200, 200, 200, 255])),
        )],
    );
    let (glass, _) = material_look(
        "glass",
        r"shaders\shader_templates\transparent\two_alpha_env",
        vec![],
    );
    src.materials = vec![armour, light, glass];
    src.pieces = vec![quad_piece(0, -0.5), quad_piece(1, 0.0), quad_piece(2, 0.5)];
    src.change_colour = vec![None, None, None];
    let glb = write_glb(&weapon(&src).unwrap());
    gltf::Gltf::from_slice(&glb).expect("valid glTF");
    let (doc, _) = parse_glb(&glb);
    let m = &doc["materials"];
    let tex = |v: &Value| v["index"].as_u64().unwrap();
    let base = tex(&m[0]["pbrMetallicRoughness"]["baseColorTexture"]);
    let spec = tex(&m[0]["pbrMetallicRoughness"]["metallicRoughnessTexture"]);
    let glow = tex(&m[0]["emissiveTexture"]);
    let cc = m[0]["extras"]["h2"]["change_colour"].as_u64().unwrap();
    assert_eq!(
        m[0]["extras"]["h2"]["template"],
        "tex_bump_env_illum_four_change_color"
    );
    assert_eq!(m[0]["emissiveFactor"], json!([1.0, 1.0, 1.0]));
    assert!(m[0].get("alphaMode").is_none());
    let mut all = vec![base, spec, glow, cc];
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 4, "four different pictures");
    // The additive light: its base map became its glow, stored once, shared.
    assert_eq!(m[1]["alphaMode"], "BLEND");
    assert_eq!(m[1]["extras"]["h2"]["blend"], "add");
    assert_eq!(
        tex(&m[1]["emissiveTexture"]),
        base,
        "same picture, one image"
    );
    assert_eq!(doc["images"].as_array().unwrap().len(), 4);
    // Glass: a see-through factor and no picture.
    assert_eq!(
        m[2]["pbrMetallicRoughness"]["baseColorFactor"],
        json!(GLASS)
    );
    assert_eq!(m[2]["alphaMode"], "BLEND");
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
    assert_eq!(ModelsEntry::new(&[], &[], &[]), None);
    let m = ModelsEntry::new(
        &["elite".into(), "spartan".into()],
        &["battle_rifle".into()],
        &[],
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&m).unwrap(),
        json!({
            "version": 2,
            "bipeds": {
                "elite": {"glb": "models/biped/elite.glb"},
                "spartan": {"glb": "models/biped/spartan.glb"}
            },
            "weapons": {"battle_rifle": {"glb": "models/weapon/battle_rifle.glb"}}
        })
    );
    // Vehicles appear once there are any.
    let m = ModelsEntry::new(&[], &[], &["warthog".into()]).unwrap();
    let v = serde_json::to_value(&m).unwrap();
    assert_eq!(
        v["vehicles"],
        json!({"warthog": {"glb": "models/vehicle/warthog.glb"}})
    );
    // An older manifest without vehicles, clips or classes still reads.
    let old: ModelsEntry = serde_json::from_value(json!({
        "version": 1,
        "bipeds": {"spartan": {"glb": "models/biped/spartan.glb"}},
        "weapons": {}
    }))
    .unwrap();
    assert!(old.vehicles.is_empty());
    assert!(old.bipeds["spartan"].clips.is_empty());
}

/// The arm turned `deg` about +z, as a sampled frame.
fn arm_frame(deg: f32) -> Pose {
    pose_of(&[("arm", Some(yaw(deg)))]).1
}

fn run_clip(overlay: bool) -> ClipPoses {
    ClipPoses {
        name: "rifle:run".into(),
        fps: ANIMATION_FPS,
        graph_names: vec!["arm".into()],
        poses: vec![arm_frame(0.0), arm_frame(45.0), arm_frame(90.0)],
        overlay,
    }
}

#[test]
fn skinned_biped_reads_back_with_a_skin_joints_and_clips() {
    let src = biped_source();
    let (names, pose) = pose_of(&[("arm", Some(yaw(30.0)))]);
    let model = biped(&src, &names, &pose, &[run_clip(false)]).unwrap();
    let glb = write_glb(&model);
    let g = gltf::Gltf::from_slice(&glb).expect("valid glTF");
    let skin = g.skins().next().expect("a skin");
    let joints: Vec<String> = skin
        .joints()
        .map(|j| j.name().unwrap().to_owned())
        .collect();
    assert_eq!(joints, ["joint_pelvis", "joint_arm", "joint_forearm"]);
    let ibm = skin.inverse_bind_matrices().unwrap();
    assert_eq!(ibm.count(), 3);
    assert_eq!(ibm.data_type(), gltf::accessor::DataType::F32);
    // The tree: forearm under arm under pelvis under the root.
    let arm = skin.joints().nth(1).unwrap();
    assert_eq!(
        arm.children().map(|c| c.name().unwrap().to_owned()).next(),
        Some("joint_forearm".to_owned())
    );
    let body = g.nodes().find(|n| n.name() == Some("body")).unwrap();
    assert_eq!(body.skin().map(|s| s.index()), Some(0));
    for prim in g.meshes().next().unwrap().primitives() {
        let semantics: Vec<_> = prim.attributes().collect();
        let joints0 = semantics
            .iter()
            .find(|(s, _)| *s == gltf::Semantic::Joints(0))
            .expect("JOINTS_0");
        assert_eq!(joints0.1.data_type(), gltf::accessor::DataType::U16);
        assert!(
            semantics
                .iter()
                .any(|(s, _)| *s == gltf::Semantic::Weights(0))
        );
    }
    // Weights sum to one; joints are in range.
    let (doc, bin) = parse_glb(&glb);
    let attrs = &doc["meshes"][0]["primitives"][0]["attributes"];
    let w = accessor_values(&doc, &bin, attrs["WEIGHTS_0"].as_u64().unwrap());
    let j = accessor_values(&doc, &bin, attrs["JOINTS_0"].as_u64().unwrap());
    for v in w.chunks(4) {
        assert!((v.iter().sum::<f32>() - 1.0).abs() < 1e-5, "{v:?}");
    }
    assert!(j.iter().all(|&k| k < 3.0));
    // The bind matrices undo each joint's default frame.
    let ibm = accessor_values(
        &doc,
        &bin,
        doc["skins"][0]["inverseBindMatrices"].as_u64().unwrap(),
    );
    let world = node_worlds(&doc, &node_locals(&doc, &bin, None));
    for (k, jn) in doc["skins"][0]["joints"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let m: Mat4 = std::array::from_fn(|c| std::array::from_fn(|r| ibm[k * 16 + c * 4 + r]));
        let id = mat_mul(&world[jn.as_u64().unwrap() as usize], &m);
        for c in 0..4 {
            assert!(close(&id[c], &IDENTITY[c]), "{id:?}");
        }
    }
    // One clip, its channels on the joints, rotations as normalised shorts.
    let anims: Vec<_> = g.animations().collect();
    assert_eq!(anims.len(), 1);
    assert_eq!(anims[0].name(), Some("rifle:run"));
    let joint_nodes: Vec<usize> = skin.joints().map(|j| j.index()).collect();
    for c in anims[0].channels() {
        assert!(joint_nodes.contains(&c.target().node().index()));
        if c.target().property() == gltf::animation::Property::Rotation {
            assert_eq!(
                c.sampler().output().data_type(),
                gltf::accessor::DataType::I16
            );
            assert!(c.sampler().output().normalized());
        }
    }
    // The arm moves (three keys at 30 fps); the pelvis never does (one key).
    let input_of = |node: &str, path: gltf::animation::Property| {
        anims[0]
            .channels()
            .find(|c| c.target().node().name() == Some(node) && c.target().property() == path)
            .unwrap()
            .sampler()
            .input()
    };
    let arm_in = input_of("joint_arm", gltf::animation::Property::Rotation);
    assert_eq!(arm_in.count(), 3);
    let end = arm_in.max().unwrap()[0].as_f64().unwrap() as f32;
    assert!((end - 2.0 / 30.0).abs() < 1e-6, "{end}");
    assert_eq!(
        input_of("joint_pelvis", gltf::animation::Property::Rotation).count(),
        1
    );
    // The manifest lists the clip.
    let dir = std::env::temp_dir().join(format!("h2-models-anim-{}", std::process::id()));
    let w = write_models(vec![(Kind::Biped, "spartan".into(), Ok(model))], &dir);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(w.entry.unwrap().bipeds["spartan"].clips, ["rifle:run"]);
}

#[test]
fn clip_keys_are_the_sampled_frames_and_play_like_a_baked_pose() {
    let src = biped_source();
    let (names, pose) = pose_of(&[("arm", Some(yaw(30.0)))]);
    let model = biped(&src, &names, &pose, &[run_clip(false)]).unwrap();
    let glb = write_glb(&model);
    let (doc, bin) = parse_glb(&glb);
    let nodes = doc["nodes"].as_array().unwrap();
    let arm = nodes.iter().position(|n| n["name"] == "joint_arm").unwrap();
    let lift = -pose_mesh(&src, &posed_world(&src.bones, &names, &pose))
        .iter()
        .flat_map(|p| p.positions.iter().map(|v| v[2]))
        .fold(f32::INFINITY, f32::min);
    for (f, deg) in [(0, 0.0f32), (1, 45.0), (2, 90.0)] {
        // The key, back in game axes, is the frame's rotation.
        let locals = node_locals(&doc, &bin, Some(("rifle:run", f)));
        let game = mat_mul(&mat_mul(&invert_rigid(&AXES), &locals[arm]), &AXES);
        let q = quat_from_axes(
            [game[0][0], game[0][1], game[0][2]],
            [game[1][0], game[1][1], game[1][2]],
            [game[2][0], game[2][1], game[2][2]],
        );
        let want = yaw(deg);
        let d: f32 = (0..4).map(|k| q[k] * want[k]).sum();
        assert!(1.0 - d.abs() < 1e-4, "frame {f}: {q:?} vs {want:?}");
        // Skinning the stored mesh to the frame gives the mesh baked in
        // that frame (on the default pose's floor).
        let played = read_drawn_at(&glb, Some(("rifle:run", f)));
        let frame = arm_frame(deg);
        let baked = pose_mesh(&src, &posed_world(&src.bones, &["arm".into()], &frame));
        let mut want: Vec<[f32; 3]> = baked[0]
            .positions
            .iter()
            .map(|p| to_gltf([p[0], p[1], p[2] + lift]))
            .collect();
        let mut got: Vec<[f32; 3]> = played.tris.iter().flat_map(|t| t.map(|v| v.0)).collect();
        // Triangles are drawn in index order; compare as sets of points.
        let key = |p: &[f32; 3]| p.map(|c| (c * 1000.0).round() as i64);
        want.sort_by_key(key);
        want.dedup_by_key(|p| key(p));
        got.sort_by_key(key);
        got.dedup_by_key(|p| key(p));
        assert_eq!(want.len(), got.len());
        for (a, b) in want.iter().zip(&got) {
            assert!(close(a, b), "frame {f}: {a:?} vs {b:?}");
        }
    }
}

#[test]
fn overlays_add_their_change_and_missing_channels_keep_the_default_pose() {
    let bones = skeleton();
    let base = vec![
        ([0.0, 0.0, 0.0], yaw(0.0)),
        ([1.0, 0.0, 0.0], yaw(30.0)),
        ([1.0, 0.0, 0.0], yaw(-20.0)),
    ];
    let names = vec!["arm".to_owned()];
    // A replacement frame: the arm takes the frame's own; the forearm (not
    // in the graph) keeps the default pose's -20, not its rest 0.
    let l = clip_locals(&bones, &names, &arm_frame(25.0), &base, None);
    assert!(close(
        &l[1][0][..3],
        &trs(base[1].0, yaw(25.0), 1.0)[0][..3]
    ));
    assert!(close(
        &l[2][0][..3],
        &trs(base[2].0, yaw(-20.0), 1.0)[0][..3]
    ));
    // An overlay: its change since its first frame (10 -> 25) goes on top
    // of the default 30.
    let first = arm_frame(10.0);
    let l = clip_locals(&bones, &names, &arm_frame(25.0), &base, Some(&first));
    assert!(
        close(&l[1][0][..3], &trs(base[1].0, yaw(45.0), 1.0)[0][..3]),
        "{:?}",
        l[1]
    );
    // Translation: added likewise.
    let mut moved = arm_frame(10.0);
    moved.nodes[0].translation = [0.5, 0.0, 0.0];
    moved.nodes[0].has_translation = true;
    let mut first_t = first.clone();
    first_t.nodes[0].translation = [0.25, 0.0, 0.0];
    first_t.nodes[0].has_translation = true;
    let l = clip_locals(&bones, &names, &moved, &base, Some(&first_t));
    assert!(close(&l[1][3][..3], &[1.25, 0.0, 0.0]), "{:?}", l[1][3]);
}

#[test]
fn clips_are_planned_from_actions_and_animation_names() {
    let st =
        |m: &str, c: &str, t: &str, a: &str| -> Stance { [m.into(), c.into(), t.into(), a.into()] };
    let stances = vec![
        st("combat", "rifle", "any", "idle"),
        st("combat", "rifle", "any", "move_front"),
        st("crouch", "rifle", "any", "idle"),
        st("combat", "rifle", "pr", "idle"),
        st("combat", "any", "any", "landing_dead"),
        st("combat", "unarmed", "any", "idle"),
        st("combat", "dual", "any", "idle"),
        st("combat", "rifle", "any", "airborne"),
        st("combat", "rifle", "any", "land_soft"),
        st("combat", "rifle", "any", "land_hard"),
    ];
    let names: Vec<String> = [
        "combat:rifle:throw_grenade:var0",
        "combat:rifle:sg:melee_strike_1",
        "combat:rifle:br:melee_strike_1",
        "combat:rifle:sg:reload_1",
        "combat:rifle:cb:reload_1",
        "combat:rifle:br:fire_1",
        "combat:rifle:ready",
        "combat:rifle:aim_still_up",
        "combat:pistol:hp:fire_1:var1",
        "h_kill:front:gut:var1",
        "s_kill:front:gut:var1",
        "s_kill:front:gut:var0",
        "s_kill:front:head:var0",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let plan = plan_clips(&stances, &names);
    let got: Vec<(&str, ClipFrom)> = plan.iter().map(|(n, f)| (n.as_str(), *f)).collect();
    assert_eq!(
        got,
        [
            // Dying and lying dead once for every class: a soft kill from
            // the front (its first variant) beats a hard one.
            ("any:die", ClipFrom::Animation(11)),
            ("any:dead", ClipFrom::Action(4)),
            ("rifle:idle", ClipFrom::Action(0)),
            ("rifle:run", ClipFrom::Action(1)),
            ("rifle:crouch", ClipFrom::Action(2)),
            ("rifle:airborne", ClipFrom::Action(7)),
            ("rifle:land", ClipFrom::Action(8)),
            ("rifle:land_hard", ClipFrom::Action(9)),
            // `:var0` counts; the battle rifle's melee beats the shotgun's;
            // a reload only per type goes to the first type by name.
            ("rifle:grenade", ClipFrom::Animation(0)),
            ("rifle:melee", ClipFrom::Animation(2)),
            ("rifle:reload", ClipFrom::Animation(4)),
            ("rifle:fire", ClipFrom::Animation(5)),
            ("rifle:switch", ClipFrom::Animation(6)),
            ("pistol:fire", ClipFrom::Animation(8)),
            ("unarmed:idle", ClipFrom::Action(5)),
        ]
    );
    // Without a death animation, dying is the dead pose.
    let plan = plan_clips(&stances, &names[..9]);
    assert_eq!(plan[0], ("any:die".to_string(), ClipFrom::Action(4)));
    // No dual clips.
    assert!(plan.iter().all(|(n, _)| clip_class(n) != "dual"));
}

#[test]
fn weapons_name_their_class_and_v1_readers_still_find_their_nodes() {
    let src = biped_source();
    let mut w = weapon(&src).unwrap();
    w.class = Some("rifle".into());
    assert!(w.skeleton.is_none());
    let (names, pose) = pose_of(&[("arm", Some(yaw(90.0)))]);
    let animated = biped(&src, &names, &pose, &[run_clip(false)]).unwrap();
    let still = biped(&src, &names, &pose, &[]).unwrap();
    // The stored mesh is the posed one whether or not it is animated, so a
    // reader that ignores the skin draws the same model.
    assert_eq!(
        animated.primitives[0].positions,
        still.primitives[0].positions
    );
    let a = read_drawn(&write_glb(&animated));
    let b = read_drawn(&write_glb(&still));
    for n in ["weapon", "head"] {
        let find = |d: &Drawn| d.nodes.iter().find(|k| k.0 == n).unwrap().clone();
        assert!(close(&find(&a).1, &find(&b).1), "{n}");
    }
    let dir = std::env::temp_dir().join(format!("h2-models-class-{}", std::process::id()));
    let written = write_models(vec![(Kind::Weapon, "battle_rifle".into(), Ok(w))], &dir);
    let _ = fs::remove_dir_all(&dir);
    let entry = written.entry.unwrap();
    assert_eq!(
        serde_json::to_value(&entry.weapons["battle_rifle"]).unwrap(),
        json!({"glb": "models/weapon/battle_rifle.glb", "class": "rifle"})
    );
}

#[test]
fn species_are_biped_directory_names() {
    assert_eq!(
        species_of("objects\\characters\\grunt\\grunt").as_deref(),
        Some("grunt")
    );
    assert_eq!(
        species_of("objects\\characters\\Flood_Combat\\x").as_deref(),
        Some("flood_combat")
    );
    assert_eq!(species_of("lonely"), None);
    assert_eq!(species_of("a b\\c"), None);
    let tags: BTreeMap<String, usize> = [
        ("c\\grunt\\grunt_heretic", 9),
        ("c\\grunt\\grunt", 2),
        ("c\\jackal\\jackal_sniper", 1),
        ("c\\jackal\\jackal_major", 4),
        ("c\\drone\\b", 3),
        ("c\\drone\\a", 3),
    ]
    .iter()
    .map(|(n, c)| (n.to_string(), *c))
    .collect();
    let chosen = choose_bipeds(&tags);
    // Named like its directory wins; else the most common; else the first.
    assert_eq!(chosen["grunt"], "c\\grunt\\grunt");
    assert_eq!(chosen["jackal"], "c\\jackal\\jackal_major");
    assert_eq!(chosen["drone"], "c\\drone\\a");
}

#[test]
fn vehicles_keep_their_rest_pose_and_get_seat_nodes() {
    assert_eq!(seat_node("driver").as_deref(), Some("seat_driver"));
    assert_eq!(
        seat_node("Gunner Seat").as_deref(),
        Some("seat_gunner_seat")
    );
    assert_eq!(
        seat_node("seat_passenger").as_deref(),
        Some("seat_passenger")
    );
    assert_eq!(
        seat_node("passenger_lb").as_deref(),
        Some("seat_passenger_lb")
    );
    assert_eq!(seat_node("left_wheel"), None);
    for not in [
        "driver_enter",
        "passenger_lm_enter01",
        "target_driver",
        "camera_gunner",
        "gunner_left_hand",
        "passenger_lb_camera",
        "seat_effect",
    ] {
        assert_eq!(seat_node(not), None, "{not}");
    }
    let mut src = biped_source();
    src.markers.push(Marker {
        name: "driver".into(),
        node: 1,
        translation: [0.0, 0.25, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
    });
    src.markers.push(Marker {
        name: "driver".into(),
        node: 0,
        translation: [9.0, 9.0, 9.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
    });
    let v = vehicle(&src, &[], &Pose { nodes: vec![] }).unwrap();
    assert_eq!(v.mesh_name, "vehicle");
    // Rest pose, not moved: the mesh is where the source put it.
    assert_eq!(v.primitives[0].positions, src.pieces[0].positions);
    let seats: Vec<_> = v
        .nodes
        .iter()
        .filter(|(n, _)| n.starts_with("seat_"))
        .collect();
    assert_eq!(seats.len(), 1, "one node per seat name");
    assert!(close(&seats[0].1[3][..3], &[1.0, 0.25, 0.0]));
    let glb = write_glb(&v);
    gltf::Gltf::from_slice(&glb).expect("valid glTF");
    let (doc, _) = parse_glb(&glb);
    assert!(
        doc["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["name"] == "seat_driver")
    );
    let mut tiny = src.clone();
    tiny.pieces[0].triangles.truncate(1);
    assert!(
        vehicle(&tiny, &[], &Pose { nodes: vec![] })
            .unwrap_err()
            .to_string()
            .contains("placeholder")
    );
    assert!(
        vehicle(
            &Source {
                pieces: vec![],
                ..src
            },
            &[],
            &Pose { nodes: vec![] }
        )
        .is_err()
    );
}

#[test]
fn vehicles_rest_with_their_hatch_closed() {
    let st = |a: &str, b: &str, c: &str, d: &str| -> Stance { [a, b, c, d].map(str::to_owned) };
    // A canopy: the end of closing, else the start of opening.
    let canopy = vec![
        st("combat", "any", "any", "opening"),
        st("combat", "any", "any", "closing"),
    ];
    assert_eq!(pick_vehicle_pose(&canopy), Some((1, Frame::Last)));
    assert_eq!(pick_vehicle_pose(&canopy[..1]), Some((0, Frame::First)));
    // A turret or a sentinel: its idle.
    let turret = vec![
        st("combat", "fixed", "any", "fire"),
        st("combat", "fixed", "any", "idle"),
    ];
    assert_eq!(pick_vehicle_pose(&turret), Some((1, Frame::First)));
    // Closing wins over idle; nothing usable keeps the rest pose.
    let both = vec![
        st("combat", "any", "any", "idle"),
        st("combat", "any", "any", "closing"),
    ];
    assert_eq!(pick_vehicle_pose(&both), Some((1, Frame::Last)));
    assert_eq!(
        pick_vehicle_pose(&[st("combat", "any", "any", "fire")]),
        None
    );
    assert_eq!(pick_vehicle_pose(&[]), None);

    // The chosen frame is baked like a biped's: the forearm's nose turned
    // 90 degrees moves its vertices and the seat on it.
    let mut src = biped_source();
    src.markers.push(Marker {
        name: "driver".into(),
        node: 2,
        translation: [0.5, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
    });
    let (names, pose) = pose_of(&[("arm", Some(yaw(90.0)))]);
    let v = vehicle(&src, &names, &pose).unwrap();
    let seat = v.nodes.iter().find(|(n, _)| n == "seat_driver").unwrap().1;
    assert!(close(&seat[3][..3], &[1.0, 1.5, 0.0]), "{:?}", seat[3]);
    // Vehicles are not lifted: the object origin stays the origin.
    assert!(
        close(&v.primitives[0].positions[0], &[1.0, -1.0, -0.5]),
        "{:?}",
        v.primitives[0].positions[0]
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
            biped(&src, &names, &pose, &[]),
        ),
        (
            Kind::Biped,
            "elite".to_string(),
            Err(anyhow::anyhow!("broken")),
        ),
        (Kind::Weapon, "smg".to_string(), weapon(&src)),
        (
            Kind::Vehicle,
            "warthog".to_string(),
            vehicle(&src, &[], &Pose { nodes: vec![] }),
        ),
    ];
    let w = write_models(built, &dir);
    let entry = w.entry.unwrap();
    assert_eq!(entry.bipeds.keys().collect::<Vec<_>>(), ["spartan"]);
    assert_eq!(entry.weapons.keys().collect::<Vec<_>>(), ["smg"]);
    assert!(w.skipped["models/biped/elite"].contains("broken"));
    assert!(dir.join("models/biped/spartan.glb").is_file());
    assert!(dir.join("models/weapon/smg.glb").is_file());
    assert!(!dir.join("models/biped/elite.glb").exists());
    assert_eq!(entry.vehicles.keys().collect::<Vec<_>>(), ["warthog"]);
    assert!(dir.join("models/vehicle/warthog.glb").is_file());
    assert_eq!(w.stats.len(), 3);
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
    read_drawn_at(glb, None)
}

fn trs_matrix(t: [f32; 3], r: [f32; 4]) -> Mat4 {
    NodeTransform {
        rotation: r,
        translation: t,
        scale: 1.0,
        ..NodeTransform::default()
    }
    .matrix()
}

/// An accessor's values as floats: f32 as is, i16 normalised, u16 as
/// integers.
fn accessor_values(doc: &Value, bin: &[u8], index: u64) -> Vec<f32> {
    let a = &doc["accessors"][index as usize];
    let v = &doc["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
    let off = v["byteOffset"].as_u64().unwrap() as usize;
    let len = v["byteLength"].as_u64().unwrap() as usize;
    let bytes = &bin[off..off + len];
    match a["componentType"].as_u64().unwrap() {
        5126 => bytes
            .chunks(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect(),
        5122 => bytes
            .chunks(2)
            .map(|c| (i16::from_le_bytes(c.try_into().unwrap()) as f32 / 32767.0).max(-1.0))
            .collect(),
        5123 => bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes(c.try_into().unwrap()) as f32)
            .collect(),
        t => panic!("component type {t}"),
    }
}

/// Every node's local matrix, with the joints of animation `clip` set to
/// its key `frame` (its only key for a still channel).
fn node_locals(doc: &Value, bin: &[u8], clip: Option<(&str, usize)>) -> Vec<Mat4> {
    let nodes = doc["nodes"].as_array().unwrap();
    let mut trs: Vec<([f32; 3], [f32; 4])> = nodes
        .iter()
        .map(|n| {
            (
                n.get("translation")
                    .map_or([0.0; 3], |v| serde_json::from_value(v.clone()).unwrap()),
                n.get("rotation").map_or([0.0, 0.0, 0.0, 1.0], |v| {
                    serde_json::from_value(v.clone()).unwrap()
                }),
            )
        })
        .collect();
    if let Some((name, frame)) = clip {
        let anim = doc["animations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == name)
            .unwrap_or_else(|| panic!("no clip {name}"));
        for ch in anim["channels"].as_array().unwrap() {
            let sampler = &anim["samplers"][ch["sampler"].as_u64().unwrap() as usize];
            let out = accessor_values(doc, bin, sampler["output"].as_u64().unwrap());
            let keys = doc["accessors"][sampler["input"].as_u64().unwrap() as usize]["count"]
                .as_u64()
                .unwrap() as usize;
            let k = frame.min(keys - 1);
            let node = ch["target"]["node"].as_u64().unwrap() as usize;
            match ch["target"]["path"].as_str().unwrap() {
                "rotation" => {
                    let q: [f32; 4] = out[k * 4..k * 4 + 4].try_into().unwrap();
                    let l = q.iter().map(|v| v * v).sum::<f32>().sqrt();
                    trs[node].1 = q.map(|v| v / l);
                }
                "translation" => trs[node].0 = out[k * 3..k * 3 + 3].try_into().unwrap(),
                p => panic!("path {p}"),
            }
        }
    }
    trs.into_iter().map(|(t, r)| trs_matrix(t, r)).collect()
}

/// World matrices of every node of the scene.
fn node_worlds(doc: &Value, locals: &[Mat4]) -> Vec<Mat4> {
    let nodes = doc["nodes"].as_array().unwrap();
    let mut world = vec![IDENTITY; nodes.len()];
    let mut stack: Vec<(usize, Mat4)> = doc["scenes"][0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| (n.as_u64().unwrap() as usize, IDENTITY))
        .collect();
    while let Some((i, parent)) = stack.pop() {
        world[i] = mat_mul(&parent, &locals[i]);
        for c in nodes[i]["children"].as_array().into_iter().flatten() {
            stack.push((c.as_u64().unwrap() as usize, world[i]));
        }
    }
    world
}

/// The glb's triangles, skinned to animation `clip` at key `frame` when
/// given (else drawn as stored, which the default pose must equal).
fn read_drawn_at(glb: &[u8], clip: Option<(&str, usize)>) -> Drawn {
    let (doc, bin) = parse_glb(glb);
    let world = node_worlds(&doc, &node_locals(&doc, &bin, clip));
    let skin: Option<Vec<Mat4>> = doc["skins"].get(0).map(|s| {
        let ibm = accessor_values(&doc, &bin, s["inverseBindMatrices"].as_u64().unwrap());
        s["joints"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(k, j)| {
                let m: Mat4 =
                    std::array::from_fn(|c| std::array::from_fn(|r| ibm[k * 16 + c * 4 + r]));
                mat_mul(&world[j.as_u64().unwrap() as usize], &m)
            })
            .collect()
    });
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
        let (pos, nrm) = match (&skin, a.get("JOINTS_0"), a.get("WEIGHTS_0")) {
            (Some(skin), Some(j), Some(w)) => {
                let j = accessor_values(&doc, &bin, j.as_u64().unwrap());
                let w = accessor_values(&doc, &bin, w.as_u64().unwrap());
                let mut p2 = vec![0.0; pos.len()];
                let mut n2 = vec![0.0; nrm.len()];
                for v in 0..pos.len() / 3 {
                    for k in 0..4 {
                        let m = &skin[j[v * 4 + k] as usize];
                        let wt = w[v * 4 + k];
                        let p = transform_point(m, [pos[v * 3], pos[v * 3 + 1], pos[v * 3 + 2]]);
                        let n = transform_vector(m, [nrm[v * 3], nrm[v * 3 + 1], nrm[v * 3 + 2]]);
                        for c in 0..3 {
                            p2[v * 3 + c] += wt * p[c];
                            n2[v * 3 + c] += wt * n[c];
                        }
                    }
                }
                (p2, n2)
            }
            _ => (pos, nrm),
        };
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
    // Empty nodes (and joints) where they are in the scene.
    for (n, w) in doc["nodes"].as_array().unwrap().iter().zip(&world) {
        if n.get("mesh").is_none() && n.get("translation").is_some() {
            let q = quat_from_axes(
                [w[0][0], w[0][1], w[0][2]],
                [w[1][0], w[1][1], w[1][2]],
                [w[2][0], w[2][1], w[2][2]],
            );
            d.nodes.push((
                n["name"].as_str().unwrap().to_owned(),
                [w[3][0], w[3][1], w[3][2]],
                q,
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
fn picture(items: &[Placed<'_>], view: usize, paint: Paint, w: usize, h: usize) -> Vec<u8> {
    let az: f32 = [0.0f32, 90.0, 40.0][view].to_radians();
    // Camera direction from the origin (glTF): (cos az, 0, sin az).
    let fwd = [-az.cos(), 0.0, -az.sin()];
    let right = cross(fwd, [0.0, 1.0, 0.0]);
    // Fit the first item (the model) for every view: its radius about the
    // vertical axis and its height.
    let (mut radius, mut lo, mut hi) = (1e-3f32, f32::INFINITY, f32::NEG_INFINITY);
    for t in &items[0].0.tris {
        for (p, ..) in t {
            radius = radius.max((p[0] * p[0] + p[2] * p[2]).sqrt());
            lo = lo.min(p[1]);
            hi = hi.max(p[1]);
        }
    }
    let scale = 0.9 * (w as f32 / (2.0 * radius)).min(h as f32 / (hi - lo).max(1e-3));
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0 + (lo + hi) / 2.0 * scale);
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
    let mut seats = String::new();
    for (stem, r) in &entry.vehicles {
        let (doc, _) = parse_glb(&fs::read(out.join(&r.glb)).unwrap());
        let names: Vec<&str> = doc["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["name"].as_str())
            .filter(|n| n.starts_with("seat_"))
            .collect();
        let pose = doc["asset"]["extras"]["pose"]
            .as_str()
            .unwrap_or("-")
            .to_owned();
        seats += &format!("{stem}: pose {pose}; {}\n", names.join(" "));
    }
    print!("{seats}");
    fs::write(check.join("seats.txt"), &seats).unwrap();
    fs::write(
        check.join("models.json"),
        serde_json::to_string_pretty(&entry).unwrap(),
    )
    .unwrap();
    assert!(entry.bipeds.contains_key("spartan") && entry.bipeds.contains_key("elite"));
    assert!(entry.weapons.contains_key("battle_rifle"));
    for v in ["warthog", "ghost", "banshee"] {
        assert!(entry.vehicles.contains_key(v), "{v}");
    }
    for b in ["grunt", "jackal", "marine"] {
        assert!(entry.bipeds.contains_key(b), "{b}");
    }

    // True scale: the glbs are in world units (1 = 10 ft), unscaled. A
    // Spartan stands about 0.7 (7 ft), an elite taller, a Warthog is about
    // two units long.
    let extras =
        |glb: &str| parse_glb(&fs::read(out.join(glb)).unwrap()).0["asset"]["extras"].clone();
    let spartan_h = extras(&entry.bipeds["spartan"].glb)["height"]
        .as_f64()
        .unwrap();
    let elite_h = extras(&entry.bipeds["elite"].glb)["height"]
        .as_f64()
        .unwrap();
    let hog = read_drawn(&fs::read(out.join(&entry.vehicles["warthog"].glb)).unwrap());
    let (lo, hi) = hog.tris.iter().flatten().fold(
        ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]),
        |(lo, hi), (p, ..)| {
            (
                std::array::from_fn(|k| lo[k].min(p[k])),
                std::array::from_fn(|k| hi[k].max(p[k])),
            )
        },
    );
    let hog_len = hi[0] - lo[0];
    println!("scale: spartan {spartan_h:.3}, elite {elite_h:.3}, warthog {hog_len:.3} long");
    assert!((0.6..0.8).contains(&spartan_h), "spartan {spartan_h}");
    assert!(elite_h > spartan_h, "elite {elite_h}");
    assert!((1.5..3.0).contains(&hog_len), "warthog {hog_len}");

    // Looks: the armour's change colour per pixel, glows, glass, team plates.
    let materials = |glb: &str| {
        parse_glb(&fs::read(out.join(glb)).unwrap()).0["materials"]
            .as_array()
            .unwrap()
            .clone()
    };
    let has = |glb: &str, f: &dyn Fn(&Value) -> bool| materials(glb).iter().any(f);
    let cc = |m: &Value| m["extras"]["h2"]["change_colour"].is_u64();
    assert!(
        has(&entry.bipeds["spartan"].glb, &cc),
        "spartan change colour"
    );
    assert!(has(&entry.bipeds["elite"].glb, &cc), "elite change colour");
    assert!(
        has(&entry.bipeds["elite"].glb, &|m| m
            .get("emissiveTexture")
            .is_some()),
        "elite glow"
    );
    assert!(
        has(&entry.vehicles["banshee"].glb, &cc),
        "banshee change colour"
    );
    assert!(
        has(&entry.vehicles["warthog"].glb, &|m| m["alphaMode"]
            == "BLEND"),
        "warthog glass"
    );
    assert!(
        has(&entry.bipeds["spartan"].glb, &|m| m["pbrMetallicRoughness"]
            .get("metallicRoughnessTexture")
            .is_some()),
        "spartan shine"
    );
    // The elite keeps its sword clips.
    assert!(
        entry.bipeds["elite"]
            .clips
            .iter()
            .any(|c| c == "sword:melee"),
        "elite sword"
    );
    // Shots by weapon, from the projectile tags.
    let shot = |w: &str| {
        entry.weapons[w]
            .ballistics
            .clone()
            .unwrap_or_else(|| panic!("{w} ballistics"))
    };
    assert_eq!(shot("battle_rifle").kind, "bullet");
    assert_eq!(shot("plasma_rifle").kind, "plasma");
    assert_eq!(shot("rocket_launcher").kind, "rocket");
    assert!(shot("plasma_rifle").speed.unwrap() < shot("battle_rifle").speed.unwrap());

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
    // secondary), holding the battle rifle at its `weapon` node. Vehicles the
    // same, with the spartan standing at `seat_driver` when there is one.
    let rifle = read_drawn(&fs::read(out.join(&entry.weapons["battle_rifle"].glb)).unwrap());
    let spartan = read_drawn(&fs::read(out.join(&entry.bipeds["spartan"].glb)).unwrap());
    let node = |d: &Drawn, n: &str| {
        d.nodes
            .iter()
            .find(|(k, ..)| k == n)
            .map(|(_, t, q)| (*t, *q))
    };
    let ai = [
        "grunt",
        "jackal",
        "brute",
        "hunter",
        "marine",
        "bugger",
        "floodcombat_human",
        "sentinel_aggressor",
        "prophet",
    ];
    for (species, r) in entry
        .bipeds
        .iter()
        .filter(|(k, _)| ["spartan", "elite"].contains(&k.as_str()) || ai.contains(&k.as_str()))
    {
        let body = read_drawn(&fs::read(out.join(&r.glb)).unwrap());
        let items = [(&body, None), (&rifle, node(&body, "weapon"))];
        write_sheet(&check.join(format!("check-{species}.png")), &items);
    }
    let vehicles = [
        "warthog", "ghost", "banshee", "scorpion", "wraith", "spectre", "pelican", "phantom",
    ];
    for (stem, r) in entry
        .vehicles
        .iter()
        .filter(|(k, _)| vehicles.contains(&k.as_str()))
    {
        let body = read_drawn(&fs::read(out.join(&r.glb)).unwrap());
        let mut items = vec![(&body, None)];
        if let Some(seat) = node(&body, "seat_driver") {
            items.push((&spartan, Some(seat)));
        }
        write_sheet(&check.join(format!("check-vehicle-{stem}.png")), &items);
    }
    animation_checks(&disc, &out, &check, &entry, &rifle);

    // Every vehicle, side and three-quarter, one small picture each (for a
    // contact sheet).
    let all = check.join("vehicles");
    let _ = fs::remove_dir_all(&all);
    fs::create_dir_all(&all).unwrap();
    for (stem, r) in &entry.vehicles {
        let body = read_drawn(&fs::read(out.join(&r.glb)).unwrap());
        let items = [(&body, None)];
        let (pw, ph) = (320usize, 240usize);
        let panels = [
            picture(&items, 1, Paint::Texture, pw, ph),
            picture(&items, 2, Paint::Texture, pw, ph),
        ];
        let mut sheet = vec![0u8; 2 * pw * ph * 4];
        for (k, p) in panels.iter().enumerate() {
            for y in 0..ph {
                let dst = (y * 2 * pw + k * pw) * 4;
                sheet[dst..dst + pw * 4].copy_from_slice(&p[y * pw * 4..(y + 1) * pw * 4]);
            }
        }
        let mut png = Vec::new();
        assets::png::write_rgba(&mut png, 2 * pw as u32, ph as u32, &sheet).unwrap();
        fs::write(all.join(format!("{stem}.png")), png).unwrap();
    }
}

/// The disc test's animation part: sizes and clip sets, the spartan's
/// default pose against its idle clip, its run clip against the sampler,
/// and frame strips of three clips.
fn animation_checks(
    disc: &crate::live_art::Disc,
    out: &Path,
    check: &Path,
    entry: &ModelsEntry,
    rifle: &Drawn,
) {
    let mut report = String::new();
    for (species, r) in &entry.bipeds {
        let size = fs::metadata(out.join(&r.glb)).unwrap().len();
        report += &format!("{species}: {} KiB, {} clips\n", size / 1024, r.clips.len());
        assert!(size as usize <= MAX_BIPED_GLB, "{species} is {size} bytes");
    }
    for (stem, r) in &entry.weapons {
        report += &format!("weapon {stem}: class {:?}\n", r.class);
    }
    assert_eq!(
        entry.weapons["battle_rifle"].class.as_deref(),
        Some("rifle")
    );
    assert_eq!(
        entry.weapons["rocket_launcher"].class.as_deref(),
        Some("missile")
    );
    let clips = &entry.bipeds["spartan"].clips;
    report += &format!("spartan clips: {}\n", clips.join(" "));
    report += &format!("elite clips: {}\n", entry.bipeds["elite"].clips.join(" "));
    for c in [
        "rifle:idle",
        "rifle:run",
        "rifle:grenade",
        "rifle:melee",
        "pistol:idle",
    ] {
        assert!(clips.iter().any(|k| k == c), "spartan lacks {c}");
    }

    // Read back by the gltf crate: one skin over every joint, bind matrices
    // for each, and every clip's channels on joints.
    let glb = fs::read(out.join(&entry.bipeds["spartan"].glb)).unwrap();
    let g = gltf::Gltf::from_slice(&glb).expect("the gltf crate reads the spartan");
    let skin = g.skins().next().expect("a skin");
    let joints: Vec<usize> = skin.joints().map(|j| j.index()).collect();
    assert_eq!(skin.inverse_bind_matrices().unwrap().count(), joints.len());
    assert_eq!(g.animations().count(), clips.len());
    for a in g.animations() {
        for c in a.channels() {
            assert!(joints.contains(&c.target().node().index()));
        }
    }

    // The default pose is the rifle idle's first frame: playing that frame
    // moves no vertex.
    let still = read_drawn(&glb);
    let idle = read_drawn_at(&glb, Some(("rifle:idle", 0)));
    let mut worst = 0.0f32;
    for (a, b) in still.tris.iter().zip(&idle.tris) {
        for k in 0..3 {
            for c in 0..3 {
                worst = worst.max((a[k].0[c] - b[k].0[c]).abs());
            }
        }
    }
    report += &format!("default pose vs rifle:idle frame 0: worst {worst}\n");
    assert!(
        worst < 2e-3,
        "default pose moves under its own frame: {worst}"
    );

    // rifle:run against the sampler: every joint's key, back in game axes,
    // has the rotation (and, below the root, the translation) the clip's
    // frame gives its node.
    let m = disc
        .maps_of(map::CacheType::Multiplayer)
        .find(|m| m.name() == "ascension")
        .unwrap();
    let (mut map, mut files) = disc.open(m).unwrap();
    let id = map
        .tags()
        .iter()
        .find(|t| {
            t.group == BIPD
                && t.name
                    .as_deref()
                    .is_some_and(|n| last_component(n) == "masterchief_mp")
        })
        .unwrap()
        .id;
    let (_, jmad) = object_model(&mut map, id).unwrap();
    let graph = AnimationGraph::read(&mut map, jmad.unwrap()).unwrap();
    let strings = map.strings().to_vec();
    let index = graph
        .find_animation(&strings, "combat:rifle:move_front")
        .unwrap();
    let clip = graph.load_clip(index, &mut Res(&mut files)).unwrap();
    let names: Vec<String> = graph
        .nodes
        .iter()
        .map(|n| n.name.resolve(&strings).unwrap().unwrap_or("").to_owned())
        .collect();
    let (doc, bin) = parse_glb(&glb);
    let joint_names: Vec<String> = joints
        .iter()
        .map(|&j| doc["nodes"][j]["name"].as_str().unwrap()["joint_".len()..].to_owned())
        .collect();
    let (mut rot_worst, mut pos_worst, mut compared) = (0.0f32, 0.0f32, 0);
    for frame in [0, 7, 13, clip.frame_count() - 1] {
        let pose = clip.sample_frame(frame).unwrap();
        let locals = node_locals(&doc, &bin, Some(("rifle:run", frame)));
        for (k, &j) in joints.iter().enumerate() {
            let Some(slot) = names.iter().position(|n| *n == joint_names[k]) else {
                continue;
            };
            let n = &pose.nodes[slot];
            // Back to game axes: AXES^-1 * L * AXES.
            let game = mat_mul(&mat_mul(&invert_rigid(&AXES), &locals[j]), &AXES);
            if n.has_rotation {
                let q = quat_from_axes(
                    [game[0][0], game[0][1], game[0][2]],
                    [game[1][0], game[1][1], game[1][2]],
                    [game[2][0], game[2][1], game[2][2]],
                );
                let d: f32 = (0..4).map(|c| q[c] * n.rotation[c]).sum();
                rot_worst = rot_worst.max(1.0 - d.abs());
                compared += 1;
            }
            let root = doc["nodes"][0]["children"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c.as_u64() == Some(j as u64));
            if n.has_translation && !root {
                for (g, t) in game[3].iter().zip(n.translation) {
                    pos_worst = pos_worst.max((g - t).abs());
                }
            }
        }
    }
    report += &format!(
        "rifle:run vs the sampler: {compared} rotations, worst 1-|dot| {rot_worst}, translations worst {pos_worst}\n"
    );
    assert!(compared > 0);
    assert!(rot_worst < 1e-4, "rotations differ: {rot_worst}");
    assert!(pos_worst < 1e-3, "translations differ: {pos_worst}");
    print!("{report}");
    fs::write(check.join("animation.txt"), &report).unwrap();

    // Strips: six frames of each clip, three-quarter view, holding the rifle.
    let (pw, ph, per) = (200usize, 280usize, 6usize);
    let rows = ["rifle:run", "rifle:grenade", "rifle:melee"];
    let mut sheet = vec![0u8; per * pw * rows.len() * ph * 4];
    for (row, name) in rows.iter().enumerate() {
        let n = doc["animations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == *name)
            .map(|a| {
                a["samplers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| {
                        doc["accessors"][s["input"].as_u64().unwrap() as usize]["count"]
                            .as_u64()
                            .unwrap()
                    })
                    .max()
                    .unwrap() as usize
            })
            .unwrap();
        for k in 0..per {
            let frame = k * (n - 1) / (per - 1);
            let body = read_drawn_at(&glb, Some((name, frame)));
            let hand = body
                .nodes
                .iter()
                .find(|(k, ..)| k == "weapon")
                .map(|(_, t, q)| (*t, *q));
            let items = [(&body, None), (rifle, hand)];
            let p = picture(&items, 2, Paint::Texture, pw, ph);
            for y in 0..ph {
                let dst = ((row * ph + y) * per * pw + k * pw) * 4;
                sheet[dst..dst + pw * 4].copy_from_slice(&p[y * pw * 4..(y + 1) * pw * 4]);
            }
        }
    }
    let mut png = Vec::new();
    assets::png::write_rgba(
        &mut png,
        (per * pw) as u32,
        (rows.len() * ph) as u32,
        &sheet,
    )
    .unwrap();
    let path = check.join("strip-spartan.png");
    fs::write(&path, png).unwrap();
    println!("wrote {}", path.display());
}

fn write_sheet(path: &Path, items: &[Placed<'_>]) {
    let (pw, ph) = (360usize, 520usize);
    let panels: Vec<Vec<u8>> = [
        (0, Paint::Texture),
        (1, Paint::Texture),
        (2, Paint::Texture),
        (0, Paint::ChangeColour),
        (2, Paint::ChangeColour),
    ]
    .iter()
    .map(|&(v, p)| picture(items, v, p, pw, ph))
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
    fs::write(path, png).unwrap();
    println!("wrote {}", path.display());
}

#[test]
fn the_pose_prefers_the_rifle_idle_then_any_idle() {
    let st = |a: &str, b: &str, c: &str, d: &str| -> Stance { [a, b, c, d].map(str::to_owned) };
    let all = vec![
        st("combat", "pistol", "any", "idle"),
        st("crouch", "rifle", "any", "idle"),
        st("combat", "rifle", "any", "move_front"),
        st("combat", "rifle", "any", "idle"),
    ];
    assert_eq!(pick_idle(&all), Some(3));
    assert_eq!(pick_idle(&all[..3]), Some(0));
    // No combat idle: any idle.
    assert_eq!(pick_idle(&all[1..3]), Some(0));
    // A creature with only differently named idles.
    let odd = vec![
        st("any", "any", "any", "move"),
        st("flying", "any", "any", "idle_2"),
    ];
    assert_eq!(pick_idle(&odd), Some(1));
    assert_eq!(pick_idle(&odd[..1]), None);
    // No frame at all: the rest pose.
    let src = biped_source();
    let rest = biped(&src, &[], &Pose { nodes: vec![] }, &[]).unwrap();
    assert_eq!(rest.primitives[0].positions.len(), 4);
}

#[test]
fn a_vertex_without_nodes_follows_its_sections_node() {
    // Indices go through the map.
    assert_eq!(vertex_nodes(&[4, 7], Some([1, 0, 0, 0])), [7, 4, 4, 4]);
    // No map: the indices are model nodes.
    assert_eq!(vertex_nodes(&[], Some([2, 1, 0, 0])), [2, 1, 0, 0]);
    // A section with no per-vertex nodes is bound to its first mapped node.
    assert_eq!(vertex_nodes(&[3], None), [3; 4]);
    assert_eq!(vertex_nodes(&[], None), [0; 4]);
}

/// Prints every material of the player bipeds and the main vehicles: the
/// shader, its template, each bitmap parameter (name, format, size, what
/// role the extractor gives it), and the instance colours and vectors, to
/// `$RUSH_KEEP/viewer-models-fix/shaders.txt`.
/// `cargo test --release -p cli --bin h2 shaders_from_the_disc -- --ignored --exact --nocapture`
#[test]
#[ignore]
fn shaders_from_the_disc() {
    use crate::live_art::Disc;
    let disc_path = std::env::var("H2_DISC").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../../disc/halo2.iso").into()
    });
    let keep = std::env::var("RUSH_KEEP")
        .unwrap_or_else(|_| "/Users/afr/.config/rush/keep/h2recomp-b5afe71a/bed6c8dc".into());
    let disc = Disc::scan(Path::new(&disc_path)).unwrap();
    let mut out = String::new();
    let mut seen = std::collections::BTreeSet::new();
    for m in disc
        .maps_of(map::CacheType::Shared)
        .chain(disc.maps_of(map::CacheType::Multiplayer))
    {
        let (mut map, mut files) = disc.open(m).unwrap();
        let wanted: Vec<(map::TagId, String)> = map
            .tags()
            .iter()
            .filter(|t| (t.group == BIPD || t.group == VEHI) && t.location.is_some())
            .filter_map(|t| Some((t.id, t.name.clone()?)))
            .filter(|(_, n)| seen.insert(n.clone()))
            .collect();
        for (id, name) in wanted {
            let Ok((mode, _)) = object_model(&mut map, id) else {
                continue;
            };
            let Ok(geo) = RenderModelGeometry::read(&mut map, mode, &mut Res(&mut files)) else {
                continue;
            };
            out += &format!("== {name}\n");
            for mat in &geo.materials {
                let shader = mat.shader.id;
                let sname = map
                    .tag(shader)
                    .ok()
                    .and_then(|t| t.name.clone())
                    .unwrap_or_default();
                let (template, bitmaps) = shader_bitmaps(&mut map, shader).unwrap_or_default();
                out += &format!("  shader {sname}\n    template {template}\n");
                let tag = map.tag(shader).unwrap().clone();
                let bytes = map.read_tag_bytes(shader).unwrap();
                let view = TagView::new(&bytes, tag.location.unwrap().address);
                let Ok(post) = view.block(&bytes[..0x54.min(bytes.len())], 0x20, 0x7c) else {
                    out += "    (no postprocess)\n";
                    continue;
                };
                let post = &post[0];
                for (k, id) in bitmaps.iter().enumerate() {
                    let bname = map
                        .tag(*id)
                        .ok()
                        .and_then(|t| t.name.clone())
                        .unwrap_or("-".into());
                    let fmt = match DecodedTag::read(&mut map, *id).map(|d| d.data) {
                        Ok(TagData::Bitmap(b)) => b
                            .images
                            .first()
                            .map(|i| {
                                format!(
                                    "{:?} {:?} {:?} mips {}",
                                    i.format, i.kind, i.dimensions, i.mip_count
                                )
                            })
                            .unwrap_or_default(),
                        _ => "-".into(),
                    };
                    out += &format!(
                        "    bitmap {k}: {bname} [{fmt}] role {:?}\n",
                        bitmap_role(&bname)
                    );
                }
                for c in view.block(post, 0xc, 4).unwrap_or_default() {
                    out += &format!("    colour {:08x}\n", u32_at(c, 0).unwrap());
                }
                for v in view.block(post, 0x14, 0x10).unwrap_or_default() {
                    out += &format!("    vector {:?}\n", f32s::<4>(v, 0).unwrap());
                }
            }
        }
    }
    let dir = Path::new(&keep).join("viewer-models-fix");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("shaders.txt"), &out).unwrap();
    println!("{} lines", out.lines().count());
}
